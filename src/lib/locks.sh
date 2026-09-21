# shellcheck shell=bash
#
# Staying out of the way of a human using pacman/yay/paru.
#
# The contract this file implements: the machine's owner may run any package
# manager at any time and must never see a lock error caused by us. We can only
# guarantee that in one direction: by never *starting* while somebody else is
# mid-transaction. So the checks here run before anything is touched, and a
# refusal simply defers the run to the next hourly tick.

# Package managers that take /var/lib/pacman/db.lck. checkupdates is absent on
# purpose: it works against a private temporary database and never touches the
# real lock.
CAU_PACKAGE_MANAGERS=(
	pacman pacman-static
	yay paru pikaur aurman trizen
	pamac pamac-daemon pamac-manager pamac-tray
	octopi octopi-helper
	makepkg
)

# cau_acquire_lock
# Guards against two runs overlapping (a slow run still going when the next
# tick fires). Non-blocking: if another run holds it, this one is pointless.
cau_acquire_lock() {
	mkdir -p "$CAU_RUNDIR" 2>/dev/null || return 1

	exec {CAU_LOCK_FD}> "$CAU_LOCKFILE" || return 1
	flock -n "$CAU_LOCK_FD" || return 1

	return 0
}

# cau_pacman_lock_holder
# Prints the pids currently holding pacman's database lock, if any.
cau_pacman_lock_holder() {
	[[ -e $CAU_PACMAN_LOCK ]] || return 1
	cau_have fuser || return 0     # cannot tell; assume it is held
	fuser "$CAU_PACMAN_LOCK" 2>/dev/null | tr -s ' ' '\n' | grep -E '^[0-9]+$' || return 0
}

# cau_package_manager_busy
# True when somebody else is using the package system right now. Sets
# CAU_SKIP_REASON.
cau_package_manager_busy() {
	local pm

	if [[ -e $CAU_PACMAN_LOCK ]]; then
		CAU_SKIP_REASON="$(cau_msg "pacman's database is locked by another process")"
		return 0
	fi

	for pm in "${CAU_PACKAGE_MANAGERS[@]}"; do
		if cau_process_running "$pm"; then
			CAU_SKIP_REASON="$(cau_msg "%s is currently running" "$pm")"
			return 0
		fi
	done

	return 1
}

# cau_pacman_lock_is_stale
# True only when the lock provably cannot belong to anything alive.
#
# The rigorous test is the boot time: no process that existed before the
# current boot can still be running, so a db.lck older than boot is abandoned
# by definition. That is exactly what a power cut during an update leaves
# behind. A lock that is merely unheld *within* this boot is not provable in
# the same way, so it is only reported (see cau_track_stale_lock) and never
# removed; guessing wrong there would corrupt a live transaction.
#
# The fuser check is kept as a second condition purely to survive a backwards
# clock jump making a live lock look pre-boot.
cau_pacman_lock_is_stale() {
	local boot lock

	[[ -e $CAU_PACMAN_LOCK ]] || return 1

	boot="$(awk '/^btime /{print $2}' /proc/stat 2>/dev/null)"
	[[ $boot =~ ^[0-9]+$ ]] || return 1

	lock="$(stat -c %Y "$CAU_PACMAN_LOCK" 2>/dev/null)" || return 1
	[[ $lock =~ ^[0-9]+$ ]] || return 1

	(( lock < boot )) || return 1
	[[ -z "$(cau_pacman_lock_holder)" ]]
}

# cau_recover_stale_lock
# Clears a provably abandoned lock so an interrupted update can be finished on
# the next run. Without this, one power cut during an update stops every future
# update permanently and silently. That is the worst possible outcome for a
# machine nobody is watching.
cau_recover_stale_lock() {
	cau_pacman_lock_is_stale || return 1

	cau_warn "Found a pacman lock older than this boot. An update was cut short"
	rm -f "$CAU_PACMAN_LOCK" 2>/dev/null || {
		cau_error "Could not remove the stale pacman lock"
		return 1
	}
	cau_state_clear stale_lock_count
	cau_info "Stale lock removed; the interrupted update will be finished now"
	return 0
}

# cau_track_stale_lock
# A db.lck with no process behind it but created during this boot: a crashed
# pacman rather than a power cut. Not provable, so it is counted, and after
# enough consecutive sightings the user is told to clean it up.
CAU_STALE_LOCK_RUNS=3

cau_track_stale_lock() {
	local count

	if [[ ! -e $CAU_PACMAN_LOCK ]]; then
		cau_state_clear stale_lock_count
		return 1
	fi

	if [[ -n "$(cau_pacman_lock_holder)" ]]; then
		# genuinely held; not stale
		cau_state_clear stale_lock_count
		return 1
	fi

	count="$(cau_state_read stale_lock_count 0)"
	[[ $count =~ ^[0-9]+$ ]] || count=0
	count=$(( count + 1 ))
	cau_state_write stale_lock_count "$count"

	(( count >= CAU_STALE_LOCK_RUNS ))
}

# cau_inhibit_prefix
# Builds the command prefix that keeps logind from suspending or shutting the
# machine down mid-transaction. Falls back to running unprotected rather than
# refusing to update at all.
cau_inhibit_prefix() {
	if cau_have systemd-inhibit; then
		printf '%s\n' systemd-inhibit
		printf '%s\n' --what=sleep:shutdown
		printf '%s\n' --mode=block
		printf '%s\n' "--who=$CAU_PRETTY"
		printf '%s\n' "--why=$(cau_msg_in C "applying system updates")"
		printf '%s\n' --
	fi
}
