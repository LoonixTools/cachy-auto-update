# shellcheck shell=bash
#
# AppImages, by way of Gear Lever.
#
# AppImages have no package manager of their own; Gear Lever is what tracks
# where each one came from and how to fetch a new build. Its CLI is a first
# class interface: `--update --all -y` is exactly the unattended entry point
# we need, and it skips AppImages whose application is currently running rather
# than pulling the file out from under it (we deliberately do not pass
# --force).
#
# This only runs for users with a live graphical session: Gear Lever is a
# Flatpak GTK application and needs the session's runtime directory. Nothing is
# lost by waiting, because the next hourly tick will catch it once they log in.

CAU_APPIMAGE_ID="it.mijorus.gearlever"
CAU_APPIMAGE_COUNT=0

# _cau_gearlever_cmd <user> <uid>
# Prints the argv prefix that invokes Gear Lever for that user, or nothing when
# it is not installed for them.
_cau_gearlever_cmd() {
	local user="$1" uid="$2"

	if cau_as_user "$user" "$uid" sh -c 'command -v gearlever >/dev/null 2>&1'; then
		printf '%s\n' gearlever
		return 0
	fi

	if cau_have flatpak && \
	   cau_as_user "$user" "$uid" flatpak info "$CAU_APPIMAGE_ID" > /dev/null 2>&1
	then
		printf '%s\n' flatpak run "--command=gearlever" "$CAU_APPIMAGE_ID"
		return 0
	fi

	return 1
}

# cau_appimage_update
# Never fatal: a headless GTK application is a best-effort proposition.
cau_appimage_update() {
	local user uid count rc=0
	local -a cmd

	# Is there a Gear Lever on this machine at all? Asked before the step is
	# announced rather than discovered inside the loop: on a machine without
	# one (the common case, it is an optional dependency), a step that exists
	# only to hand its share of the bar straight to the next one is a jump the
	# bar does not need. Stops at the first user who has it, so the extra probe
	# costs anything only in the case it is there to remove.
	local found=0
	while read -r user uid; do
		[[ -n $user ]] || continue
		if _cau_gearlever_cmd "$user" "$uid" > /dev/null; then
			found=1
			break
		fi
	done < <(cau_active_session_users)

	if (( ! found )); then
		cau_progress_drop appimage
		return 0
	fi

	cau_progress_step appimage "Updating AppImages"

	while read -r user uid; do
		[[ -n $user ]] || continue

		mapfile -t cmd < <(_cau_gearlever_cmd "$user" "$uid")
		(( ${#cmd[@]} )) || continue

		# "[Update available, <manager>]" is the per-app marker in the plain
		# text listing; the alternative is --json, which would drag in a JSON
		# parser for no gain.
		count="$(cau_as_user "$user" "$uid" timeout 300 "${cmd[@]}" --list-updates 2>/dev/null \
			| grep -c '\[Update available')" || count=0
		[[ $count =~ ^[0-9]+$ ]] || count=0

		if (( count == 0 )); then
			cau_info "No AppImage updates pending for $user"
			continue
		fi

		cau_info "Updating $count AppImage(s) for $user"
		if cau_run_logged cau_as_user "$user" "$uid" timeout 1800 \
			"${cmd[@]}" --update --all -y
		then
			CAU_APPIMAGE_COUNT=$(( CAU_APPIMAGE_COUNT + count ))
		else
			cau_warn "AppImage update failed for $user"
			rc=1
		fi
	done < <(cau_active_session_users)

	return $rc
}
