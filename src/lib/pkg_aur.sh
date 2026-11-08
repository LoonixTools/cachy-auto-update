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

# AUR packages that failed to build, space separated. They are held back, so
# one broken package never stops the others. Some can never build on their
# own: davinci-resolve wants a file that has to be downloaded by hand.
CAU_AUR_HELD=''

# A held package is tried again once the AUR has a new version of it, or
# after this long. That way a build that only failed because the network
# dropped does not stay held for months.
CAU_AUR_RETRY_AFTER=$(( 3 * 86400 ))

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

# _cau_aur_list
# Pending AUR updates, one "name newversion" per line. Packages that pacman.conf
# ignores are left out: the helper would skip them anyway.
_cau_aur_list() {
	cau_as_build_user "$CAU_AUR_HELPER" -Qua 2>/dev/null \
		| grep -v '\[ignored\]' \
		| awk '$1 ~ /^[A-Za-z0-9@._+-]+$/ && NF >= 2 { print $1, $NF }'
}

# cau_aur_pending
# Number of AUR packages with an update available.
cau_aur_pending() {
	_cau_aur_list | grep -c . || true
}

# _cau_aur_build [args...]
# One run of the helper. After a pause it runs again: packages installed before
# the pause are not pending any more.
_cau_aur_build() {
	while :; do
		cau_run_logged cau_work aur '' \
			cau_as_build_user "$CAU_AUR_HELPER" "$@" && return 0
		cau_control_stopped && cau_control_wait && continue
		return 1
	done
}

# _cau_aur_join <name>...
# The names as one comma separated list, the way --ignore takes them.
_cau_aur_join() {
	local IFS=,
	printf '%s' "$*"
}

# cau_aur_update
# Always returns 0. A package that fails to build is held back and the others
# are updated. CAU_AUR_HELD names the held ones.
cau_aur_update() {
	local name ver since now other
	local -a args=() todo=() left=() skip=()
	local -A want=() held=()

	CAU_AUR_HELD=''
	CAU_AUR_COUNT=0

	# Left over from before packages were held back.
	cau_state_clear aur_failures

	# No helper, no base-devel, no bar: a step that cannot run should not be
	# holding a share of it.
	cau_aur_ready || { cau_progress_drop aur; return 0; }

	cau_progress_step aur "Updating AUR packages"

	while read -r name ver; do
		want[$name]="$ver"
	done < <(_cau_aur_list)

	if (( ${#want[@]} == 0 )); then
		cau_info "No AUR updates pending"
		cau_state_clear aur_held
		cau_progress_drop aur
		return 0
	fi

	# Still held: the same version is pending and it is not time to try again.
	now="$(date +%s)"
	while read -r name ver since; do
		[[ -n $name && ${want[$name]-} == "$ver" && $since =~ ^[0-9]+$ ]] || continue
		(( now - since < CAU_AUR_RETRY_AFTER )) && held[$name]="$ver $since"
	done < <(cau_state_read aur_held '')

	for name in "${!want[@]}"; do
		[[ -n ${held[$name]-} ]] || todo+=("$name")
	done

	if (( ${#todo[@]} == 0 )); then
		cau_progress_drop aur
	else
		cau_info "Updating ${#todo[@]} AUR package(s) with $CAU_AUR_HELPER"

		# The helper builds each package from source and prints plenty about
		# it, none of it countable from this side. A single large package can
		# take ten minutes, so the bar creeps through the step rather than
		# sitting at its start for all of them; the item count stays where it
		# is, because that is the number that would be lying if it moved.
		cau_progress_item 0 "${#todo[@]}"
		mapfile -t args < <(cau_aur_helper_args)
		(( ${#held[@]} )) && skip=(--ignore "$(_cau_aur_join "${!held[@]}")")
		cau_progress_creep_start

		# All at once first. If that fails, each package that is still pending
		# gets a try on its own, so only the broken one is held back.
		if _cau_aur_build "${args[@]}" "${skip[@]}"; then
			CAU_AUR_COUNT=${#todo[@]}
		elif cau_control_stopped || cau_control_cancelled; then
			cau_info "AUR update was stopped from the desktop"
		else
			while read -r name ver; do
				[[ -n ${want[$name]-} && -z ${held[$name]-} ]] && left+=("$name")
			done < <(_cau_aur_list)
			CAU_AUR_COUNT=$(( ${#todo[@]} - ${#left[@]} ))

			for name in "${left[@]}"; do
				if (( ${#left[@]} > 1 )); then
					skip=("${!held[@]}")
					for other in "${left[@]}"; do
						[[ $other == "$name" || -n ${held[$other]-} ]] || skip+=("$other")
					done
					cau_info "Trying $name on its own"
					if _cau_aur_build "${args[@]}" --ignore "$(_cau_aur_join "${skip[@]}")"; then
						CAU_AUR_COUNT=$(( CAU_AUR_COUNT + 1 ))
						continue
					fi
				fi
				cau_control_cancelled && break
				cau_warn "Could not build $name; holding it back"
				held[$name]="${want[$name]} $now"
			done
		fi

		cau_progress_creep_stop
		cau_progress_item "$CAU_AUR_COUNT"
	fi

	if (( ${#held[@]} == 0 )); then
		cau_state_clear aur_held
		return 0
	fi

	cau_state_write aur_held "$(
		for name in "${!held[@]}"; do
			printf '%s %s\n' "$name" "${held[$name]}"
		done | sort
	)"
	CAU_AUR_HELD="$(printf '%s\n' "${!held[@]}" | sort | paste -sd ' ')"
	cau_info "Holding back AUR package(s): $CAU_AUR_HELD"
	return 0
}

# Where AUR helpers build, below a cache directory: one folder per package,
# each with a PKGBUILD in it.
CAU_AUR_CACHES=(paru/clone yay Shelly)

# Programs that build in those folders. While one runs, nothing is cleaned.
CAU_AUR_BUILDERS=(paru yay makepkg shelly shelly-ui)

# cau_aur_clean_caches
# Removes build leftovers from the AUR helpers' caches: built packages,
# downloaded sources, and the src/ and pkg/ build folders. PKGBUILDs, patches
# and git data stay.
#
# paru's CleanAfter does the same for paru, but only where somebody turned it
# on. Without it every version of every -bin package stays: 32 GB of
# ~/.cache/paru on one laptop.
cau_aur_clean_caches() {
	local user uid home base p freed total=0

	[[ $CFG_CLEAN_AUR_CACHE == yes ]] || return 0

	for p in "${CAU_AUR_BUILDERS[@]}"; do
		if cau_process_running "$p"; then
			cau_info "Not cleaning the AUR caches while $p is running"
			return 0
		fi
	done

	while read -r user base; do
		[[ -d $base ]] || continue
		for p in "${CAU_AUR_CACHES[@]}"; do
			[[ -d $base/$p ]] || continue
			freed="$(_cau_aur_clean_cache "$user" "$base/$p")"
			[[ $freed =~ ^[0-9]+$ ]] && total=$(( total + freed ))
		done
	done < <(
		printf '%s %s\n' "$CAU_BUILD_USER" "$CAU_CACHEDIR"
		cau_human_users | while read -r user uid home; do
			printf '%s %s\n' "$user" "$home/.cache"
		done
	)

	(( total >= 1048576 )) || return 0
	cau_info "Cleaned the AUR caches: $(( total / 1048576 )) MiB freed"
}

# _cau_aur_clean_cache <user> <dir>
# Prints how many bytes it freed. Runs as the owner of the cache, so a symlink
# in there can never make root delete something else.
#
# Only files that a build made are removed: built packages, anything over 1 MiB
# (the AUR takes no file that large, so those were downloaded), and the build
# folders. Version control checkouts of -git sources stay, so the next build
# only fetches what is new.
_cau_aur_clean_cache() {
	runuser -u "$1" -- sh -c '
		before=$(du -sb "$1" 2>/dev/null | cut -f1)
		for d in "$1"/*/; do
			[ -L "${d%/}" ] && continue
			[ -f "${d}PKGBUILD" ] || continue
			rm -rf -- "${d}src" "${d}pkg"
			find "$d" -mindepth 1 -maxdepth 1 -type f \
				\( -name "*.pkg.tar*" -o -size +1M \) -delete
		done
		after=$(du -sb "$1" 2>/dev/null | cut -f1)
		echo $(( ${before:-0} - ${after:-0} ))
	' sh "$2" 2>/dev/null
}
