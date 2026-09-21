# shellcheck shell=bash
#
# The interactive front end.
#
# This is the part the machine's owner actually sees, so it stays deliberately
# small: two switches, a status block, and a way to look at what happened.

CAU_UNIT="cachy-auto-update.timer"

# Terminal mode.
#
# bash flips the terminal into non-canonical mode for each `read -sn1` and back
# out again in between. That gap matters: in canonical mode DEL is the ERASE
# character, so the line discipline eats it instead of delivering it, and a
# backspace typed while the interface was between reads simply vanished.
# Holding non-canonical mode for the whole interface removes the gap.
CAU_TERM_SAVED=''

cau_ui_term_raw() {
	cau_have stty || return 0
	[[ -t 0 ]] || return 0
	[[ -n $CAU_TERM_SAVED ]] && return 0

	CAU_TERM_SAVED="$(stty -g 2>/dev/null)" || { CAU_TERM_SAVED=''; return 0; }
	stty -icanon -echo min 1 time 0 2>/dev/null || true
}

cau_ui_term_restore() {
	[[ -n $CAU_TERM_SAVED ]] || return 0
	stty "$CAU_TERM_SAVED" 2>/dev/null || true
	CAU_TERM_SAVED=''
}

# Runs an action with the terminal handed back to normal line mode, so anything
# it prints or prompts for behaves the way a program expects.
cau_ui_cooked() {
	cau_ui_term_restore
	"$@"
	local rc=$?
	cau_ui_term_raw
	return $rc
}

# cau_read_key
# One keypress, resolved to a symbolic name: a literal character, or one of
# up/down/left/right/enter/space/escape. Arrow keys arrive as ESC [ A, so the
# tail of the sequence is consumed here rather than being mistaken for three
# separate presses.
cau_read_key() {
	local k rest

	IFS= read -rsn1 k || return 1

	case "$k" in
		$'\e')
			if IFS= read -rsn2 -t 0.05 rest; then
				case "$rest" in
					'[A') printf 'up\n' ;;
					'[B') printf 'down\n' ;;
					'[C') printf 'right\n' ;;
					'[D') printf 'left\n' ;;
					*)    printf 'escape\n' ;;
				esac
			else
				printf 'escape\n'
			fi
			;;
		# Enter is an empty read in cooked mode and a carriage return in raw
		# mode, depending on whether the terminal is translating it.
		''|$'\r')      printf 'enter\n' ;;
		$'\x7f'|$'\b') printf 'backspace\n' ;;
		' ')           printf 'space\n' ;;
		*)             printf '%s\n' "$k" ;;
	esac
}

# cau_ui_read_line <initial>
# A minimal line editor built on cau_read_key, with the result in
# CAU_LINE_RESULT.
#
# This exists instead of bash's own `read -r` because mixing line mode into a
# single-key interface breaks it: after one cooked-mode read the following
# `read -sn1` stops receiving keystrokes entirely, reproducibly, on a real pty.
# Never leaving single-character mode side-steps that completely.
CAU_LINE_RESULT=''

cau_ui_read_line() {
	local buf="${1:-}" key

	CAU_LINE_RESULT=''
	printf '%s' "$buf"

	while true; do
		key="$(cau_read_key)" || { printf '\n'; return 1; }

		case "$key" in
			enter)
				printf '\n'
				CAU_LINE_RESULT="$buf"
				return 0
				;;
			escape)
				printf '\n'
				return 1
				;;
			backspace)
				if [[ -n $buf ]]; then
					buf="${buf%?}"
					printf '\b \b'
				fi
				;;
			space)
				buf+=' '
				printf ' '
				;;
			up|down|left|right) ;;   # no cursor movement in this editor
			*)
				# a single printable character; control keys arrive as names
				[[ ${#key} -eq 1 ]] || continue
				buf+="$key"
				printf '%s' "$key"
				;;
		esac
	done
}

# Everything that can be changed without opening a text editor.
# Format: Key|type|default|label-msgid
# type is bool, choice:<space separated values>, or text.
CAU_SETTINGS=(
	"NotifyOnStart|bool|yes|Notify when an update starts"
	"NotifyOnSuccess|bool|yes|Notify after a successful update"
	"NotifyOnError|bool|yes|Notify when something goes wrong"
	"UpdateInterval|choice:6h 12h 1d 2d 1w|1d|Time between update runs"
	"SkipWhenGaming|bool|yes|Postpone while a game is running"
	"RequireAC|bool|no|Only update on mains power"
	"MinBatteryPercent|choice:0 20 30 40 50 60 70 80|30|Minimum battery level (%)"
	"UpdateAUR|bool|yes|Update AUR packages"
	"UpdateFlatpak|bool|yes|Update Flatpaks"
	"UpdateAppImages|bool|yes|Update AppImages"
	"UpdateDevel|bool|no|Also rebuild -git packages"
	"AURHelper|choice:auto paru yay pikaur|auto|AUR helper"
	"AutoResolveConflicts|bool|yes|Resolve package conflicts automatically"
	"CleanCache|bool|yes|Trim the package cache"
	"KeepOldPackages|choice:0 1 2 3 5|3|Cached versions to keep"
	"RemoveOrphans|bool|no|Remove packages nothing needs any more"
	"IgnorePkg|text||Never update these packages"
)

_cau_is_true() {
	case "${1,,}" in
		yes|y|true|1|on|enabled) return 0 ;;
		*) return 1 ;;
	esac
}

# _cau_setting_display <type> <value>
# Result in CAU_SETTING_SHOWN. The three constant strings are resolved once by
# the caller into CAU_LBL_*; looking them up here would put a translation call
# on the per-line path.
CAU_SETTING_SHOWN=''
CAU_LBL_ON=''
CAU_LBL_OFF=''
CAU_LBL_NONE=''

_cau_setting_display() {
	local type="$1" value="$2"

	case "$type" in
		bool)
			if _cau_is_true "$value"; then
				CAU_SETTING_SHOWN="${CAU_C_GREEN}${CAU_LBL_ON}${CAU_C_RESET}"
			else
				CAU_SETTING_SHOWN="${CAU_C_DIM}${CAU_LBL_OFF}${CAU_C_RESET}"
			fi
			;;
		text)
			if [[ -n $value ]]; then
				CAU_SETTING_SHOWN="$value"
			else
				CAU_SETTING_SHOWN="${CAU_C_DIM}${CAU_LBL_NONE}${CAU_C_RESET}"
			fi
			;;
		*) CAU_SETTING_SHOWN="$value" ;;
	esac
}

# _cau_setting_cycle <type> <value> <direction: 1|-1>
# The next value for this setting. Choices wrap around, so one key is enough to
# reach everything without needing a second one for the other direction.
_cau_setting_cycle() {
	local type="$1" value="$2" dir="$3"

	if [[ $type == bool ]]; then
		_cau_is_true "$value" && printf 'no\n' || printf 'yes\n'
		return
	fi

	local -a choices
	read -r -a choices <<< "${type#choice:}"
	(( ${#choices[@]} )) || { printf '%s\n' "$value"; return; }

	local i idx=0
	for i in "${!choices[@]}"; do
		[[ ${choices[i]} == "$value" ]] && { idx=$i; break; }
	done

	idx=$(( (idx + dir + ${#choices[@]}) % ${#choices[@]} ))
	printf '%s\n' "${choices[idx]}"
}

# cau_ui_settings
# A cursor list rather than a numbered menu: there are eighteen settings, and
# numbering them would run out of digits and force paging.
#
# The frame is assembled in memory and written once. Everything constant (the
# specs, the translated labels, the clear sequence) is resolved before the
# loop, and the values are re-read only after something actually changes.
# Drawing the naive way cost a command substitution per label per frame, which
# measured 435 ms per keypress: arrow keys felt like the console was reloading,
# because in effect it was.
cau_ui_settings() {
	local count=${#CAU_SETTINGS[@]}
	local -a names=() types=() defaults=() labels=()
	local -a values=()
	local spec name type default label locale i key frame row pad dirty=1 cursor=0

	locale="$(cau_ui_locale)"
	cau_msg_into "$locale" "ON";     CAU_LBL_ON="$CAU_MSG_RESULT"
	cau_msg_into "$locale" "OFF";    CAU_LBL_OFF="$CAU_MSG_RESULT"
	cau_msg_into "$locale" "(none)"; CAU_LBL_NONE="$CAU_MSG_RESULT"

	for spec in "${CAU_SETTINGS[@]}"; do
		IFS='|' read -r name type default label <<< "$spec"
		names+=("$name"); types+=("$type"); defaults+=("$default")
		cau_msg_into "$locale" "$label"
		labels+=("$CAU_MSG_RESULT")
	done

	local title hint
	cau_msg_into "$locale" "Settings"; title="$CAU_MSG_RESULT"
	cau_msg_into "$locale" "Up/Down: select, Space or Right: change, q: back"
	hint="$CAU_MSG_RESULT"

	# the terminfo clear string, fetched once instead of forking per frame
	local clearseq
	clearseq="$(clear 2>/dev/null)" || clearseq=$'\033[H\033[2J'

	while true; do
		if (( dirty )); then
			for i in "${!names[@]}"; do
				_cau_config_lookup "${names[i]}" "${defaults[i]}"
				values[i]="$CAU_CONFIG_VALUE"
			done
			dirty=0
		fi

		frame="$clearseq"$'\n'"${CAU_C_BOLD}${CAU_C_BLUE}  ${title}${CAU_C_RESET}"$'\n\n'

		local marker selected="${CAU_C_BLUE}▸${CAU_C_RESET} "
		for i in "${!names[@]}"; do
			_cau_setting_display "${types[i]}" "${values[i]}"
			pad=$(( 42 - ${#labels[i]} ))
			(( pad < 0 )) && pad=0
			if (( i == cursor )); then marker="$selected"; else marker='  '; fi
			printf -v row '  %s%s%*s %s' \
				"$marker" "${labels[i]}" "$pad" '' "$CAU_SETTING_SHOWN"
			frame+="$row"$'\n'
		done

		frame+=$'\n'"  ${CAU_C_DIM}${hint}${CAU_C_RESET}"$'\n'
		printf '%s' "$frame"

		key="$(cau_read_key)" || return 0

		type="${types[cursor]}"
		name="${names[cursor]}"

		case "$key" in
			up|k)    cursor=$(( (cursor - 1 + count) % count )) ;;
			down|j)  cursor=$(( (cursor + 1) % count )) ;;
			space|enter|right|l)
				if [[ $type == text ]]; then
					cau_ui_edit_text "$name" "${values[cursor]}"
				else
					cau_config_set "$name" "$(_cau_setting_cycle "$type" "${values[cursor]}" 1)" \
						|| { cau_bad "$(cau_msg "Could not write the configuration file.")"; cau_pause; }
				fi
				dirty=1
				;;
			left|h)
				if [[ $type != text ]]; then
					cau_config_set "$name" "$(_cau_setting_cycle "$type" "${values[cursor]}" -1)" \
						|| { cau_bad "$(cau_msg "Could not write the configuration file.")"; cau_pause; }
					dirty=1
				fi
				;;
			q|Q|escape) return 0 ;;
			*) ;;
		esac
	done
}

# cau_ui_edit_text <key> <current>
# The one setting that is a free-text list rather than a choice.
cau_ui_edit_text() {
	local name="$1" current="$2"

	printf '\n  %s\n' "$(cau_msg "Package names separated by spaces, empty to clear:")"
	printf '  > '

	# pre-filled with the current value so it can be corrected rather than
	# retyped; Escape leaves it unchanged
	if cau_ui_read_line "$current"; then
		cau_config_set "$name" "$CAU_LINE_RESULT" \
			|| { cau_bad "$(cau_msg "Could not write the configuration file.")"; cau_pause; }
	fi
}

# _cau_row <label> <value>
# printf's %-28s pads by bytes, so a label containing "ü" comes out one column
# short. ${#s} counts characters in a UTF-8 locale, so the padding is computed
# here instead. It is applied inline, because command substitution would eat
# the trailing spaces again.
_cau_row() {
	local label="$1" value="$2" pad
	pad=$(( 28 - ${#label} ))
	(( pad < 0 )) && pad=0
	printf '  %s%*s %s\n' "$label" "$pad" '' "$value"
}

_cau_onoff() {
	if [[ $1 == yes ]]; then
		printf '%s%s%s' "$CAU_C_GREEN" "$(cau_msg "ON")" "$CAU_C_RESET"
	else
		printf '%s%s%s' "$CAU_C_DIM" "$(cau_msg "OFF")" "$CAU_C_RESET"
	fi
}

# cau_timer_next
# When systemd will fire the timer next, in the user's locale.
cau_timer_next() {
	local raw

	cau_have systemctl || { cau_msg "unknown"; return; }
	systemctl is-enabled --quiet "$CAU_UNIT" 2>/dev/null || { cau_msg "not scheduled"; return; }

	raw="$(systemctl show "$CAU_UNIT" --property=NextElapseUSecRealtime --value 2>/dev/null)"

	if [[ $raw =~ ^[0-9]+$ ]] && (( raw > 0 )); then
		date -d "@$(( raw / 1000000 ))" '+%c'
	elif [[ -n $raw && $raw != 0 && $raw != n/a ]]; then
		printf '%s' "$raw"
	else
		cau_msg "unknown"
	fi
}

# cau_ui_status
# Shared by the `status` subcommand and the menu header.
cau_ui_status() {
	local last_run last_success result counts reboot
	local pac aur fp ai total

	last_run="$(cau_state_read last_run '')"
	last_success="$(cau_state_read last_success '')"
	result="$(cau_state_read last_result '')"
	counts="$(cau_state_read last_counts '0 0 0 0')"
	reboot="$(cau_state_read reboot_needed 0)"

	read -r pac aur fp ai <<< "$counts"
	pac=${pac:-0}; aur=${aur:-0}; fp=${fp:-0}; ai=${ai:-0}
	total=$(( pac + aur + fp + ai ))

	_cau_row "$(cau_msg "Automatic updates")" "$(_cau_onoff "$CFG_ENABLED")"
	_cau_row "$(cau_msg "Notifications")"     "$(_cau_onoff "$CFG_NOTIFICATIONS")"
	printf '\n'

	_cau_row "$(cau_msg "Last check")" "$(cau_time_ago "$last_run")"

	if [[ -n $last_success ]]; then
		_cau_row "$(cau_msg "Last successful update")" \
			"$(date -d "@$last_success" '+%c' 2>/dev/null || printf '%s' "$last_success") ($(cau_msg "%d packages" "$total"))"
	else
		_cau_row "$(cau_msg "Last successful update")" "$(cau_msg "never")"
	fi

	_cau_row "$(cau_msg "Next scheduled run")" "$(cau_timer_next)"

	if [[ $result == failed ]]; then
		printf '\n  %s%s%s\n' "$CAU_C_YELLOW" \
			"$(cau_msg "The last run reported a problem. See 'cachy-auto-update log'.")" \
			"$CAU_C_RESET"
	elif [[ $result == interrupted ]]; then
		printf '\n  %s%s%s\n' "$CAU_C_YELLOW" \
			"$(cau_msg "The last run was stopped before it finished.")" \
			"$CAU_C_RESET"
	fi
	if [[ $reboot == 1 ]]; then
		printf '\n  %s%s%s\n' "$CAU_C_YELLOW" \
			"$(cau_msg "A restart is recommended to finish a kernel update.")" \
			"$CAU_C_RESET"
	fi

	local held
	held="$(cau_state_read held_back '')"
	if [[ -n $held ]]; then
		printf '\n  %s%s%s\n' "$CAU_C_YELLOW" \
			"$(cau_msg "Held back: %s" "$held")" "$CAU_C_RESET"
	fi
}

# cau_ui_status_conditions
# Evaluates every gate individually. Without this there is no way to explain
# why an enabled updater has not done anything on somebody else's machine.
cau_ui_status_conditions() {
	local pct

	printf '\n  %s\n' "$(cau_msg "Current conditions:")"

	if cau_on_ac; then
		printf '    %s %s\n' "${CAU_C_GREEN}✔${CAU_C_RESET}" "$(cau_msg "on AC power")"
	else
		pct="$(cau_battery_percent || true)"
		if [[ -n $pct ]]; then
			printf '    %s %s\n' "${CAU_C_DIM}•${CAU_C_RESET}" \
				"$(cau_msg "on battery (%d%%, threshold %d%%)" "$pct" "$CFG_MIN_BATTERY")"
		else
			printf '    %s %s\n' "${CAU_C_DIM}•${CAU_C_RESET}" "$(cau_msg "on battery")"
		fi
	fi

	CAU_SKIP_REASON=''
	if cau_busy; then
		printf '    %s %s\n' "${CAU_C_YELLOW}•${CAU_C_RESET}" "$CAU_SKIP_REASON"
	else
		printf '    %s %s\n' "${CAU_C_GREEN}✔${CAU_C_RESET}" "$(cau_msg "no game detected")"
	fi

	CAU_SKIP_REASON=''
	if cau_package_manager_busy; then
		printf '    %s %s\n' "${CAU_C_YELLOW}•${CAU_C_RESET}" "$CAU_SKIP_REASON"
	else
		printf '    %s %s\n' "${CAU_C_GREEN}✔${CAU_C_RESET}" "$(cau_msg "package system is free")"
	fi
}

# cau_ui_menu
# The default view. Reloads the config after every action so the toggles always
# reflect what is actually on disk.
cau_ui_menu() {
	local choice

	cau_ui_term_raw
	# restore the terminal even if this exits through Ctrl-C or an error
	trap 'cau_ui_term_restore' EXIT INT TERM

	while true; do
		cau_config_load

		clear 2>/dev/null || true
		cau_head "  $CAU_PRETTY"
		cau_ui_status
		printf '\n'
		printf '  [1] %s\n' "$(cau_msg "Toggle automatic updates")"
		printf '  [2] %s\n' "$(cau_msg "Toggle notifications")"
		printf '  [3] %s\n' "$(cau_msg "Update now")"
		printf '  [4] %s\n' "$(cau_msg "Show log")"
		printf '  [5] %s\n' "$(cau_msg "Show current conditions")"
		printf '  [6] %s\n' "$(cau_msg "Settings")"
		printf '  [q] %s\n' "$(cau_msg "Quit")"
		printf '\n  > '

		# One keypress, no Enter. A failing read means EOF (Ctrl-D, or a
		# script piping input), which quits.
		choice="$(cau_read_key)" || {
			printf '\n'; cau_ui_term_restore; trap - EXIT INT TERM; return 0
		}
		case "$choice" in
			enter|space|up|down|left|right|escape) choice='' ;;
		esac
		printf '%s\n' "$choice"

		# Actions run in normal line mode: they print program output and some
		# of them prompt, neither of which behaves in raw mode.
		case "$choice" in
			# The two switches flip and return straight to the menu, where the
			# status block shows the result. Only a warning or an error holds
			# the screen (see CAU_UI_NEEDS_ACK).
			1)
				CAU_UI_NEEDS_ACK=''
				if [[ $CFG_ENABLED == yes ]]; then
					cau_ui_cooked cau_do_disable
				else
					cau_ui_cooked cau_do_enable
				fi
				if [[ -n $CAU_UI_NEEDS_ACK ]]; then cau_pause; fi
				;;
			2)
				CAU_UI_NEEDS_ACK=''
				if [[ $CFG_NOTIFICATIONS == yes ]]; then
					cau_ui_cooked cau_do_notifications off
				else
					cau_ui_cooked cau_do_notifications on
				fi
				if [[ -n $CAU_UI_NEEDS_ACK ]]; then cau_pause; fi
				;;
			3) cau_ui_cooked cau_do_run --force; cau_pause ;;
			4) cau_ui_cooked cau_do_log; cau_pause ;;
			5) cau_ui_status_conditions; cau_pause ;;
			6) cau_ui_settings ;;
			q|Q) cau_ui_term_restore; trap - EXIT INT TERM; return 0 ;;
			# Anything else (Enter, arrow keys, stray characters) just
			# redraws. Escape is deliberately not a quit key, so a mistyped
			# arrow key cannot close the menu.
			*) ;;
		esac
	done
}

cau_pause() {
	printf '\n  %s' "$(cau_msg "Press any key to continue...")"
	read -rsn1 _ || true
	printf '\n'
}
