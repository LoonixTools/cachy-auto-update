# shellcheck shell=bash
#
# Should we update right now?
#
# Every check here is allowed to say "not now" and nothing more. The timer
# ticks hourly, so a deferral costs nothing: the next tick simply tries again.
# That is also why none of these functions ever return a hard failure.

# Set by the checks below to a human-readable, already translated reason.
CAU_SKIP_REASON=''

# ---------------------------------------------------------------------------
# Power
# ---------------------------------------------------------------------------

# cau_on_ac
# True when running on mains power. systemd-ac-power also returns success when
# the machine has neither a battery nor an adapter, which is exactly right for
# desktops. Hand-rolled sysfs globbing gets that case wrong.
cau_on_ac() {
	if cau_have systemd-ac-power; then
		systemd-ac-power > /dev/null 2>&1
		return $?
	fi

	local ps online
	for ps in /sys/class/power_supply/*; do
		[[ -r "$ps/type" ]] || continue
		[[ "$(< "$ps/type")" == Mains ]] || continue
		[[ -r "$ps/online" ]] || continue
		online="$(< "$ps/online")"
		[[ $online == 1 ]] && return 0
	done

	# No mains device found at all: if there is no system battery either this
	# is a desktop, so assume mains rather than blocking updates forever.
	cau_battery_percent > /dev/null || return 0
	return 1
}

# cau_battery_percent
# Average charge across the system batteries, or failure when the machine has
# none. Peripheral batteries (mice, headsets) advertise type=Battery too and
# are filtered out via the scope attribute. When scope is missing entirely (as
# on many laptops), the device counts as a system battery.
cau_battery_percent() {
	local ps sum=0 count=0 cap

	for ps in /sys/class/power_supply/*; do
		[[ -r "$ps/type" ]] || continue
		[[ "$(< "$ps/type")" == Battery ]] || continue

		if [[ -r "$ps/scope" ]]; then
			[[ "$(< "$ps/scope")" == System ]] || continue
		fi
		if [[ -r "$ps/present" ]]; then
			[[ "$(< "$ps/present")" == 1 ]] || continue
		fi
		[[ -r "$ps/capacity" ]] || continue

		cap="$(< "$ps/capacity")"
		[[ $cap =~ ^[0-9]+$ ]] || continue
		sum=$(( sum + cap ))
		count=$(( count + 1 ))
	done

	(( count > 0 )) || return 1
	printf '%s\n' "$(( sum / count ))"
}

# cau_power_ok
# Applies RequireAC and MinBatteryPercent.
cau_power_ok() {
	local pct

	if cau_on_ac; then
		return 0
	fi

	if [[ $CFG_REQUIRE_AC == yes ]]; then
		CAU_SKIP_REASON="$(cau_msg "running on battery")"
		return 1
	fi

	pct="$(cau_battery_percent)" || return 0   # no battery: nothing to gate on

	if (( pct < CFG_MIN_BATTERY )); then
		CAU_SKIP_REASON="$(cau_msg "battery at %d%%, below the %d%% threshold" \
			"$pct" "$CFG_MIN_BATTERY")"
		return 1
	fi

	return 0
}

# ---------------------------------------------------------------------------
# Is the user in the middle of something?
# ---------------------------------------------------------------------------

# Process names that mean a game is actually running. Deliberately excludes
# "steam" itself, which sits in the tray all day on a gaming machine.
CAU_GAME_PROCESSES=(
	gamescope
	wine wine64 wineserver wine-preloader wine64-preloader
	umu-run proton
	reaper
	lutris lutris-wrapper heroic bottles
	retroarch dolphin-emu pcsx2-qt rpcs3 cemu ryujinx ppsspp citra-qt duckstation-qt
)

# cau_process_running <name>
# Exact-name match that ignores kernel threads. The exactness matters: a
# substring match on "reaper" hits the kernel's own oom_reaper on every box.
cau_process_running() {
	local name="$1" pid ppid

	while read -r pid; do
		[[ $pid =~ ^[0-9]+$ ]] || continue
		(( pid == 2 )) && continue
		# /proc/<pid>/status rather than /stat: the comm field in /stat can
		# contain spaces and parentheses, which shifts every column after it.
		ppid="$(sed -n 's/^PPid:[[:space:]]*//p' "/proc/$pid/status" 2>/dev/null)" || continue
		[[ $ppid == 2 ]] && continue        # kernel thread
		return 0
	done < <(pgrep -x -- "$name" 2>/dev/null)

	return 1
}

# cau_gamemode_active
# Asks GameMode how many clients it is tracking, per graphical session.
# GameMode is optional and frequently absent; any failure means "unknown", not
# "busy".
cau_gamemode_active() {
	local user uid out count

	cau_have busctl || return 1

	while read -r user uid; do
		[[ -n $user ]] || continue
		# timeout runs inside the runuser call because it has to be a real
		# binary there. It cannot wrap a shell function from out here.
		out="$(cau_as_user "$user" "$uid" timeout 5 busctl --user --json=short \
			get-property com.feralinteractive.GameMode \
			/com/feralinteractive/GameMode \
			com.feralinteractive.GameMode ClientCount 2>/dev/null)" || continue
		count="$(sed -nE 's/.*"data"[[:space:]]*:[[:space:]]*([0-9]+).*/\1/p' <<< "$out")"
		[[ -z $count ]] && count="$(awk '{print $NF}' <<< "$out")"
		[[ $count =~ ^[0-9]+$ ]] || continue
		(( count > 0 )) && return 0
	done < <(cau_active_session_users)

	return 1
}

# cau_idle_inhibited
# A blocking "idle" inhibitor is what fullscreen games and video players take.
cau_idle_inhibited() {
	cau_have systemd-inhibit || return 1
	systemd-inhibit --list 2>/dev/null \
		| awk 'tolower($0) ~ /idle/ && $NF == "block" { found = 1 } END { exit !found }'
}

# cau_busy
# True when something is running that an update should not interrupt.
cau_busy() {
	local proc

	if cau_gamemode_active; then
		CAU_SKIP_REASON="$(cau_msg "a game is running (GameMode)")"
		return 0
	fi

	for proc in "${CAU_GAME_PROCESSES[@]}"; do
		if cau_process_running "$proc"; then
			CAU_SKIP_REASON="$(cau_msg "a game is running (%s)" "$proc")"
			return 0
		fi
	done

	if cau_idle_inhibited; then
		CAU_SKIP_REASON="$(cau_msg "an application is blocking idle (fullscreen game or video)")"
		return 0
	fi

	return 1
}

# ---------------------------------------------------------------------------
# Scheduling
# ---------------------------------------------------------------------------

# cau_update_due
# The timer fires hourly; this decides whether enough time has passed since the
# last *successful* run. Deferred runs therefore retry automatically without
# any backoff bookkeeping of their own.
cau_update_due() {
	local last now

	last="$(cau_state_read last_success 0)"
	[[ $last =~ ^[0-9]+$ ]] || last=0
	(( last == 0 )) && return 0

	now="$(date +%s)"
	(( now - last >= CFG_INTERVAL_SECONDS ))
}
