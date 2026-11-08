# shellcheck shell=bash
#
# Flatpak.
#
# System-wide installations are updated directly as root. That side-steps a
# real obstacle: the shipped polkit rule for Flatpak only grants install and
# uninstall, and only to a subject that is active, local and in the wheel
# group. None of that is true for an unattended service. Being root means
# polkit is never consulted in the first place.
#
# Per-user installations live in the user's home and are updated inside their
# own account.

CAU_FLATPAK_COUNT=0

# cau_flatpak_pending_system
cau_flatpak_pending_system() {
	local out
	out="$(flatpak remote-ls --system --updates --columns=application 2>/dev/null | grep -c .)" || out=0
	[[ $out =~ ^[0-9]+$ ]] || out=0
	printf '%s\n' "$out"
}

# _cau_flatpak_run <command> [args...]
# A Flatpak command that Cancel and Pause can stop. After a pause it runs
# again, and ostree keeps what it had already fetched.
_cau_flatpak_run() {
	while :; do
		cau_run_logged cau_work flatpak '' "$@" && return 0
		cau_control_stopped && cau_control_wait && continue
		return 1
	done
}

# _cau_flatpak_repair <command> [args...]
# After a failed update: fix the installation, then the caller tries once more.
# A half pulled or broken object in the repo is the usual cause, and
# `flatpak repair` is the fix for it. Fails only when there is no point in
# trying again.
_cau_flatpak_repair() {
	cau_control_cancelled && return 1
	cau_info "Repairing the Flatpak installation and retrying"
	cau_run_logged cau_work flatpak '' "$@" || true
	! cau_control_cancelled
}

# cau_flatpak_update
# Failures are reported but never abort the rest of the run.
cau_flatpak_update() {
	local rc=0 pending user uid home count

	cau_have flatpak || { cau_progress_drop flatpak; return 0; }

	cau_progress_step flatpak "Updating Flatpak apps"

	# refresh appstream metadata first so remote-ls sees current versions.
	# Nothing is countable until that has finished, so the bar creeps rather
	# than waiting at the start of the step for it.
	cau_progress_creep_start
	_cau_flatpak_run flatpak update --appstream --system --noninteractive || true
	cau_progress_creep_stop
	cau_control_cancelled && return 0

	pending="$(cau_flatpak_pending_system)"
	if (( pending > 0 )); then
		cau_info "Updating $pending system Flatpak(s)"
		cau_progress_item 0 "$pending"
		# Started after the count above and stopped before the one below, so the
		# only reports this side makes while a ticker is running are further
		# along than the ticker ever gets.
		cau_progress_creep_start
		if _cau_flatpak_run flatpak update --system --noninteractive --assumeyes \
			|| { _cau_flatpak_repair flatpak repair --system \
				&& _cau_flatpak_run flatpak update --system --noninteractive --assumeyes; }
		then
			cau_progress_creep_stop
			CAU_FLATPAK_COUNT=$(( CAU_FLATPAK_COUNT + pending ))
			cau_progress_item "$pending"
		elif cau_control_cancelled; then
			cau_progress_creep_stop
			return 0
		else
			cau_progress_creep_stop
			cau_warn "System Flatpak update failed"
			rc=1
		fi
	else
		cau_info "No system Flatpak updates pending"
	fi

	while read -r user uid home; do
		cau_control_cancelled && return 0
		[[ -d "$home/.local/share/flatpak" ]] || continue

		count="$(cau_as_user "$user" "$uid" flatpak remote-ls --user --updates \
			--columns=application 2>/dev/null | grep -c .)" || count=0
		[[ $count =~ ^[0-9]+$ ]] || count=0
		(( count > 0 )) || continue

		cau_info "Updating $count user Flatpak(s) for $user"
		if _cau_flatpak_run cau_as_user "$user" "$uid" \
			flatpak update --user --noninteractive --assumeyes \
			|| { _cau_flatpak_repair cau_as_user "$user" "$uid" flatpak repair --user \
				&& _cau_flatpak_run cau_as_user "$user" "$uid" \
					flatpak update --user --noninteractive --assumeyes; }
		then
			CAU_FLATPAK_COUNT=$(( CAU_FLATPAK_COUNT + count ))
		elif cau_control_cancelled; then
			return 0
		else
			cau_warn "User Flatpak update failed for $user"
			rc=1
		fi
	done < <(cau_human_users)

	# Unused runtimes are the Flatpak equivalent of orphaned packages. This is
	# removal of installed software, not cache trimming, so it belongs behind
	# RemoveOrphans rather than CleanCache.
	if [[ $CFG_REMOVE_ORPHANS == yes ]] && ! cau_control_cancelled; then
		cau_run_logged flatpak uninstall --system --unused --noninteractive --assumeyes || true
	fi

	return $rc
}
