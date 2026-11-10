# shellcheck shell=bash
#
# Repository packages.
#
# The whole step is skipped when checkupdates reports nothing pending, which
# matters more than it looks: checkupdates works against a private temporary
# database, so on the common "nothing to do" day the real pacman lock is never
# taken at all and a human using pacman is never inconvenienced.

CAU_PACMAN_COUNT=0
CAU_PACMAN_PENDING=''

# Packages that had to be skipped so the rest of the upgrade could go through.
CAU_PACMAN_HELD=''

# Base flags for every unattended pacman invocation.
cau_pacman_flags() {
	printf '%s\n' --noconfirm --color never --disable-download-timeout

	# A progress bar is worth having when somebody is watching a `run` from a
	# terminal; in the timer's log it is only carriage-return noise.
	[[ -n $CAU_INTERACTIVE ]] || printf '%s\n' --noprogressbar

	local pkg
	for pkg in $CFG_IGNORE_PKG; do
		printf '%s\n' --ignore "$pkg"
	done
}

# Bytes per second the downloads are taken to run at, until a run has
# measured this machine's line (state file download_rate).
CAU_PACMAN_RATE=6291456

_cau_pacman_rate() {
	local rate
	rate="$(cau_state_read download_rate "$CAU_PACMAN_RATE")"
	[[ $rate =~ ^[0-9]+$ ]] && (( rate > 0 )) || rate=$CAU_PACMAN_RATE
	printf '%s\n' "$rate"
}

# What pacman prints once the downloads are done: it checks, then runs the
# hooks, then installs. From the first of these on, pacman must not be
# interrupted any more (see control.sh). Not anchored, because with a progress
# bar these lines start with a counter or a carriage return.
CAU_PACMAN_COMMIT_RE='checking keyring|checking keys in keyring|checking package integrity|loading package files|checking for file conflicts|checking available disk space|pre-transaction hooks|Processing package changes|(upgrading|installing|reinstalling|downgrading|removing) [^[:space:]]+'

# Everything the progress bar needs from pacman's log, in one pass and on one
# line: package count, download size in bytes, and how far each phase is.
# Matched against English on purpose: the runner forces LC_ALL=C precisely so
# pacman's output stays parseable.
#
# The database sync prints the same " core downloading..." lines as the
# packages do, so those only count after the "Retrieving packages" header.
#
# In the transaction pacman announces each package in one of two shapes:
#
#   upgrading glibc...              with --noprogressbar, i.e. every timer run
#   ( 12/218) upgrading glibc [##]  with the bar, i.e. an interactive `run`
#
# Only the second carries a counter, so the lines are counted as well. Hooks
# and checks print an (n/m) of their own. Requiring one of the verbs is what
# keeps those out.
CAU_PACMAN_WATCH_AWK='
	BEGIN { unit["B"] = 1; unit["KiB"] = 1024; unit["MiB"] = 1048576; unit["GiB"] = 1073741824 }
	/^Packages? \([0-9]+\)/ && !total { total = $2; gsub(/[^0-9]/, "", total) }
	/^Total Download Size:/ { size = $4 * unit[$5] }
	/^:: Starting full system upgrade/ { upgrade = 1 }
	/^:: Retrieving packages/ { fetch = 1; next }
	fetch && / downloading\.\.\.$/ { started++; file = $1 }
	$0 ~ commit_re { commit = 1 }
	/^(\([ 0-9]+\/[0-9]+\) )?(upgrading|installing|reinstalling|downgrading|removing) [^ ]+/ {
		n++
		if (match($0, /^\([ 0-9]+\/[0-9]+\)/)) {
			split(substr($0, 2, RLENGTH - 2), c, "/")
			counted = c[1]; outof = c[2]
		}
		match($0, /(upgrading|installing|reinstalling|downgrading|removing) [^ ]+/)
		name = substr($0, RSTART, RLENGTH)
		sub(/^[a-z]+ /, "", name); sub(/\.\.\.$/, "", name)
	}
	/^:: Running post-transaction hooks/ { hooks = 1 }
	END {
		printf "%d %.0f %d %d %d %d %d %d %d %d %s %s\n", total, size, upgrade,
			fetch, started, commit, n, counted, outof, hooks,
			(file == "" ? "-" : file), (name == "" ? "-" : name)
	}'

# _cau_pacman_weigh
# About how long the repository steps will take, for the bar. Measured on real
# runs: the checks before the first package take about six seconds, a package
# installs in 0.4, the hooks after them take half as long again, and
# rebuilding the boot image adds half a minute. Eight megabytes a package is
# a stand-in until pacman says how big the download really is.
_cau_pacman_weigh() {
	local n="${CAU_PACMAN_COUNT:-0}" boot=0

	cau_progress_active || return 0
	(( n > 0 )) || return 0

	grep -qE '^(linux|mkinitcpio|systemd|nvidia|[^ ]+-dkms)' <<< "$CAU_PACMAN_PENDING" \
		&& boot=25

	cau_progress_weigh download $(( n * 8388608 / $(_cau_pacman_rate) + 2 ))
	cau_progress_weigh repo $(( 6 + n * 2 / 5 ))
	cau_progress_weigh hooks $(( 5 + n / 5 + boot ))
}

# _cau_pacman_progress_watch <logfile>
# Feeds the desktop's progress bar by watching pacman work, through the four
# stretches a pacman run has. Each one is silent in its own way.
#
#   resolve   pacman works out what the upgrade consists of and prints
#             nothing. On a large backlog that is minutes. The bar goes by the
#             clock, with a label and no counter: a tally frozen at "0 of 161"
#             reads as a stuck update.
#   download  pacman names each package as its download starts, several at a
#             time, and says nothing when one is done. So the bar goes by the
#             bytes that have arrived instead.
#   repo      one line per package. The total comes from the "Package (218)"
#             header: checkupdates knows nothing about new dependencies.
#   hooks     the boot image, the caches. Often as long as the unpacking, and
#             by the clock again.
#
# Read by polling the log rather than from a pipe: the log is written either by
# pacman directly or through tee depending on whether a person is watching, and
# one reader that works for both is worth the second of latency it costs.
_cau_pacman_progress_watch() {
	local log="$1"
	local count="${CAU_PACMAN_COUNT:-0}" phase=resolve labelled='' shown='' t0=$SECONDS
	local total size upgrade fetch started commit n counted outof hooks file name
	local cache rate dir='' bytes got=0 pre since=$(( EPOCHSECONDS - 2 ))

	cache="$(pacman-conf CacheDir 2>/dev/null | head -n1)"
	rate="$(_cau_pacman_rate)"

	while :; do
		sleep 1

		hooks=''
		read -r total size upgrade fetch started commit n counted outof hooks file name \
			< <({ tr '\r' '\n' < "$log" \
				| awk -v commit_re="$CAU_PACMAN_COMMIT_RE" "$CAU_PACMAN_WATCH_AWK"; } 2>/dev/null)
		[[ $hooks == [01] ]] || continue

		(( total > 0 )) && count=$total
		# Where pacman does carry a counter, believe it over the tally.
		if (( outof > 0 )); then
			count=$outof
			n=$counted
		fi

		if (( hooks )); then
			if [[ $phase != hooks ]]; then
				phase=hooks
				cau_progress_step hooks "Finishing the installation"
			fi
			cau_progress_pace
			continue
		fi

		# Checking and unpacking. If nothing was retrieved (every package
		# already in the cache, which is normal after an interrupted run), the
		# download step simply never had a turn.
		if (( commit )); then
			if [[ $phase != repo ]]; then
				# How fast this line really is, for the next run's bar.
				if [[ $phase == download ]] && (( size > 0 && SECONDS - t0 >= 5 )); then
					cau_state_write download_rate \
						$(( (rate + size / (SECONDS - t0)) / 2 )) 2>/dev/null
				fi
				phase=repo
				shown=''
				t0=$SECONDS
				cau_progress_step repo "Updating system packages" "$count"
			fi

			# In fifths of a second, like _cau_pacman_weigh: 30 for the checks,
			# which go by the clock, and 2 for each package.
			pre=10000
			if (( n == 0 )); then
				_cau_progress_pacing $(( SECONDS - t0 )) 6
				pre=$CAU_PROGRESS_PACED
			fi
			cau_progress_at $(( 3 * pre / 1000 + 2 * n )) $(( 30 + 2 * count ))

			if [[ "$n $name" != "$shown" ]]; then
				shown="$n $name"
				cau_progress_count "$n" "$count"
				[[ $name == - ]] || cau_progress_detail "Package" "$name"
			fi
			continue
		fi

		if (( fetch )); then
			if [[ $phase != download ]]; then
				phase=download
				shown=''
				t0=$SECONDS
				(( size > 0 )) && cau_progress_weigh download $(( size / rate + 2 ))
				cau_progress_step download "Downloading updates" "$count"
			fi

			# pacman fetches into a folder of its own inside the cache and moves
			# the files over at the end. What is in there, against the size it
			# announced, is how far the download is. Old runs leave such folders
			# behind, so only one made since this run began counts.
			if [[ -z $dir && -n $cache ]]; then
				dir="$(find "$cache" -maxdepth 1 -type d -name 'download-*' \
					-newermt "@$since" -print -quit 2>/dev/null)"
			fi
			bytes=0
			[[ -n $dir ]] && read -r bytes _ < <(du -sb "$dir" 2>/dev/null)
			[[ $bytes =~ ^[0-9]+$ ]] && (( bytes > got )) && got=$bytes

			if (( size > 0 && got > 0 )); then
				cau_progress_at "$got" "$size"
			else
				# No such folder (an older pacman): by the clock.
				cau_progress_pace
			fi

			if [[ $started != "$shown" ]]; then
				shown="$started"
				cau_progress_count "$started" "$count"
				# Down to the bare name, as the transaction reports it: the file
				# pacman names here carries version, release and architecture.
				[[ $file == - ]] || cau_progress_detail "Package" "${file%-*-*-*}"
			fi
			continue
		fi

		# Still resolving. pacman does mark the point where it stops syncing
		# databases and starts working out the upgrade, and that is the half
		# worth naming, because it is the half that takes the minutes.
		if (( upgrade )) && [[ -z $labelled ]]; then
			labelled=1
			cau_progress_step resolve "Preparing the update"
		fi
		cau_progress_pace
	done
}

# _cau_pacman_exec <logfile> <pacman args...>
# Captures pacman's output for classification, and streams it as well when a
# person is watching. Upgrading a few hundred packages takes minutes; without
# this an interactive run shows one line and then nothing at all, which is
# indistinguishable from a hang and invites someone to kill it mid-transaction.
_cau_pacman_exec() {
	local log="$1"
	shift
	local rc watcher=0

	# Only worth a second process and a grep per second if a bar exists to feed.
	if cau_progress_active; then
		_cau_pacman_progress_watch "$log" &
		watcher=$!
	fi

	# Line-buffered on purpose. pacman writes to a file or through a pipe here,
	# never to a terminal, so libc buffers it in 4KB blocks. And 4KB of
	# "upgrading foo..." is on the order of a hundred and sixty packages. The
	# watcher would see nothing at all, then a hundred and sixty lines at once,
	# which is exactly how a bar comes to sit still and then leap to the end.
	# Guarded rather than assumed: without coreutils there is no bar to feed
	# either, but there is still an update to run.
	local -a buffered=()
	cau_have stdbuf && buffered=(stdbuf -oL)

	if [[ -n $CAU_INTERACTIVE ]]; then
		cau_work pacman "$log" "${buffered[@]}" pacman "$@" 2>&1 | tee "$log"
		rc="${PIPESTATUS[0]}"
	else
		cau_work pacman "$log" "${buffered[@]}" pacman "$@" > "$log" 2>&1
		rc=$?
	fi

	if (( watcher )); then
		kill "$watcher" 2>/dev/null
		wait "$watcher" 2>/dev/null
		cau_progress_sync
	fi

	return "$rc"
}

# cau_pacman_pending
# Fills CAU_PACMAN_PENDING and returns 1 when there is nothing to do.
cau_pacman_pending() {
	local db

	cau_have checkupdates || {
		# Without pacman-contrib we cannot look ahead cheaply; assume work.
		CAU_PACMAN_PENDING=''
		return 0
	}

	# Kept out of CAU_CACHEDIR, which belongs to the unprivileged build
	# account; this one is written by root. pacman keeps its own sync
	# databases under /var/lib too.
	db="${CAU_STATEDIR}/checkupdates-db"
	mkdir -p "$db" 2>/dev/null || db="${TMPDIR:-/tmp}/cachy-auto-update-checkupdates"

	CAU_PACMAN_PENDING="$(CHECKUPDATES_DB="$db" checkupdates --nocolor 2>/dev/null)"
	local rc=$?

	# exit 2 means "no updates", anything else non-zero is a lookup failure and
	# is treated as "might have work" so a transient network hiccup does not
	# silently skip the whole run
	if (( rc == 2 )) || [[ -z $CAU_PACMAN_PENDING ]]; then
		return 1
	fi
	return 0
}

# _cau_pacman_classify <logfile>
# Which kind of unattended failure was this?
_cau_pacman_classify() {
	local log="$1"

	if grep -qiE 'are in conflict|unresolvable package conflicts' "$log"; then
		printf 'conflict\n'
	elif grep -qiE 'could not satisfy dependencies|breaks dependency|unable to satisfy dependency' "$log"; then
		printf 'dependency\n'
	elif grep -qiE 'signature from .* is (unknown trust|marginal trust|invalid)|invalid or corrupted package \(PGP signature\)|key ".*" is unknown|keyring is not writable' "$log"; then
		printf 'keyring\n'
	elif grep -qiE 'exists in filesystem' "$log"; then
		printf 'filesystem\n'
	else
		printf 'other\n'
	fi
}

# _cau_pacman_blockers <logfile>
# The packages standing in the way of an otherwise fine upgrade. pacman names
# them in its dependency errors:
#
#   :: removing libperconaserverclient breaks dependency 'libperconaserverclient'
#      required by heidisql-qt6-bin
#   :: unable to satisfy dependency 'foo' required by bar
#
# In the first form the package being removed is the one to keep; in the second
# it is the package that cannot be installed.
_cau_pacman_blockers() {
	local log="$1"

	{
		sed -nE "s/.*removing ([^ ]+) breaks dependency.*/\\1/p" "$log"
		sed -nE "s/.*unable to satisfy dependency '[^']*' required by ([^ ]+).*/\\1/p" "$log"
	} | grep -E '^[A-Za-z0-9@._+-]+$' | sort -u
}

# cau_pacman_update
# Returns 0 on success (including "nothing to do"), 1 on a failure the user
# needs to hear about. CAU_PACMAN_COUNT holds how many packages moved.
cau_pacman_update() {
	local log kind
	local -a flags

	# checkupdates goes first, against its own private database, and the bar
	# says so rather than naming a step that has not begun. The watcher takes
	# over from here and moves on to the download and repo steps as pacman
	# actually reaches them.
	cau_progress_step resolve "Checking for updates"
	cau_progress_tick_start

	if ! cau_pacman_pending; then
		cau_progress_tick_stop
		cau_progress_forecast
		cau_info "No repository updates pending"
		# None of the steps this would have led to is going to happen, so the
		# rest of the run gets their share of the bar.
		cau_progress_drop download repo hooks
		return 0
	fi
	cau_progress_tick_stop
	cau_progress_forecast

	if [[ -n $CAU_PACMAN_PENDING ]]; then
		CAU_PACMAN_COUNT="$(grep -c . <<< "$CAU_PACMAN_PENDING")"
		[[ $CAU_PACMAN_COUNT =~ ^[0-9]+$ ]] || CAU_PACMAN_COUNT=0
		cau_info "Updating $CAU_PACMAN_COUNT repository package(s)"
		# Deliberately no item count yet. Until pacman has prepared a
		# transaction there is nothing being worked through, and "0 of 161"
		# against a bar that cannot move for the next few minutes is the exact
		# impression the resolve step exists to avoid.
	else
		# checkupdates is unavailable, so the list is unknown and pacman is
		# asked to work it out itself.
		CAU_PACMAN_COUNT=0
		cau_info "Running a full system upgrade (pending list unavailable)"
	fi
	_cau_pacman_weigh

	# Clicked while checkupdates ran: no start notice for an update that is
	# paused or not going to happen.
	if ! cau_control_wait; then
		CAU_PACMAN_COUNT=0
		return 0
	fi

	# Say so before the transaction starts, not after it finishes.
	#
	# While an upgrade runs, a shutdown request is refused by logind and the
	# desktop answers with a polkit password prompt reading "Power off the
	# system while an application is inhibiting this". That never mentions
	# updates and, on a German system, is not even translated. Somebody who was
	# simply told beforehand does not end up staring at that.
	if [[ $CFG_NOTIFY_START == yes ]]; then
		cau_notify_tagged run no normal no \
			"Installing updates" \
			"%d packages are being updated. Please leave the computer switched on until this is done." \
			"${CAU_PACMAN_COUNT:-0}"
	fi

	mapfile -t flags < <(cau_pacman_flags)
	log="$(mktemp)" || return 1

	# Recovery loop rather than a single retry: fixing one problem regularly
	# uncovers the next (a conflict resolved into a dependency error, say).
	# Each remedy is applied at most once, so this always terminates.
	local -a extra=() blockers=() tried=()
	local attempt=0 b

	while true; do
		if _cau_pacman_exec "$log" -Syu "${flags[@]}" "${extra[@]}"; then
			cat "$log" >> "$CAU_RUNLOG" 2>/dev/null
			grep -E '^(removing|replacing) ' "$log" 2>/dev/null \
				| while read -r line; do cau_info "  $line"; done
			rm -f "$log"
			return 0
		fi

		cat "$log" >> "$CAU_RUNLOG" 2>/dev/null

		# Stopped from the desktop, not failed. After a pause the same attempt
		# runs again; what was downloaded so far is still in the cache.
		if cau_control_stopped || cau_control_cancelled; then
			cau_control_wait && continue
			cau_info "pacman was stopped from the desktop"
			rm -f "$log"
			CAU_PACMAN_COUNT=0
			CAU_PACMAN_HELD=''
			return 0
		fi

		kind="$(_cau_pacman_classify "$log")"
		cau_warn "pacman -Syu failed ($kind)"

		if (( ++attempt > 3 )) || [[ " ${tried[*]} " == *" $kind "* ]]; then
			break
		fi
		tried+=("$kind")

		case "$kind" in
			keyring)
				# A stale keyring is the one failure that is always safe to fix
				# automatically, and it blocks everything else until it is.
				cau_info "Refreshing keyrings and retrying"
				local -a keyrings=()
				pacman -Qq archlinux-keyring &> /dev/null && keyrings+=(archlinux-keyring)
				pacman -Qq cachyos-keyring   &> /dev/null && keyrings+=(cachyos-keyring)
				if (( ${#keyrings[@]} )); then
					cau_run_logged cau_work other '' \
						pacman -Sy --noconfirm --color never "${keyrings[@]}" || true
				fi
				;;

			conflict)
				# A package that has to replace another one. --noconfirm already
				# answers "Replace X with Y?" affirmatively; what it declines is
				# ":: X and Y are in conflict. Remove Y? [y/N]". --ask is pacman's
				# question bitmask: 4 = CONFLICT_PKG, 16 = REMOVE_PKGS.
				if [[ $CFG_RESOLVE_CONFLICTS != yes ]]; then
					cau_error "Package conflict requires a decision (AutoResolveConflicts is off)"
					break
				fi
				cau_info "Resolving package conflicts automatically and retrying"
				extra+=(--ask=20)
				;;

			dependency)
				# Something installed still depends on a package the repos want
				# to drop or replace, almost always an AUR package that has not
				# caught up yet. Nothing here can fix that, and it is not worth
				# failing over: letting one stuck package block every other
				# update indefinitely is far worse on an unattended machine.
				# Hold the blockers back and upgrade everything else.
				mapfile -t blockers < <(_cau_pacman_blockers "$log")
				if (( ${#blockers[@]} == 0 )); then
					cau_error "Dependency problem with no package to hold back"
					break
				fi
				for b in "${blockers[@]}"; do
					extra+=(--ignore "$b")
				done
				CAU_PACMAN_HELD="${blockers[*]}"
				cau_warn "Holding back ${blockers[*]} and retrying without them"
				;;

			filesystem)
				# Untracked files in the way. Forcing --overwrite here could
				# silently clobber something the user put there deliberately, so
				# this one stays a human decision.
				cau_error "Files on disk conflict with the update; manual review needed"
				break
				;;

			*)
				# Mostly a mirror that was down or a download that broke off.
				# A corrupt file in the cache is deleted by pacman itself, as
				# --noconfirm answers its question. So: wait a moment, fetch
				# fresh databases and try again.
				cau_info "Refreshing the package databases and retrying"
				sleep 15
				extra+=(-y)
				;;
		esac
	done

	rm -f "$log"
	CAU_PACMAN_COUNT=0
	CAU_PACMAN_HELD=''
	return 1
}

# cau_pacman_reboot_needed
# The running kernel's module tree no longer carries a vmlinuz, so the kernel
# package was replaced underneath us.
cau_pacman_reboot_needed() {
	[[ ! -f "/usr/lib/modules/$(uname -r)/vmlinuz" ]]
}

# cau_pacman_pacnew_count
# Purely informational; the files themselves are never touched.
cau_pacman_pacnew_count() {
	cau_have pacdiff || { printf '0\n'; return; }
	pacdiff -o 2>/dev/null | grep -c . || printf '0\n'
}

# cau_pacman_cleanup
# Optional and off by default.
cau_pacman_cleanup() {
	local -a orphans

	cau_progress_step cleanup "Cleaning up after the update"
	cau_progress_tick_start

	if [[ $CFG_REMOVE_ORPHANS == yes ]]; then
		mapfile -t orphans < <(pacman -Qtdq 2>/dev/null)
		if (( ${#orphans[@]} )); then
			cau_info "Removing ${#orphans[@]} orphaned package(s)"
			cau_run_logged pacman -Rns --noconfirm --color never "${orphans[@]}" \
				|| cau_warn "Removing orphans failed"
		fi
	fi

	if [[ $CFG_CLEAN_CACHE == yes ]] && cau_have paccache; then
		# paccache's dry run reports what a real run would reclaim; logging it
		# first is the only way to tell afterwards whether trimming is doing
		# anything, since the real run says little.
		local reclaim
		reclaim="$( { paccache -d --nocolor -k"$CFG_KEEP_OLD"; paccache -du --nocolor -k0; } 2>&1 \
			| grep -oE 'disk space saved: [^)]*' | paste -sd', ' -)"

		cau_info "Trimming the package cache (keeping $CFG_KEEP_OLD old version(s))${reclaim:+: $reclaim}"
		cau_run_logged paccache -r --nocolor -k"$CFG_KEEP_OLD"  || cau_warn "paccache -r failed"
		cau_run_logged paccache -ru --nocolor -k0               || cau_warn "paccache -ru failed"
	fi
}
