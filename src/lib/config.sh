# shellcheck shell=bash
#
# Reading and writing /etc/cachy-auto-update/cachy-auto-update.conf.
#
# The file is deliberately *parsed* rather than sourced: it is edited by a
# root-run daemon, and sourcing it would turn a stray line into arbitrary code
# execution. The format is one "Key=Value" per line, '#' starts a comment.

# The file is cached and parsed in-process rather than shelled out to sed on
# every lookup. The settings screen reads every key on every redraw, and a fork
# per key made the redraw slow enough to swallow keystrokes.
CAU_CONFIG_CACHE=''
CAU_CONFIG_CACHED=0

_cau_config_slurp() {
	(( CAU_CONFIG_CACHED )) && return 0
	CAU_CONFIG_CACHE=''
	[[ -r $CAU_CONFIG ]] && CAU_CONFIG_CACHE="$(< "$CAU_CONFIG")"
	CAU_CONFIG_CACHED=1
	return 0
}

# _cau_config_lookup <Key> [default]
# Result in CAU_CONFIG_VALUE. Assigning rather than printing matters on the
# settings screen, which reads every key on every frame: a command substitution
# there is a fork, and forks were the entire cost of a redraw.
CAU_CONFIG_VALUE=''

_cau_config_lookup() {
	local key="$1" default="${2:-}" val='' line

	CAU_CONFIG_VALUE="$default"
	[[ -r $CAU_CONFIG ]] || return 0
	_cau_config_slurp

	# last assignment wins, matching the previous sed|tail behaviour
	while IFS= read -r line; do
		[[ $line == *"$key"* ]] || continue
		[[ $line =~ ^[[:space:]]*"$key"[[:space:]]*=(.*)$ ]] || continue
		val="${BASH_REMATCH[1]}"
	done <<< "$CAU_CONFIG_CACHE"

	# strip a trailing comment and surrounding whitespace/quotes
	val="${val%%#*}"
	val="${val#"${val%%[![:space:]]*}"}"
	val="${val%"${val##*[![:space:]]}"}"
	val="${val%\"}"
	val="${val#\"}"

	[[ -n $val ]] && CAU_CONFIG_VALUE="$val"
	return 0
}

# cau_config_get <Key> [default]
cau_config_get() {
	_cau_config_lookup "$@"
	printf '%s\n' "$CAU_CONFIG_VALUE"
}

# cau_config_bool <Key> <default: yes|no>
# Succeeds when the key is truthy.
cau_config_bool() {
	local val
	val="$(cau_config_get "$1" "$2")"
	case "${val,,}" in
		yes|y|true|1|on|enabled) return 0 ;;
		*) return 1 ;;
	esac
}

# cau_config_int <Key> <default>
cau_config_int() {
	local val
	val="$(cau_config_get "$1" "$2")"
	[[ $val =~ ^-?[0-9]+$ ]] && printf '%s\n' "$val" || printf '%s\n' "$2"
}

# cau_config_set <Key> <Value>
# Replaces the key in place if present (including a commented-out template
# line), otherwise appends it. Requires write access to the config file.
cau_config_set() {
	local key="$1" value="$2" tmp

	if [[ ! -e $CAU_CONFIG ]]; then
		mkdir -p "$(dirname "$CAU_CONFIG")" || return 1
		printf '# %s configuration\n' "$CAU_NAME" > "$CAU_CONFIG" || return 1
	fi
	[[ -w $CAU_CONFIG ]] || return 1

	tmp="$(mktemp "${CAU_CONFIG}.XXXXXX")" || return 1
	# keep the original permissions rather than mktemp's 0600
	chmod --reference="$CAU_CONFIG" "$tmp" 2>/dev/null || chmod 0644 "$tmp"

	if grep -qE "^[[:space:]]*#?[[:space:]]*${key}[[:space:]]*=" "$CAU_CONFIG"; then
		awk -v key="$key" -v value="$value" '
			!done && $0 ~ "^[[:space:]]*#?[[:space:]]*" key "[[:space:]]*=" {
				print key "=" value; done = 1; next
			}
			# drop any further occurrences so the file cannot grow duplicates
			$0 ~ "^[[:space:]]*" key "[[:space:]]*=" { next }
			{ print }
		' "$CAU_CONFIG" > "$tmp" || { rm -f "$tmp"; return 1; }
	else
		cat "$CAU_CONFIG" > "$tmp" || { rm -f "$tmp"; return 1; }
		printf '%s=%s\n' "$key" "$value" >> "$tmp"
	fi

	mv -f "$tmp" "$CAU_CONFIG"
	CAU_CONFIG_CACHED=0
}

# ---------------------------------------------------------------------------
# Resolved settings
# ---------------------------------------------------------------------------
# Loaded once per process into plain variables so the rest of the code does not
# re-parse the file for every lookup.

cau_config_load() {
	CFG_ENABLED=no;               cau_config_bool Enabled              no  && CFG_ENABLED=yes
	CFG_NOTIFICATIONS=no;         cau_config_bool Notifications        yes && CFG_NOTIFICATIONS=yes
	CFG_REQUIRE_AC=no;            cau_config_bool RequireAC            no  && CFG_REQUIRE_AC=yes
	CFG_SKIP_GAMING=no;           cau_config_bool SkipWhenGaming       yes && CFG_SKIP_GAMING=yes
	CFG_AUR=no;                   cau_config_bool UpdateAUR            yes && CFG_AUR=yes
	CFG_FLATPAK=no;               cau_config_bool UpdateFlatpak        yes && CFG_FLATPAK=yes
	CFG_APPIMAGE=no;              cau_config_bool UpdateAppImages      yes && CFG_APPIMAGE=yes
	CFG_DEVEL=no;                 cau_config_bool UpdateDevel          no  && CFG_DEVEL=yes
	CFG_RESOLVE_CONFLICTS=no;     cau_config_bool AutoResolveConflicts yes && CFG_RESOLVE_CONFLICTS=yes
	CFG_CLEAN_CACHE=no;           cau_config_bool CleanCache           yes && CFG_CLEAN_CACHE=yes
	CFG_REMOVE_ORPHANS=no;        cau_config_bool RemoveOrphans        no  && CFG_REMOVE_ORPHANS=yes
	CFG_CLEAN_AUR_CACHE=no;       cau_config_bool CleanAURCache        yes && CFG_CLEAN_AUR_CACHE=yes
	CFG_NOTIFY_START=no;          cau_config_bool NotifyOnStart        yes && CFG_NOTIFY_START=yes
	CFG_MINIMIZE_PROGRESS=no;     cau_config_bool MinimizeProgress     no  && CFG_MINIMIZE_PROGRESS=yes
	CFG_NOTIFY_SUCCESS=no;        cau_config_bool NotifyOnSuccess      yes && CFG_NOTIFY_SUCCESS=yes
	CFG_NOTIFY_ERROR=no;          cau_config_bool NotifyOnError        yes && CFG_NOTIFY_ERROR=yes

	CFG_INTERVAL="$(cau_config_get UpdateInterval 1d)"
	CFG_INTERVAL_SECONDS="$(cau_duration_to_seconds "$CFG_INTERVAL" 86400)"
	CFG_MIN_BATTERY="$(cau_config_int MinBatteryPercent 30)"
	CFG_KEEP_OLD="$(cau_config_int KeepOldPackages 3)"
	CFG_AUR_HELPER="$(cau_config_get AURHelper auto)"
	CFG_IGNORE_PKG="$(cau_config_get IgnorePkg '')"
}
