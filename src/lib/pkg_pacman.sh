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

# The verbs pacman puts in front of a package as it works through a
# transaction. Matched against English on purpose: the runner forces LC_ALL=C
# precisely so pacman's output stays parseable.
CAU_PACMAN_OP_RE='^(\([[:space:]]*[0-9]+/[0-9]+\) )?(upgrading|installing|reinstalling|downgrading|removing) [^[:space:]]+'

# Before any of that, everything has to be fetched, and on a domestic line
# that is the longer half of the run: two hundred packages take minutes to
# arrive and seconds to unpack. pacman prints one line per package while it
# does it,
#
#    glibc-2.44+r24+g16be1518495f-1-x86_64_v3 downloading...
#
# and nothing else: no counter, no total. So the position here is counted the
# same way the transaction is.
#
# The database sync a few lines earlier prints the very same shape (" core
# downloading..."), and pacman strips the suffix that would tell a database
# from a package, so the count begins only after the header that separates the
# two phases.
CAU_PACMAN_DL_AWK='
	/^:: Retrieving packages/ { retrieving = 1; next }
	retrieving && / downloading\.\.\.$/ { n++; name = $1 }
	END { print n + 0, name }'

# _cau_pacman_progress_watch <logfile>
# Feeds the desktop's progress bar by watching pacman work, through the three
# phases a pacman run has: first it works out what the upgrade consists of,
# then everything is fetched, then everything is unpacked. Three steps on the
# bar rather than one, because they are three stretches to sit through. And
# each one is silent in its own way.
#
# The first is the one that used to look like a hang. Between "starting full
# system upgrade" and the transaction it eventually prepares, pacman says
# nothing whatsoever, and on a large backlog that silence is minutes long. The
# only honest thing to show there is a label and no counter at all: a tally
# frozen at "0 of 161" reads as a stuck update, where "Preparing the update"
# with no number reads as what it actually is.
#
# In the transaction, pacman announces each package twice over, in one of two
# shapes, and which one depends on a flag this program sets itself:
#
#   upgrading glibc...              with --noprogressbar, i.e. every timer run
#   ( 12/218) upgrading glibc [##]  with the bar, i.e. an interactive `run`
#
# Only the second carries a counter, and the unattended runs that this bar
# exists for are exactly the ones that do not get it. So the position is
# counted here instead (one line per package), and the total taken from the
# "Package (218)" header pacman prints before it starts. That header is the
# better number anyway: checkupdates counts packages with an update available
# and knows nothing about the new dependencies pulled in alongside them.
#
# What must not be counted is the other (n/m) sequence pacman prints, for
# hooks and for checking keys, integrity and file conflicts. Each of those runs
# up to its own total, so following them would drive the bar to the end several
# times before the first package was unpacked. Requiring one of the verbs above
# is what excludes them.
#
# Read by polling the log rather than from a pipe: the log is written either by
# pacman directly or through tee depending on whether a person is watching, and
# one reader that works for both is worth the second of latency it costs.
_cau_pacman_progress_watch() {
	local log="$1"
	local total="${CAU_PACMAN_COUNT:-0}" announced processed line pkg last=''
	local phase=resolve fetched shown='' labelled=''
	local t0=$SECONDS

	while :; do
		sleep 1

		announced="$(grep -aoE '^Packages? \([0-9]+\)' "$log" 2>/dev/null \
			| head -n1 | grep -oE '[0-9]+')"
		[[ $announced =~ ^[0-9]+$ ]] && (( announced > 0 )) && total="$announced"

		line="$(grep -aoE "$CAU_PACMAN_OP_RE" "$log" 2>/dev/null | tail -n1)"

		if [[ -n $line ]]; then
			# Unpacking has started, so whatever came before it is over. If
			# nothing was ever retrieved (every package already sitting in the
			# cache, which is normal after a run that was interrupted once
			# already), then the download step never happened,
			# and it is dropped rather than handed its whole share of the bar in
			# exchange for no work at all.
			if [[ $phase != install ]]; then
				[[ $phase == download ]] || cau_progress_drop download
				phase=install
				cau_progress_step repo "Updating system packages" "$total"
			fi

			# Nothing new since the last look. Checked before the counting grep
			# because on a large upgrade this loop spends most of its life here.
			[[ $line != "$last" ]] || continue
			last="$line"

			processed="$(grep -acE "$CAU_PACMAN_OP_RE" "$log" 2>/dev/null)"
			[[ $processed =~ ^[0-9]+$ ]] || continue

			# Where pacman does carry a counter, believe it over the tally: it
			# is the same number, but it also knows the true total.
			if [[ $line =~ ^\([[:space:]]*([0-9]+)/([0-9]+)\) ]]; then
				processed="${BASH_REMATCH[1]}"
				total="${BASH_REMATCH[2]}"
			fi

			cau_progress_item "$processed" "$total"

			pkg="${line##* }"
			cau_progress_detail "Package" "${pkg%...}"
			continue
		fi

		# Fetching. The header is what tells this apart from the database sync
		# a few lines earlier, which prints the very same shape.
		if grep -qa '^:: Retrieving packages' "$log" 2>/dev/null; then
			if [[ $phase == resolve ]]; then
				phase=download
				cau_progress_step download "Downloading updates" "$total"
			fi

			read -r fetched pkg < <(awk "$CAU_PACMAN_DL_AWK" "$log" 2>/dev/null)
			[[ $fetched =~ ^[0-9]+$ ]] || continue
			[[ $fetched != "$shown" ]] || continue
			shown="$fetched"

			cau_progress_item "$fetched" "$total"
			# Down to the bare name, as the transaction reports it: the file
			# pacman names here carries version, release and architecture.
			[[ -n $pkg ]] && cau_progress_detail "Package" "${pkg%-*-*-*}"
			continue
		fi

		# Still resolving. pacman does mark the point where it stops syncing
		# databases and starts working out the upgrade, and that is the half
		# worth naming, because it is the half that takes the minutes.
		if [[ $phase == resolve && $labelled != upgrade ]] \
			&& grep -qa '^:: Starting full system upgrade' "$log" 2>/dev/null; then
			labelled=upgrade
			cau_progress_step resolve "Preparing the update"
		fi

		# Nothing countable happens in here at all, so the bar creeps instead.
		# This is the stretch that used to look like a hung update.
		cau_progress_creep $(( SECONDS - t0 ))
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
		"${buffered[@]}" pacman "$@" 2>&1 | tee "$log"
		rc="${PIPESTATUS[0]}"
	else
		"${buffered[@]}" pacman "$@" > "$log" 2>&1
		rc=$?
	fi

	if (( watcher )); then
		kill "$watcher" 2>/dev/null
		wait "$watcher" 2>/dev/null
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

	if ! cau_pacman_pending; then
		cau_info "No repository updates pending"
		# Neither of the two steps this would have led to is going to happen,
		# so the rest of the run gets their share of the bar instead of
		# watching it jump 70% the moment Flatpaks start.
		cau_progress_drop download repo
		return 0
	fi

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
			# The watcher decided the same thing in a subshell, so a step it
			# dropped is still in the plan out here. Same question, same answer,
			# and the two copies agree on what the rest of the run is scaled
			# against. Only on the way out: a failed attempt is about to be
			# retried, and that retry may well download after all.
			grep -qa '^:: Retrieving packages' "$log" 2>/dev/null \
				|| cau_progress_drop download
			cat "$log" >> "$CAU_RUNLOG" 2>/dev/null
			grep -E '^(removing|replacing) ' "$log" 2>/dev/null \
				| while read -r line; do cau_info "  $line"; done
			rm -f "$log"
			return 0
		fi

		cat "$log" >> "$CAU_RUNLOG" 2>/dev/null
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
					cau_run_logged pacman -Sy --noconfirm --color never "${keyrings[@]}" || true
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
				break
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
