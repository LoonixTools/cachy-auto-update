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
# repository step" into one number for the bar.
#
# If anything is missing along the way (no session, no Plasma, no Python
# bindings), this does nothing at all and the update proceeds exactly as before.

CAU_PROGRESS_HELPER="${CAU_LIBEXECDIR}/cachy-auto-update-progress"

# One entry per session being driven; the indices line up across all four.
CAU_PROGRESS_FDS=()
CAU_PROGRESS_PIDS=()
CAU_PROGRESS_FIFOS=()
CAU_PROGRESS_LOCALES=()

# What each step is worth on the bar. Rough shares of a typical run rather than
# anything measured: the repositories dominate (fetching them and unpacking
# them about equally, on a domestic line), and the cleanup is a rounding error.
# They do not have to add up to 100. Only the steps a given run will actually
# perform are counted, and the total is normalised against those.
#
# "resolve" is everything pacman does before it has a transaction: syncing the
# databases and working out what the upgrade actually consists of. It is
# usually seconds, which is why it is worth so little. But on a large backlog
# it is minutes, and those minutes used to be spent looking at a bar that had
# not moved yet.
declare -A CAU_PROGRESS_WEIGHTS=(
	[resolve]=10 [download]=30 [repo]=40 [aur]=15 [flatpak]=10 [appimage]=3
	[cleanup]=2
)

CAU_PROGRESS_PLAN=()
CAU_PROGRESS_SCALE=0
CAU_PROGRESS_BASE=0
CAU_PROGRESS_SPAN=0
CAU_PROGRESS_TOTAL=0
CAU_PROGRESS_SHOWN=-1

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

# cau_progress_begin <step-id...>
# Opens the progress entry in every graphical session, and records which steps
# this run is going to perform so the bar can be scaled to them.
cau_progress_begin() {
	local user uid fifo pid
	local fd=''

	CAU_PROGRESS_PLAN=("$@")
	CAU_PROGRESS_SCALE=0
	local step
	for step in "${CAU_PROGRESS_PLAN[@]}"; do
		CAU_PROGRESS_SCALE=$(( CAU_PROGRESS_SCALE + ${CAU_PROGRESS_WEIGHTS[$step]:-0} ))
	done
	(( CAU_PROGRESS_SCALE > 0 )) || return 0

	[[ $CFG_NOTIFICATIONS == yes ]] || return 0
	[[ -x $CAU_PROGRESS_HELPER ]] || return 0
	mkdir -p "$CAU_RUNDIR" 2>/dev/null || return 0

	while read -r user uid; do
		[[ -n $user ]] || continue

		# The helper speaks D-Bus through GLib's Python bindings. Checked here
		# rather than left to fail inside the helper, because a helper that
		# gave up immediately would leave nobody draining the pipe.
		cau_as_user "$user" "$uid" sh -c \
			'command -v python3 >/dev/null 2>&1 && python3 -c "import gi" 2>/dev/null' \
			|| continue

		fifo="${CAU_RUNDIR}/progress.${uid}"
		rm -f "$fifo" 2>/dev/null
		mkfifo -m 0600 "$fifo" 2>/dev/null || continue
		chown "$uid" "$fifo" 2>/dev/null || true

		cau_as_user "$user" "$uid" "$CAU_PROGRESS_HELPER" < "$fifo" > /dev/null 2>&1 &
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
}

# cau_progress_active
# Whether anybody is listening. For callers that would otherwise do work whose
# only purpose is to feed the bar.
cau_progress_active() {
	(( ${#CAU_PROGRESS_FDS[@]} ))
}

# cau_progress_drop <step-id...>
# Takes steps out of the plan and rescales the bar to what is left.
#
# Which steps a run will perform is only half known up front. The other half
# turns up while it runs: nothing to download because every package was already
# in the cache, no AUR updates pending, no Flatpaks installed. A step like that
# keeps its whole share of the bar and then hands it over in a single jump the
# moment the next one starts. That is precisely the stutter this is here to
# remove. Dropping it hands its share to the steps that do have work instead,
# so the bar advances at a steady pace rather than leaping across the gaps.
#
# It is also what lets the weights above stay rough: they never have to be
# right about a step that does not run, only about the ones that do.
#
# Only ever called for a step that has not started, so nothing already behind
# the bar is rescaled and the bar does not travel backwards.
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

	CAU_PROGRESS_PLAN=("${kept[@]}")
	CAU_PROGRESS_SCALE=0
	for step in "${CAU_PROGRESS_PLAN[@]}"; do
		CAU_PROGRESS_SCALE=$(( CAU_PROGRESS_SCALE + ${CAU_PROGRESS_WEIGHTS[$step]:-0} ))
	done

	# Nothing left to weigh against would divide by zero further down. Cannot
	# happen while cleanup is unconditional, but this is cheaper than relying
	# on that staying true.
	(( CAU_PROGRESS_SCALE > 0 )) || CAU_PROGRESS_SCALE=1
}

# cau_progress_step <step-id> <label-msgid> [item-count]
# Moves on to the next step. The bar jumps to where that step begins, so a step
# that reported fewer items than it promised still completes rather than
# leaving a gap.
cau_progress_step() {
	local id="$1" label="$2" total="${3:-0}"
	local step i fd base=0

	(( ${#CAU_PROGRESS_FDS[@]} )) || return 0

	local found=0
	for step in "${CAU_PROGRESS_PLAN[@]}"; do
		[[ $step == "$id" ]] && { found=1; break; }
		base=$(( base + ${CAU_PROGRESS_WEIGHTS[$step]:-0} ))
	done

	# A step that was dropped for having no work is not a step to move to.
	# Without this the loop above would fall off the end of the plan and hand
	# back the sum of every weight, i.e. send the bar straight to 100%.
	(( found )) || return 0

	CAU_PROGRESS_BASE=$base
	CAU_PROGRESS_SPAN=${CAU_PROGRESS_WEIGHTS[$id]:-0}
	CAU_PROGRESS_TOTAL=$total

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

	cau_progress_item 0
}

# _cau_progress_pct <numerator> <denominator>
# How far through the current step we are, as a share of its span, turned into
# one number for the whole run and sent on if it has moved.
_cau_progress_pct() {
	local num="$1" den="$2" pct scaled

	if (( den > 0 )); then
		scaled=$(( CAU_PROGRESS_BASE * 100 + CAU_PROGRESS_SPAN * 100 * num / den ))
	else
		scaled=$(( CAU_PROGRESS_BASE * 100 ))
	fi

	pct=$(( scaled / CAU_PROGRESS_SCALE ))
	(( pct > 100 )) && pct=100

	# Never backwards. Two honest things can ask for that: dropping a step
	# rescales the run against a smaller total, and the conflict-recovery loop
	# restarts pacman (and with it the item tally) from the top. Both are
	# real, neither is a reason to show somebody a bar that retreats.
	(( pct < CAU_PROGRESS_SHOWN )) && pct=$CAU_PROGRESS_SHOWN

	# Only when the whole number changes. Percent is the one field the runner
	# would otherwise rewrite for every package on a 500-package upgrade.
	(( pct == CAU_PROGRESS_SHOWN )) && return 0
	CAU_PROGRESS_SHOWN=$pct
	_cau_progress_line 'percent\t%s' "$pct"
}

# cau_progress_item <processed> [total]
# How far through the current step we are.
cau_progress_item() {
	local processed="$1" total="${2:-$CAU_PROGRESS_TOTAL}"

	(( ${#CAU_PROGRESS_FDS[@]} )) || return 0
	[[ $processed =~ ^[0-9]+$ ]] || return 0

	if [[ $total =~ ^[0-9]+$ ]] && (( total > 0 )); then
		(( processed > total )) && processed=$total
		if (( total != CAU_PROGRESS_TOTAL )); then
			CAU_PROGRESS_TOTAL=$total
			_cau_progress_line 'total\t%s' "$total"
		fi
		_cau_progress_line 'done\t%s' "$processed"
		_cau_progress_pct "$processed" "$total"
	else
		_cau_progress_pct 0 0
	fi
}

# cau_progress_creep <seconds-elapsed>
# Moves the bar through a step whose length cannot be known in advance.
#
# Some of a run has no counter to offer and never will. pacman prints nothing
# whatsoever between "starting full system upgrade" and the transaction it
# eventually prepares; an AUR helper compiling a package prints plenty, none of
# it countable. On a large backlog either is minutes. There is no honest number
# to show for that. But a bar that has not moved since it appeared is read as
# a hang, and somebody who reads it that way reaches for the power button in
# the middle of an update. That is the failure this is here to prevent.
#
# So it creeps, along a curve that approaches the end of the step without ever
# reaching it: half the step's share after HALFLIFE seconds, three quarters
# after three times that, the whole of it never. Nothing is claimed that is not
# known: the item counter stays empty throughout, and it is the field that
# would be lying if it moved. The step still finishes the instant real work
# reports in, because every real report is further along than the creep.
#
# Confined to the step's own span, so a creep can never overtake the step that
# comes after it however long it is left running.
CAU_PROGRESS_CREEP_HALFLIFE=45

cau_progress_creep() {
	local elapsed="$1"

	(( ${#CAU_PROGRESS_FDS[@]} )) || return 0
	[[ $elapsed =~ ^[0-9]+$ ]] || return 0

	_cau_progress_pct "$elapsed" $(( elapsed + CAU_PROGRESS_CREEP_HALFLIFE ))
}

# cau_progress_creep_start / cau_progress_creep_stop
# The same, for a step that blocks in one long call instead of polling: the
# ticker runs alongside it and is stopped when it returns. Only one at a time,
# and starting a second one replaces the first.
CAU_PROGRESS_CREEP_PID=0
CAU_PROGRESS_CREEP_T0=0

cau_progress_creep_start() {
	cau_progress_creep_stop
	(( ${#CAU_PROGRESS_FDS[@]} )) || return 0

	CAU_PROGRESS_CREEP_T0=$SECONDS
	local t0=$SECONDS
	{
		# Waiting without forking a sleep every two seconds, for the same
		# reason cau_progress_begin opens its fifo read-write: a pipe held open
		# at both ends never reports end-of-file, so a timed read on it blocks
		# for exactly the timeout and nothing else. A forked sleep would also
		# survive the kill below (it is a child of this subshell, not this
		# subshell) and inherit the fifo's write end, which would keep the
		# helper from seeing the end of its input until the sleep ran out.
		local nap
		exec {nap}<> <(:)
		while :; do
			read -r -t 2 -u "$nap" _ || true
			cau_progress_creep $(( SECONDS - t0 ))
		done
	} &
	CAU_PROGRESS_CREEP_PID=$!
}

cau_progress_creep_stop() {
	(( CAU_PROGRESS_CREEP_PID )) || return 0
	kill "$CAU_PROGRESS_CREEP_PID" 2>/dev/null
	wait "$CAU_PROGRESS_CREEP_PID" 2>/dev/null
	CAU_PROGRESS_CREEP_PID=0

	# The ticker moved the bar from inside a subshell, so this side never saw
	# it happen and still believes the bar is where it was left. Catching up
	# costs one recomputation, since the curve is a function of elapsed time and
	# nothing else. Without it the next ordinary report from here would be
	# measured against a stale percentage and send the bar backwards.
	cau_progress_creep $(( SECONDS - CAU_PROGRESS_CREEP_T0 ))
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

# cau_progress_end [outcome: ok|failed] [failure-msgid]
# Closes the entry. Must run on every exit path, including a killed run: an
# entry whose owner merely vanishes is reported by the desktop as "the
# application closed unexpectedly", which would end every update with a failure
# notice. Safe to call twice, and safe to call when nothing was ever opened.
#
#   ok             the bar fills and the entry goes away
#   failed         the entry goes away from wherever the bar had got to
#   failed <msgid> and the desktop labels it as failed, with that text
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
	cau_progress_creep_stop
	cau_progress_tail_stop

	# The last step never consumes its own share (nothing reports items for
	# the cleanup), so the bar would stop a few percent short of the end and
	# vanish there. Only on the way out of a run that actually worked, though:
	# filling the bar for a failed update says the opposite of what happened.
	[[ $outcome == ok ]] && _cau_progress_line 'percent\t100'

	for i in "${!CAU_PROGRESS_FDS[@]}"; do
		fd="${CAU_PROGRESS_FDS[$i]}"
		[[ -n $fd ]] || continue

		CAU_MSG_RESULT=''
		[[ -n $msgid ]] && cau_msg_into "${CAU_PROGRESS_LOCALES[$i]}" "$msgid"

		printf 'end\t%s\n' "$CAU_MSG_RESULT" >&"$fd" 2>/dev/null || true
		exec {fd}>&-
	done

	for i in "${!CAU_PROGRESS_PIDS[@]}"; do
		wait "${CAU_PROGRESS_PIDS[$i]}" 2>/dev/null
	done

	for i in "${!CAU_PROGRESS_FIFOS[@]}"; do
		rm -f "${CAU_PROGRESS_FIFOS[$i]}" 2>/dev/null
	done

	CAU_PROGRESS_FDS=()
	CAU_PROGRESS_PIDS=()
	CAU_PROGRESS_FIFOS=()
	CAU_PROGRESS_LOCALES=()
	CAU_PROGRESS_SHOWN=-1
}
