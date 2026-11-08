# shellcheck shell=bash
#
# Keeping other update notices quiet.
#
# Once updates install themselves, two things CachyOS ships only get in the
# way:
#
# - cachy-update: a tray icon and a daily check that say "N updates
#   available". This tool has its own tray icon, so that is the same thing
#   twice, and it asks for something that happens by itself anyway.
# - The "Reboot recommended" notice of cachyos-hooks. It pops up the moment a
#   kernel or driver is unpacked. During an unattended update that is in the
#   middle of the run, with AUR packages and Flatpaks still ahead, and reads as
#   an invitation to cut the update in half.
#
# Both are off while automatic updates are on, unless CachyUpdate or
# RebootNotification turns them back on. Switching automatic updates off brings
# them back, because then they are useful again. Each is undone the standard
# way, so nothing of the other packages is touched:
#
# - The cachy-update timer is a user unit: disabled per user (and globally if
#   it was enabled that way), enabled again per user.
# - Its tray starts from /etc/xdg/autostart. A file of the same name in the
#   user's own autostart folder with Hidden=true hides it for that user.
# - pacman takes a hook in /etc/pacman.d/hooks over one of the same name in
#   /usr/share/libalpm/hooks, and a link to /dev/null there turns it off.

CAU_CU_TIMER="arch-update.timer"
CAU_CU_UNIT="/usr/lib/systemd/user/$CAU_CU_TIMER"
CAU_CU_GLOBAL="/etc/systemd/user/timers.target.wants/$CAU_CU_TIMER"
CAU_CU_TRAY="arch-update-tray.desktop"
CAU_CU_TRAY_SYSTEM="/etc/xdg/autostart/$CAU_CU_TRAY"
CAU_CU_MARK="# Hidden by cachy-auto-update. Turn on \"Also run cachy-update\" to undo."

CAU_REBOOT_HOOK="cachyos-reboot-required.hook"
CAU_REBOOT_HOOK_SYSTEM="/usr/share/libalpm/hooks/$CAU_REBOOT_HOOK"
CAU_REBOOT_HOOK_OVERRIDE="/etc/pacman.d/hooks/$CAU_REBOOT_HOOK"

# cau_quiet_apply [--only-off]
# Makes the machine match the settings. As root only. --only-off is for the
# hourly runner: it only switches things off and costs nothing more than a
# few file checks when there is nothing to do. Switching back on happens where
# a person changed a setting.
cau_quiet_apply() {
	local only_off="${1:-}"

	cau_is_root || return 0

	if [[ $CFG_ENABLED == yes && $CFG_CACHY_UPDATE == no ]]; then
		_cau_cachy_update_off
	elif [[ -z $only_off ]]; then
		_cau_cachy_update_on
	fi

	if [[ $CFG_ENABLED == yes && $CFG_REBOOT_NOTIFY == no ]]; then
		_cau_reboot_hook_off
	elif [[ -z $only_off ]]; then
		_cau_reboot_hook_on
	fi
	return 0
}

_cau_cachy_update_off() {
	local user uid home file global=0

	[[ -e $CAU_CU_UNIT || -e $CAU_CU_TRAY_SYSTEM ]] || return 0

	if [[ -L $CAU_CU_GLOBAL ]]; then
		systemctl --global disable "$CAU_CU_TIMER" > /dev/null 2>&1 || true
		cau_info "Turned off cachy-update's update check for all users"
		global=1
	fi

	while read -r user uid home; do
		# The timer, wherever it is enabled for this user.
		if [[ -L $home/.config/systemd/user/timers.target.wants/$CAU_CU_TIMER ]]; then
			if [[ -S /run/user/$uid/bus ]]; then
				cau_as_user "$user" "$uid" systemctl --user disable --now "$CAU_CU_TIMER" > /dev/null 2>&1 || true
			else
				runuser -u "$user" -- rm -f "$home/.config/systemd/user/timers.target.wants/$CAU_CU_TIMER" 2>/dev/null || true
			fi
			cau_info "Turned off cachy-update's update check for $user"
		elif (( global )) && [[ -S /run/user/$uid/bus ]]; then
			# Was enabled for everyone until a moment ago: stop the running one.
			cau_as_user "$user" "$uid" systemctl --user stop "$CAU_CU_TIMER" > /dev/null 2>&1 || true
		fi

		# The tray icon. A file the user made themselves is left alone.
		[[ -e $CAU_CU_TRAY_SYSTEM ]] || continue
		file="$home/.config/autostart/$CAU_CU_TRAY"
		[[ -e $file ]] && continue
		[[ -d $home ]] || continue
		runuser -u "$user" -- sh -c 'mkdir -p "${1%/*}" && printf "%s\n" "$2" "[Desktop Entry]" "Hidden=true" > "$1"' \
			sh "$file" "$CAU_CU_MARK" 2>/dev/null || continue
		pkill -u "$uid" -f 'arch-update.*--tray|arch-update-tray' 2>/dev/null || true
		cau_info "Hid cachy-update's tray icon for $user"
	done < <(cau_human_users)
}

_cau_cachy_update_on() {
	local user uid home file

	while read -r user uid home; do
		file="$home/.config/autostart/$CAU_CU_TRAY"
		if [[ -f $file ]] && grep -qxF "$CAU_CU_MARK" "$file" 2>/dev/null; then
			runuser -u "$user" -- rm -f "$file" 2>/dev/null || true
		fi
	done < <(cau_human_users)

	[[ -e $CAU_CU_UNIT ]] || return 0
	while read -r user uid; do
		[[ -n $user ]] || continue
		cau_as_user "$user" "$uid" systemctl --user enable --now "$CAU_CU_TIMER" > /dev/null 2>&1 || true
		[[ -e $CAU_CU_TRAY_SYSTEM ]] || continue
		pgrep -u "$uid" -f 'arch-update.*--tray|arch-update-tray' > /dev/null 2>&1 && continue
		cau_as_user "$user" "$uid" systemd-run --user --quiet --collect \
			--unit=arch-update-tray arch-update --tray < /dev/null > /dev/null 2>&1 || true
	done < <(cau_active_session_users)
	return 0
}

_cau_reboot_hook_off() {
	[[ -f $CAU_REBOOT_HOOK_SYSTEM ]] || return 0
	[[ -e $CAU_REBOOT_HOOK_OVERRIDE || -L $CAU_REBOOT_HOOK_OVERRIDE ]] && return 0
	mkdir -p "${CAU_REBOOT_HOOK_OVERRIDE%/*}" 2>/dev/null \
		&& ln -s /dev/null "$CAU_REBOOT_HOOK_OVERRIDE" 2>/dev/null \
		&& cau_info "Turned off CachyOS's restart notice"
	return 0
}

_cau_reboot_hook_on() {
	# Only the link this tool makes. A real file there is somebody's own hook.
	if [[ -L $CAU_REBOOT_HOOK_OVERRIDE && $(readlink "$CAU_REBOOT_HOOK_OVERRIDE") == /dev/null ]]; then
		rm -f "$CAU_REBOOT_HOOK_OVERRIDE" 2>/dev/null || true
	fi
	return 0
}
