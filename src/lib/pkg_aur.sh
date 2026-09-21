# shellcheck shell=bash
#
# AUR packages.
#
# makepkg (and therefore paru and yay) refuse to run as root, so this is the
# one part of the run that cannot happen in the service's own context. It is
# executed as the locked "cachy-auto-update" system account instead, which
# sysusers.d creates with no password and no shell. That account is granted
# NOPASSWD access to /usr/bin/pacman through /etc/sudoers.d/cachy-auto-update,
# which is what lets the helper install what it built without a human present.
#
# The alternative (stashing the user's password somewhere the daemon can read
# it) buys nothing: whatever can decrypt it is exactly what an attacker would
# already have.

CAU_AUR_COUNT=0
CAU_AUR_HELPER=''

# Notify only after this many consecutive failed AUR runs. A single failed
# build is routine (upstream broke a tarball, a checksum moved) and self-heals
# a day later; nagging about it on someone's parents' machine is noise.
CAU_AUR_FAILURE_THRESHOLD=2

# cau_aur_detect
# Resolves the helper to use, honouring AURHelper from the config.
cau_aur_detect() {
	local candidate

	CAU_AUR_HELPER=''

	if [[ -n $CFG_AUR_HELPER && $CFG_AUR_HELPER != auto ]]; then
		if cau_have "$CFG_AUR_HELPER"; then
			CAU_AUR_HELPER="$CFG_AUR_HELPER"
			return 0
		fi
		cau_warn "Configured AUR helper '$CFG_AUR_HELPER' not found"
		return 1
	fi

	for candidate in paru yay pikaur; do
		if cau_have "$candidate"; then
			CAU_AUR_HELPER="$candidate"
			return 0
		fi
	done

	return 1
}

# cau_as_build_user <command> [args...]
# A deliberately small environment: the helper gets its own HOME and cache so
# nothing it downloads ever lands in a human's home directory.
cau_as_build_user() {
	runuser -u "$CAU_BUILD_USER" -- env \
		"HOME=$CAU_BUILD_HOME" \
		"USER=$CAU_BUILD_USER" \
		"LOGNAME=$CAU_BUILD_USER" \
		"XDG_CACHE_HOME=$CAU_CACHEDIR" \
		"XDG_CONFIG_HOME=$CAU_BUILD_HOME/.config" \
		"XDG_DATA_HOME=$CAU_BUILD_HOME/.local/share" \
		"PATH=/usr/local/sbin:/usr/local/bin:/usr/bin" \
		LC_ALL=C \
		"$@"
}

# cau_aur_ready
# True when everything the AUR path needs is actually in place.
cau_aur_ready() {
	cau_aur_detect || { cau_info "No AUR helper installed; skipping AUR updates"; return 1; }

	if ! getent passwd "$CAU_BUILD_USER" > /dev/null; then
		cau_warn "Build account '$CAU_BUILD_USER' is missing; skipping AUR updates"
		return 1
	fi

	# -l asks sudo whether the command is permitted; -n guarantees it can
	# never block on a password prompt. Testing `sudo -n true` instead would
	# fail by design, because the rule is scoped to pacman alone.
	if ! cau_as_build_user sudo -n -l /usr/bin/pacman > /dev/null 2>&1; then
		cau_warn "Build account cannot run pacman without a password; check /etc/sudoers.d/cachy-auto-update"
		return 1
	fi

	if ! cau_have makepkg; then
		cau_warn "makepkg not found (base-devel missing); skipping AUR updates"
		return 1
	fi

	return 0
}

# cau_aur_helper_args
# The flags that turn an interactive helper into a silent one.
cau_aur_helper_args() {
	case "$CAU_AUR_HELPER" in
		paru)
			printf '%s\n' -Sua --noconfirm --skipreview --removemake --cleanafter --color never
			[[ $CFG_DEVEL == yes ]] && printf '%s\n' --devel
			;;
		yay)
			printf '%s\n' -Sua --noconfirm --removemake --cleanafter --color never
			printf '%s\n' --answerclean All --answerdiff None --answeredit None --answerupgrade None
			[[ $CFG_DEVEL == yes ]] && printf '%s\n' --devel
			;;
		pikaur)
			printf '%s\n' -Sua --noconfirm --noedit
			;;
	esac
}

# cau_aur_pending
# Number of AUR packages with an update available.
cau_aur_pending() {
	local out
	out="$(cau_as_build_user "$CAU_AUR_HELPER" -Qua 2>/dev/null | grep -c .)" || out=0
	[[ $out =~ ^[0-9]+$ ]] || out=0
	printf '%s\n' "$out"
}

# cau_aur_update
# Returns 0 on success or "nothing to do", 1 on a failure worth reporting.
# Transient build failures are swallowed until they repeat.
cau_aur_update() {
	local pending failures
	local -a args

	# No helper, no base-devel, no bar: a step that cannot run should not be
	# holding a share of it.
	cau_aur_ready || { cau_progress_drop aur; return 0; }

	cau_progress_step aur "Updating AUR packages"

	pending="$(cau_aur_pending)"
	if (( pending == 0 )); then
		cau_info "No AUR updates pending"
		cau_state_clear aur_failures
		cau_progress_drop aur
		return 0
	fi

	cau_info "Updating $pending AUR package(s) with $CAU_AUR_HELPER"

	# The helper builds each package from source and prints plenty about it,
	# none of it countable from this side. A single large package can take ten
	# minutes, so the bar creeps through the step rather than sitting at its
	# start for all of them; the item count stays where it is, because that is
	# the number that would be lying if it moved.
	cau_progress_item 0 "$pending"
	mapfile -t args < <(cau_aur_helper_args)
	cau_progress_creep_start

	if cau_run_logged cau_as_build_user "$CAU_AUR_HELPER" "${args[@]}"; then
		cau_progress_creep_stop
		CAU_AUR_COUNT="$pending"
		cau_progress_item "$pending"
		cau_state_clear aur_failures
		return 0
	fi

	cau_progress_creep_stop
	CAU_AUR_COUNT=0
	failures="$(cau_state_read aur_failures 0)"
	[[ $failures =~ ^[0-9]+$ ]] || failures=0
	failures=$(( failures + 1 ))
	cau_state_write aur_failures "$failures"

	if (( failures >= CAU_AUR_FAILURE_THRESHOLD )); then
		cau_error "AUR update failed $failures times in a row"
		return 1
	fi

	cau_warn "AUR update failed (attempt $failures); will retry on the next run"
	return 0
}
