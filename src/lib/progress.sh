# shellcheck shell=bash
#
# The update's progress bar on the desktop.
#
# An unattended upgrade can take twenty minutes, and for most of that a user is
# told only that "an update is running". This drives the desktop's job list
# (the same widget that shows a bar while Dolphin copies files), so how far
# along the run is stays visible the whole time.
#
# The desktop ends the progress entry as soon as the D-Bus connection that
# asked for it goes away, which no one-shot bus client can survive. So an
# actual process per session holds that connection open and takes instructions
# on stdin; see cachy-auto-update-progress. Everything below is the writing end
# of those pipes, plus the arithmetic that turns "package 120 of 260 in the
# repository step" into one number for the bar. That number follows the work:
# every step is weighed by how long it should take for the packages this run
# really has, so the bar moves at about the same pace from start to end.
#
# If anything is missing along the way (no session, no Plasma, no Python
# bindings), this does nothing at all and the update proceeds exactly as before.

CAU_PROGRESS_HELPER="${CAU_LIBEXECDIR}/cachy-auto-update-progress"

# One entry per session being driven; the indices line up across all four.
CAU_PROGRESS_FDS=()
CAU_PROGRESS_PIDS=()
CAU_PROGRESS_FIFOS=()
CAU_PROGRESS_LOCALES=()

# What each step is worth on the bar: about how many seconds it takes. These
# are first guesses. The run puts in better numbers as soon as it knows how
# many packages there are and how big they are (cau_progress_weigh).
declare -A CAU_PROGRESS_WEIGHTS=(
	[resolve]=10 [download]=30 [repo]=15 [hooks]=15 [aur]=30 [flatpak]=10
	[userflatpak]=5 [appimage]=5 [cleanup]=3
)

# The steps still to do, the current one first.
CAU_PROGRESS_PLAN=()
CAU_PROGRESS_STEP=''

# Where the bar is, in hundredths of a percent. A step is measured the same
# way: FRAC is how far it has come. The bar was at ANCHOR when the step had
# come FROM, and the steps still to do share the rest of the bar from there.
# So nothing that changes the plan can make the bar jump: it only changes how
# fast the bar moves from here on.
CAU_PROGRESS_POS=0
CAU_PROGRESS_ANCHOR=0
CAU_PROGRESS_FROM=0
CAU_PROGRESS_FRAC=0
CAU_PROGRESS_SHOWN=-1
CAU_PROGRESS_TOTAL=0

# For work that goes by the clock: when the current item began, and how many
# items are done.
CAU_PROGRESS_T0=0
CAU_PROGRESS_DONE=0

# Where a background shell leaves the bar's position for the main one.
CAU_PROGRESS_STATE="${CAU_RUNDIR}/progress.pos"

# _cau_progress_send <line>
# The same instruction to every session. A session whose helper has exited is
# dropped rather than written to: the runner writes into a pipe, and a pipe
# nobody is draining fills up and would eventually block the update itself.
_cau_progress_send() {
	local i fd

	for i in "${!CAU_PROGRESS_FDS[@]}"; do
		fd="${CAU_PROGRESS_FDS[$i]}"
		[[ -n $fd ]] || continue

		if ! kill -0 "${CAU_PROGRESS_PIDS[$i]}" 2>/dev/null; then
			CAU_PROGRESS_FDS[$i]=''
			continue
		fi

		printf '%s\n' "$1" >&"$fd" 2>/dev/null || CAU_PROGRESS_FDS[$i]=''
	done
}

# _cau_progress_line <format> [printf args...]
# Assembled with printf -v rather than in a command substitution: this is on
# the per-package path of a large upgrade, and a fork per line is a fork too
# many for something whose entire job is to be unobtrusive.
_cau_progress_line() {
	local line
	# shellcheck disable=SC2059  # the format is ours; the arguments are numbers
	printf -v line "$@"
	_cau_progress_send "$line"
}

# _cau_progress_popups <user> <uid>
# Applies MinimizeProgress. Plasma cannot open one job hidden, only switch off
# the popups of a whole application, so this sets "Show popups" for our desktop
# entry. Our other notifications carry no desktop entry, so only the bar is
# affected. Written only when it differs, so the file is not touched every run.
_cau_progress_popups() {
	local user="$1" uid="$2" want=true have
	local -a key=(--file plasmanotifyrc --group Applications --group "$CAU_NAME" --key ShowPopups)

	[[ $CFG_MINIMIZE_PROGRESS == yes && -z ${CAU_BY_HAND:-} ]] && want=false

	have="$(cau_as_user "$user" "$uid" kreadconfig6 "${key[@]}" --default true 2>/dev/null)" \
		|| return 0
	[[ $have == "$want" ]] && return 0

	# --notify, so a running desktop picks it up right away
	if [[ $want == true ]]; then
		cau_as_user "$user" "$uid" kwriteconfig6 "${key[@]}" --delete --notify 2>/dev/null
	else
		cau_as_user "$user" "$uid" kwriteconfig6 "${key[@]}" --notify false 2>/dev/null
	fi
	return 0
}

# cau_progress_begin <step-id...>
# Opens the progress entry in every graphical session, and records which steps
# this run is going to perform so the bar can be shared out between them.
cau_progress_begin() {
	local user uid fifo pid
	local fd=''

	CAU_PROGRESS_PLAN=("$@")
	(( $# )) || return 0

	# The desktop pops the bar up as it opens, which reads as a start notice.
	# Not so after a click on Update now: then the bar is the answer to it.
	if [[ -z ${CAU_BY_HAND:-} ]]; then
		[[ $CFG_NOTIFICATIONS == yes ]] || return 0
		[[ $CFG_NOTIFY_START == yes ]] || return 0
	fi
	[[ -x $CAU_PROGRESS_HELPER ]] || return 0
	mkdir -p "$CAU_RUNDIR" 2>/dev/null || return 0
	rm -f "$CAU_PROGRESS_STATE" "$CAU_PROGRESS_FORECAST" 2>/dev/null

	# Where the helpers report clicks on Cancel and Pause. Without it they do
	# not offer either button.
	local -a controls=()
	cau_control_open && controls=(--controls)

	while read -r user uid; do
		[[ -n $user ]] || continue

		# The helper speaks D-Bus through GLib's Python bindings. Checked here
		# rather than left to fail inside the helper, because a helper that
		# gave up immediately would leave nobody draining the pipe.
		cau_as_user "$user" "$uid" sh -c \
			'command -v python3 >/dev/null 2>&1 && python3 -c "import gi" 2>/dev/null' \
			|| continue

		_cau_progress_popups "$user" "$uid"

		fifo="${CAU_RUNDIR}/progress.${uid}"
		rm -f "$fifo" 2>/dev/null
		mkfifo -m 0600 "$fifo" 2>/dev/null || continue
		chown "$uid" "$fifo" 2>/dev/null || true

		if [[ -n $CAU_CONTROL_FD ]]; then
			cau_as_user "$user" "$uid" "$CAU_PROGRESS_HELPER" "${controls[@]}" \
				< "$fifo" >&"$CAU_CONTROL_FD" 2>/dev/null &
		else
			cau_as_user "$user" "$uid" "$CAU_PROGRESS_HELPER" < "$fifo" > /dev/null 2>&1 &
		fi
		pid=$!

		# Read-write deliberately. Opening the writing end of a fifo blocks
		# until a reader shows up, so if the helper died on the way in, the
		# update would hang here for good. O_RDWR never blocks, and the helper
		# still sees end-of-file once this descriptor is closed.
		if ! exec {fd}<> "$fifo"; then
			kill "$pid" 2>/dev/null
			rm -f "$fifo" 2>/dev/null
			continue
		fi

		CAU_PROGRESS_FDS+=("$fd")
		CAU_PROGRESS_PIDS+=("$pid")
		CAU_PROGRESS_FIFOS+=("$fifo")
		CAU_PROGRESS_LOCALES+=("$(cau_user_locale "$user" "$uid")")
	done < <(cau_active_session_users)

	if (( ${#CAU_PROGRESS_FDS[@]} )); then
		cau_control_start
	else
		cau_control_stop
	fi
}

# cau_progress_active
# Whether anybody is listening. For callers that would otherwise do work whose
# only purpose is to feed the bar.
cau_progress_active() {
	(( ${#CAU_PROGRESS_FDS[@]} ))
}

# _cau_progress_anchor
# Pins the bar where it is. Called before the plan changes.
_cau_progress_anchor() {
	CAU_PROGRESS_ANCHOR=$CAU_PROGRESS_POS
	CAU_PROGRESS_FROM=$CAU_PROGRESS_FRAC
}

# cau_progress_drop <step-id...>
# Takes steps out of the plan: no AUR updates pending, no Flatpaks installed.
# The steps that do have work share the rest of the bar.
cau_progress_drop() {
	local drop step
	local -a kept=()

	(( ${#CAU_PROGRESS_FDS[@]} )) || return 0

	for step in "${CAU_PROGRESS_PLAN[@]}"; do
		for drop in "$@"; do
			[[ $step == "$drop" ]] && continue 2
		done
		kept+=("$step")
	done

	(( ${#kept[@]} == ${#CAU_PROGRESS_PLAN[@]} )) && return 0

	_cau_progress_anchor
	CAU_PROGRESS_PLAN=("${kept[@]}")
	for drop in "$@"; do
		[[ $drop == "$CAU_PROGRESS_STEP" ]] && CAU_PROGRESS_STEP=''
	done
	return 0
}

# cau_progress_weigh <step-id> <seconds>
# A better guess at how long a step takes. For a step that is already
# running, call cau_progress_step again afterwards: the number then stands for
# what the step still has to do.
cau_progress_weigh() {
	local id="$1" seconds="$2"

	(( ${#CAU_PROGRESS_FDS[@]} )) || return 0
	[[ -n $id && $seconds =~ ^[0-9]+$ ]] || return 0
	(( seconds > 0 )) || seconds=1

	_cau_progress_anchor
	CAU_PROGRESS_WEIGHTS[$id]=$seconds
}

# cau_progress_step <step-id> <label-msgid> [item-count]
# Moves on to a step, or starts the next part of the current one. The bar
# stays where it is. The steps before this one are over, whatever they
# reported, and this step and the ones after it share what is left.
cau_progress_step() {
	local id="$1" label="$2" total="${3:-0}"
	local i fd at=-1

	(( ${#CAU_PROGRESS_FDS[@]} )) || return 0

	for i in "${!CAU_PROGRESS_PLAN[@]}"; do
		[[ ${CAU_PROGRESS_PLAN[$i]} == "$id" ]] && { at=$i; break; }
	done

	# A step that was dropped for having no work is not a step to move to.
	(( at >= 0 )) || return 0

	CAU_PROGRESS_PLAN=("${CAU_PROGRESS_PLAN[@]:$at}")
	CAU_PROGRESS_STEP=$id
	CAU_PROGRESS_ANCHOR=$CAU_PROGRESS_POS
	CAU_PROGRESS_FROM=0
	CAU_PROGRESS_FRAC=0
	CAU_PROGRESS_T0=$SECONDS
	CAU_PROGRESS_DONE=0
	CAU_PROGRESS_TOTAL=$total

	# What a background shell left there belongs to the part before this one.
	: 2>/dev/null > "$CAU_PROGRESS_STATE"

	# The label is the one line a user actually reads, so it is rendered in
	# each session's own locale rather than the run's C locale.
	for i in "${!CAU_PROGRESS_FDS[@]}"; do
		fd="${CAU_PROGRESS_FDS[$i]}"
		[[ -n $fd ]] || continue
		cau_msg_into "${CAU_PROGRESS_LOCALES[$i]}" "$label"
		printf 'info\t%s\n' "$CAU_MSG_RESULT" >&"$fd" 2>/dev/null || true
	done

	# Unconditionally, including the zero case: a step with no item count of
	# its own would otherwise keep displaying the previous step's tally, and
	# "260 of 260 items" under the heading "Flatpaks" is worse than no count.
	_cau_progress_line 'total\t%s' "$total"
	_cau_progress_line 'done\t%s' 0

	_cau_progress_move 0
}

# _cau_progress_move <fraction>
# The current step has come this far, in hundredths of a percent. Works out
# where that puts the bar and sends it on if the whole percent has changed.
_cau_progress_move() {
	local frac="$1" step weight=0 rest=0 den pos pct

	# Never backwards. A pacman that is started again after a conflict counts
	# from the top, and that is no reason to show a bar that retreats.
	(( frac > 10000 )) && frac=10000
	(( frac < CAU_PROGRESS_FRAC )) && frac=$CAU_PROGRESS_FRAC
	CAU_PROGRESS_FRAC=$frac

	for step in "${CAU_PROGRESS_PLAN[@]}"; do
		if [[ $step == "$CAU_PROGRESS_STEP" ]]; then
			weight=${CAU_PROGRESS_WEIGHTS[$step]:-0}
		else
			rest=$(( rest + ${CAU_PROGRESS_WEIGHTS[$step]:-0} ))
		fi
	done

	# This step's share of what was left of the bar at the anchor, times how
	# much of the step has been done since.
	pos=$CAU_PROGRESS_ANCHOR
	den=$(( weight * (10000 - CAU_PROGRESS_FROM) + rest * 10000 ))
	(( den > 0 )) && pos=$(( pos + (10000 - pos) * weight * (frac - CAU_PROGRESS_FROM) / den ))
	(( pos > CAU_PROGRESS_POS )) && CAU_PROGRESS_POS=$pos

	# A background shell moves the bar in its own copy of all this. It leaves
	# the result where the main shell picks it up (cau_progress_sync).
	(( BASH_SUBSHELL )) && printf '%s %s %s\n' "${CAU_PROGRESS_STEP:--}" \
		"$CAU_PROGRESS_POS" "$CAU_PROGRESS_FRAC" 2>/dev/null > "$CAU_PROGRESS_STATE"

	# Only when the whole number changes.
	pct=$(( CAU_PROGRESS_POS / 100 ))
	(( pct == CAU_PROGRESS_SHOWN )) && return 0
	CAU_PROGRESS_SHOWN=$pct
	_cau_progress_line 'percent\t%s' "$pct"
}

# cau_progress_sync
# Takes over where a background shell left the bar. Call it once that shell
# is gone.
cau_progress_sync() {
	local step pos frac

	(( ${#CAU_PROGRESS_FDS[@]} )) || return 0
	{ read -r step pos frac < "$CAU_PROGRESS_STATE"; } 2>/dev/null || return 0
	[[ $pos =~ ^[0-9]+$ && $frac =~ ^[0-9]+$ ]] || return 0

	(( pos > CAU_PROGRESS_POS )) && CAU_PROGRESS_POS=$pos
	CAU_PROGRESS_SHOWN=$(( CAU_PROGRESS_POS / 100 ))
	[[ $step == "$CAU_PROGRESS_STEP" ]] && (( frac > CAU_PROGRESS_FRAC )) \
		&& CAU_PROGRESS_FRAC=$frac
	_cau_progress_anchor
}

# cau_progress_count <processed> [total]
# The item counter under the bar. Does not move the bar.
cau_progress_count() {
	local processed="$1" total="${2:-$CAU_PROGRESS_TOTAL}"

	(( ${#CAU_PROGRESS_FDS[@]} )) || return 0
	[[ $processed =~ ^[0-9]+$ && $total =~ ^[0-9]+$ ]] || return 0
	(( total > 0 )) || return 0

	(( processed > total )) && processed=$total
	if (( total != CAU_PROGRESS_TOTAL )); then
		CAU_PROGRESS_TOTAL=$total
		_cau_progress_line 'total\t%s' "$total"
	fi
	_cau_progress_line 'done\t%s' "$processed"
}

# cau_progress_at <done> <of>
# How far through the current step we are, in any unit: packages, bytes.
cau_progress_at() {
	local num="$1" den="$2"

	(( ${#CAU_PROGRESS_FDS[@]} )) || return 0
	[[ $num =~ ^[0-9]+$ && $den =~ ^[0-9]+$ ]] || return 0
	(( den > 0 )) || return 0
	(( num > den )) && num=$den

	_cau_progress_move $(( num * 10000 / den ))
}

# cau_progress_item <processed> [total]
# Both at once, for a step that counts its items.
cau_progress_item() {
	cau_progress_count "$@"
	cau_progress_at "$1" "${2:-$CAU_PROGRESS_TOTAL}"
}

# _cau_progress_pacing <seconds> <expected>
# How far work is that cannot be counted, into CAU_PROGRESS_PACED. It moves
# at an even pace and reaches 90% when the expected time is up. From there it
# gets slower and slower and never arrives. An estimate can be wrong, but a
# bar that stands still looks like a hang, and somebody who reads it that way
# reaches for the power button in the middle of an update.
CAU_PROGRESS_PACED=0

_cau_progress_pacing() {
	local s="$1" e="$2"

	(( e > 0 )) || e=1
	if (( s <= e )); then
		CAU_PROGRESS_PACED=$(( 9000 * s / e ))
	else
		CAU_PROGRESS_PACED=$(( 9000 + 1000 * (s - e) / s ))
	fi
}

# cau_progress_pace [done total [piece]]
# Moves the bar by the clock. <done> of <total> is finished, and the piece in
# progress is <piece> of that total (1 if not given). The step's weight is how
# long it all should take, so the piece gets its part of that time, and the
# clock starts again whenever more is done.
cau_progress_pace() {
	local finished="${1:-0}" total="${2:-1}" piece="${3:-1}"

	(( ${#CAU_PROGRESS_FDS[@]} )) || return 0
	[[ -n $CAU_PROGRESS_STEP ]] || return 0
	(( total > 0 )) || total=1
	(( finished > total )) && finished=$total
	(( finished + piece > total )) && piece=$(( total - finished ))

	if (( finished > CAU_PROGRESS_DONE )); then
		CAU_PROGRESS_DONE=$finished
		CAU_PROGRESS_T0=$SECONDS
	fi

	_cau_progress_pacing $(( SECONDS - CAU_PROGRESS_T0 )) \
		$(( ${CAU_PROGRESS_WEIGHTS[$CAU_PROGRESS_STEP]:-1} * piece / total ))
	_cau_progress_move $(( (CAU_PROGRESS_DONE * 10000 + CAU_PROGRESS_PACED * piece) / total ))
}

# cau_progress_tick_start [total [count-command...]] / cau_progress_tick_stop
# cau_progress_pace once a second, next to a step that blocks in one long
# call. The count command prints how many of the step's items are done. If
# the items are not all the same size it prints two more numbers, in seconds
# of the step's weight: the work that is done, and the piece in progress.
# Only one ticker at a time, and starting a second one replaces the first.
CAU_PROGRESS_TICK_PID=0

cau_progress_tick_start() {
	local total="${1:-1}"
	(( $# )) && shift

	cau_progress_tick_stop
	(( ${#CAU_PROGRESS_FDS[@]} )) || return 0

	{
		# Waiting without forking a sleep every second, for the same reason
		# cau_progress_begin opens its fifo read-write: a pipe held open at both
		# ends never reports end-of-file, so a timed read on it blocks for
		# exactly the timeout and nothing else. A forked sleep would also
		# survive the kill below (it is a child of this subshell, not this
		# subshell) and inherit the fifo's write end, which would keep the
		# helper from seeing the end of its input until the sleep ran out.
		local nap finished=0 seen secs piece
		exec {nap}<> <(:)
		while :; do
			read -r -t 1 -u "$nap" _ || true
			seen='' secs='' piece=''
			(( $# )) && read -r seen secs piece < <("$@" 2>/dev/null)
			if [[ $seen =~ ^[0-9]+$ ]] && (( seen > finished )); then
				finished=$seen
				cau_progress_count "$finished" "$total"
			fi
			if [[ $secs =~ ^[0-9]+$ && $piece =~ ^[0-9]+$ ]]; then
				cau_progress_pace "$secs" \
					"${CAU_PROGRESS_WEIGHTS[$CAU_PROGRESS_STEP]:-1}" "$piece"
			else
				cau_progress_pace "$finished" "$total"
			fi
		done
	} &
	CAU_PROGRESS_TICK_PID=$!
}

cau_progress_tick_stop() {
	(( CAU_PROGRESS_TICK_PID )) || return 0
	kill "$CAU_PROGRESS_TICK_PID" 2>/dev/null
	wait "$CAU_PROGRESS_TICK_PID" 2>/dev/null
	CAU_PROGRESS_TICK_PID=0
	cau_progress_sync
}

# cau_progress_forecast_start / cau_progress_forecast
# Counts the AUR and Flatpak updates in the background while the run looks at
# the repositories. Their steps then have a fair share of the bar from the
# start, instead of a guess that is put right only when their turn comes.
# Only for the bar: each step still looks for itself.
CAU_PROGRESS_FORECAST="${CAU_RUNDIR}/progress.forecast"
CAU_PROGRESS_FORECAST_PID=0
CAU_PROGRESS_FORECAST_DONE=''

cau_progress_forecast_start() {
	(( ${#CAU_PROGRESS_FDS[@]} )) || return 0

	{
		# Quiet. The steps log the same things when they run.
		cau_log() { :; }
		local aur=- flatpak='- -'
		[[ $CFG_AUR == yes ]] && aur="$(cau_aur_forecast)"
		[[ $CFG_FLATPAK == yes ]] && cau_have flatpak && flatpak="$(cau_flatpak_forecast)"
		printf '%s %s\n' "${aur:--}" "$flatpak" > "$CAU_PROGRESS_FORECAST"
	} > /dev/null 2>&1 &
	CAU_PROGRESS_FORECAST_PID=$!
}

# Puts the counts in once they are there. Does not wait for them.
cau_progress_forecast() {
	local aur flatpak mine

	(( ${#CAU_PROGRESS_FDS[@]} )) || return 0
	[[ -z $CAU_PROGRESS_FORECAST_DONE ]] || return 0
	{ read -r aur flatpak mine < "$CAU_PROGRESS_FORECAST"; } 2>/dev/null || return 0
	CAU_PROGRESS_FORECAST_DONE=1

	# A few seconds on top: each step looks for itself before it starts.
	[[ $aur =~ ^[0-9]+$ ]] \
		&& cau_progress_weigh aur $(( 3 + aur ))
	[[ $flatpak =~ ^[0-9]+$ ]] \
		&& cau_progress_weigh flatpak $(( 5 + flatpak * CAU_FLATPAK_SECONDS ))
	[[ $mine =~ ^[0-9]+$ ]] \
		&& cau_progress_weigh userflatpak $(( 1 + mine * CAU_FLATPAK_SECONDS ))
	return 0
}

# cau_progress_detail <label-msgid> <value>
# A labelled line under the entry's "Details", for example which package is
# being unpacked right now.
cau_progress_detail() {
	local label="$1" value="$2" i fd

	(( ${#CAU_PROGRESS_FDS[@]} )) || return 0

	for i in "${!CAU_PROGRESS_FDS[@]}"; do
		fd="${CAU_PROGRESS_FDS[$i]}"
		[[ -n $fd ]] || continue
		cau_msg_into "${CAU_PROGRESS_LOCALES[$i]}" "$label"
		printf 'detail\t%s\t%s\n' "$CAU_MSG_RESULT" "$value" >&"$fd" 2>/dev/null || true
	done
}

# cau_progress_suspended <0|1>
# Shows the run as paused, or as running again.
cau_progress_suspended() {
	_cau_progress_line 'suspended\t%s' "$1"
}

# cau_progress_note <label-msgid>
# A line shown instead of the step's label until the run pauses. Empty clears
# it.
cau_progress_note() {
	local msgid="$1" i fd

	(( ${#CAU_PROGRESS_FDS[@]} )) || return 0

	for i in "${!CAU_PROGRESS_FDS[@]}"; do
		fd="${CAU_PROGRESS_FDS[$i]}"
		[[ -n $fd ]] || continue
		CAU_MSG_RESULT=''
		[[ -n $msgid ]] && cau_msg_into "${CAU_PROGRESS_LOCALES[$i]}" "$msgid"
		printf 'note\t%s\n' "$CAU_MSG_RESULT" >&"$fd" 2>/dev/null || true
	done
}

# How many lines of the run log the job entry carries, and how wide each one
# is allowed to be. Both are display limits rather than arbitrary ones: five
# lines is about what fits under a notification popup before it starts pushing
# the buttons off the bottom, and a line long enough to be elided anyway is
# only costing room in the pipe. Together they also keep one instruction well
# inside PIPE_BUF, which is what makes the write atomic against the runner
# writing its own progress down the same pipe.
CAU_PROGRESS_TAIL_LINES=5
CAU_PROGRESS_TAIL_COLS=120
CAU_PROGRESS_TAIL_PID=0

# cau_progress_log <label-msgid> <text>
# The second description field, holding the last few lines of the run log.
#
# The protocol is one instruction per line, so the tail travels tab separated
# and is put back together on the other side. Tabs inside a log line would
# split it in two on the way, so they become spaces first. The job view
# renders either as whitespace, and a line broken in half renders as nonsense.
cau_progress_log() {
	local label="$1" text="$2" i fd

	(( ${#CAU_PROGRESS_FDS[@]} )) || return 0

	text="${text//$'\t'/ }"
	text="${text//$'\n'/$'\t'}"

	for i in "${!CAU_PROGRESS_FDS[@]}"; do
		fd="${CAU_PROGRESS_FDS[$i]}"
		[[ -n $fd ]] || continue
		cau_msg_into "${CAU_PROGRESS_LOCALES[$i]}" "$label"
		printf 'log\t%s\t%s\n' "$CAU_MSG_RESULT" "$text" >&"$fd" 2>/dev/null || true
	done
}

# cau_progress_tail_start <logfile> / cau_progress_tail_stop
# Follows the run log for as long as the update lasts, so expanding "Details"
# shows what the update is actually doing right now.
#
# This is the answer to the part of a run that has no counter and never will:
# an AUR helper compiling for a quarter of an hour says plenty about what it is
# up to, none of it countable, and all of it going into a log file nobody is
# looking at. The percentage says the update is alive; these lines say what it
# is alive doing.
#
# Polled rather than followed with tail -F, for the same reason the pacman
# watcher polls: only the last few lines are ever displayed, so every line in
# between is work nobody would see. Re-read whole and compared, which also
# makes truncation and rotation of the log a non-event.
cau_progress_tail_start() {
	local file="$1"

	cau_progress_tail_stop
	(( ${#CAU_PROGRESS_FDS[@]} )) || return 0
	cau_have tail || return 0

	{
		local nap last='' now
		exec {nap}<> <(:)
		while :; do
			read -r -t 2 -u "$nap" _ || true
			now="$(tail -n "$CAU_PROGRESS_TAIL_LINES" "$file" 2>/dev/null \
				| cut -c "1-${CAU_PROGRESS_TAIL_COLS}")"
			[[ -n $now && $now != "$last" ]] || continue
			last="$now"
			cau_progress_log "Log" "$now"
		done
	} &
	CAU_PROGRESS_TAIL_PID=$!
}

cau_progress_tail_stop() {
	(( CAU_PROGRESS_TAIL_PID )) || return 0
	kill "$CAU_PROGRESS_TAIL_PID" 2>/dev/null
	wait "$CAU_PROGRESS_TAIL_PID" 2>/dev/null
	CAU_PROGRESS_TAIL_PID=0
}

# cau_progress_end [outcome: ok|failed|cancelled] [failure-msgid]
# Closes the entry. Must run on every exit path, including a killed run: an
# entry whose owner merely vanishes is reported by the desktop as "the
# application closed unexpectedly", which would end every update with a failure
# notice. Safe to call twice, and safe to call when nothing was ever opened.
#
#   ok             the bar fills and the entry goes away
#   failed         the entry goes away from wherever the bar had got to
#   failed <msgid> and the desktop labels it as failed, with that text
#   cancelled      the entry goes away without a word, the click said it all
#
# The distinction between the last two is which message the user ends up with.
# An ordinary failure already sends a notification that stays until dismissed,
# and two messages about one problem is one too many; a run that was killed
# sends nothing at all, so there the label is the only thing that explains why
# a bar that was at 40% is suddenly gone.
cau_progress_end() {
	local outcome="${1:-ok}" msgid="${2:-}"
	local i fd

	# Before the descriptors go: anything still running in the background holds
	# its own copy of them, so the helper would not see the end of its input
	# until it exited. And it is about to be waited on.
	cau_progress_tick_stop
	cau_progress_tail_stop
	if (( CAU_PROGRESS_FORECAST_PID )); then
		kill "$CAU_PROGRESS_FORECAST_PID" 2>/dev/null
		wait "$CAU_PROGRESS_FORECAST_PID" 2>/dev/null
		CAU_PROGRESS_FORECAST_PID=0
	fi
	cau_control_stop

	# The last step goes by the clock and never quite arrives, so the bar
	# would stop a few percent short of the end and vanish there. Only on the
	# way out of a run that actually worked, though: filling the bar for a
	# failed update says the opposite of what happened.
	[[ $outcome == ok ]] && _cau_progress_line 'percent\t100'

	for i in "${!CAU_PROGRESS_FDS[@]}"; do
		fd="${CAU_PROGRESS_FDS[$i]}"
		[[ -n $fd ]] || continue

		if [[ $outcome == cancelled ]]; then
			printf 'killed\n' >&"$fd" 2>/dev/null || true
		else
			CAU_MSG_RESULT=''
			[[ -n $msgid ]] && cau_msg_into "${CAU_PROGRESS_LOCALES[$i]}" "$msgid"
			printf 'end\t%s\n' "$CAU_MSG_RESULT" >&"$fd" 2>/dev/null || true
		fi
		exec {fd}>&-
	done

	for i in "${!CAU_PROGRESS_PIDS[@]}"; do
		wait "${CAU_PROGRESS_PIDS[$i]}" 2>/dev/null
	done

	for i in "${!CAU_PROGRESS_FIFOS[@]}"; do
		rm -f "${CAU_PROGRESS_FIFOS[$i]}" 2>/dev/null
	done
	rm -f "$CAU_PROGRESS_STATE" "$CAU_PROGRESS_FORECAST" 2>/dev/null

	CAU_PROGRESS_FDS=()
	CAU_PROGRESS_PIDS=()
	CAU_PROGRESS_FIFOS=()
	CAU_PROGRESS_LOCALES=()
	CAU_PROGRESS_PLAN=()
	CAU_PROGRESS_STEP=''
	CAU_PROGRESS_POS=0
	CAU_PROGRESS_SHOWN=-1
}
