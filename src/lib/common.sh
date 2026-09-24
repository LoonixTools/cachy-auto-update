# shellcheck shell=bash
#
# Paths, logging, translations and small shared helpers.
#
# Sourced by both the CLI and the update runner, so it must not assume it is
# running as root and must not produce any output on its own.

CAU_VERSION="@VERSION@"
CAU_NAME="cachy-auto-update"
CAU_PRETTY="CachyOS Auto-Update"

# Directories. All of them are overridable so the tree can be exercised
# straight from a git checkout without installing anything.
CAU_LIBDIR="${CAU_LIBDIR:-@LIBDIR@}"
CAU_LIBEXECDIR="${CAU_LIBEXECDIR:-@LIBEXECDIR@}"
CAU_LOCALEDIR="${CAU_LOCALEDIR:-@LOCALEDIR@}"
CAU_CONFDIR="${CAU_CONFDIR:-/etc/cachy-auto-update}"
CAU_CONFIG="${CAU_CONFIG:-${CAU_CONFDIR}/cachy-auto-update.conf}"
CAU_STATEDIR="${CAU_STATEDIR:-/var/lib/cachy-auto-update}"
CAU_CACHEDIR="${CAU_CACHEDIR:-/var/cache/cachy-auto-update}"
CAU_LOGDIR="${CAU_LOGDIR:-/var/log/cachy-auto-update}"
CAU_RUNDIR="${CAU_RUNDIR:-/run/cachy-auto-update}"

CAU_LOGFILE="${CAU_LOGDIR}/cachy-auto-update.log"
CAU_RUNLOG="${CAU_LOGDIR}/last-run.log"
CAU_LOCKFILE="${CAU_RUNDIR}/run.lock"
CAU_NOTIFY_QUEUE="${CAU_STATEDIR}/notify-queue"

# The locked system account that builds and installs AUR packages. Created by
# sysusers.d; its home lives under the state directory.
CAU_BUILD_USER="cachy-auto-update"
CAU_BUILD_HOME="${CAU_STATEDIR}/builder"

# Pacman's own database lock. Presence means somebody is mid-transaction.
CAU_PACMAN_LOCK="/var/lib/pacman/db.lck"

# ---------------------------------------------------------------------------
# Translations
# ---------------------------------------------------------------------------
# Messages are authored in English and translated through gettext. The runner
# is started with LC_ALL=C so that pacman/upower output stays parseable, which
# means every user-facing string has to opt back into a real locale explicitly
# via cau_msg / cau_msg_in.

export TEXTDOMAIN="cachy-auto-update"
export TEXTDOMAINDIR="${CAU_LOCALEDIR}"

# The locale user-facing text should be rendered in.
#
# Standard POSIX precedence, deliberately: LC_ALL wins outright, and LC_ALL=C
# really does mean English. The service sets LC_ALL=C so that pacman and upower
# stay parseable, which makes the log English. That is correct, since the log
# is a technical artefact. Anything aimed at a person (a desktop notification) does
# not go through here at all; it names the recipient's own locale explicitly
# via cau_msg_in and cau_user_locale.
cau_ui_locale() {
	local l="${CAU_UI_LOCALE:-}"

	if [[ -z $l ]]; then
		l="${LC_ALL:-}"
		[[ -z $l ]] && l="${LC_MESSAGES:-}"
		[[ -z $l ]] && l="${LANG:-}"
	fi

	if [[ -z $l && -r /etc/locale.conf ]]; then
		l="$(sed -n 's/^LANG=//p' /etc/locale.conf | tr -d '"' | head -n1)"
	fi

	printf '%s\n' "${l:-C}"
}

# cau_msg <msgid> [printf args...]
# Translate a string into the UI locale and printf it (no trailing newline).
cau_msg() {
	cau_msg_in "$(cau_ui_locale)" "$@"
}

# cau_msg_into <locale> <msgid>
# Plain lookup with the result in CAU_MSG_RESULT and no printf formatting.
# For callers that redraw many labels per keypress, where wrapping cau_msg in a
# command substitution would cost a fork per label.
CAU_MSG_RESULT=''

cau_msg_into() {
	local locale="$1" msgid="$2" cachekey
	cachekey="${locale}"$'\x1f'"${msgid}"

	if [[ -n ${CAU_MSG_CACHE[$cachekey]+set} ]]; then
		CAU_MSG_RESULT="${CAU_MSG_CACHE[$cachekey]}"
		return 0
	fi

	CAU_MSG_RESULT="$(LC_ALL="$locale" LANGUAGE="${locale%%.*}" gettext -- "$msgid" 2>/dev/null)"
	[[ -n $CAU_MSG_RESULT ]] || CAU_MSG_RESULT="$msgid"
	CAU_MSG_CACHE[$cachekey]="$CAU_MSG_RESULT"
	return 0
}

# Translations are memoized. Every gettext lookup is a fork, and the settings
# screen redraws forty-odd labels per keypress; without this the redraw takes
# long enough that a keystroke arriving during it is lost when the terminal
# switches back to single-character mode.
declare -A CAU_MSG_CACHE=()

# cau_msg_in <locale> <msgid> [printf args...]
cau_msg_in() {
	local locale="$1" msgid="$2" translated cachekey
	shift 2

	cachekey="${locale}"$'\x1f'"${msgid}"
	if [[ -n ${CAU_MSG_CACHE[$cachekey]+set} ]]; then
		translated="${CAU_MSG_CACHE[$cachekey]}"
	else
		translated="$(LC_ALL="$locale" LANGUAGE="${locale%%.*}" gettext -- "$msgid" 2>/dev/null)"
		[[ -n $translated ]] || translated="$msgid"
		CAU_MSG_CACHE[$cachekey]="$translated"
	fi

	# With no arguments the message is plain text, not a format string. Feeding
	# it to printf anyway turns any literal percent sign in it (like
	# "Battery (%)" or "100 % done") into an invalid conversion, and that is a trap every
	# translator would eventually walk into.
	if (( $# == 0 )); then
		printf '%s' "$translated"
		return
	fi

	# shellcheck disable=SC2059  # the format string is the translated message
	printf -- "$translated" "$@"
}

# ---------------------------------------------------------------------------
# Output and logging
# ---------------------------------------------------------------------------

# Is a person watching? Decided once, here, while stdout is still whatever the
# process was started with. Testing `-t 1` at the point of use is unreliable:
# any function called through $(...) or <(...) sees a pipe on stdout and would
# conclude nobody is there.
CAU_INTERACTIVE=''
[[ -t 1 ]] && CAU_INTERACTIVE=1

if [[ -n $CAU_INTERACTIVE && -z ${NO_COLOR:-} ]]; then
	CAU_C_RESET=$'\033[0m'
	CAU_C_BOLD=$'\033[1m'
	CAU_C_DIM=$'\033[2m'
	CAU_C_BLUE=$'\033[38;2;23;147;209m'
	CAU_C_GREEN=$'\033[32m'
	CAU_C_YELLOW=$'\033[33m'
	CAU_C_RED=$'\033[31m'
else
	CAU_C_RESET='' CAU_C_BOLD='' CAU_C_DIM='' CAU_C_BLUE=''
	CAU_C_GREEN='' CAU_C_YELLOW='' CAU_C_RED=''
fi

# Appends to the persistent log when writable (i.e. when running as root) and
# always echoes to stderr so journald picks it up for the service.
cau_log() {
	local level="$1"
	shift
	local line
	line="$(date '+%Y-%m-%d %H:%M:%S') [$level] $*"

	printf '%s\n' "$line" >&2
	if [[ -n ${CAU_LOG_OPEN:-} ]]; then
		printf '%s\n' "$line" >> "$CAU_LOGFILE" 2>/dev/null || true
		printf '%s\n' "$line" >> "$CAU_RUNLOG" 2>/dev/null || true
	fi
}

cau_info()  { cau_log INFO  "$@"; }
cau_warn()  { cau_log WARN  "$@"; }
cau_error() { cau_log ERROR "$@"; }
cau_debug() { [[ -n ${CAU_DEBUG:-} ]] && cau_log DEBUG "$@"; return 0; }

# Opens the log files for this run. Only meaningful as root.
cau_log_open() {
	mkdir -p "$CAU_LOGDIR" 2>/dev/null || return 0
	chmod 0750 "$CAU_LOGDIR" 2>/dev/null || true
	: > "$CAU_RUNLOG" 2>/dev/null || return 0
	CAU_LOG_OPEN=1
}

# Runs a command, capturing its combined output in the run log. Returns the
# command's exit status.
#
# When a person is watching (`cachy-auto-update run` from a terminal), the
# output is shown as well. Building an AUR package or pulling a few hundred
# megabytes of Flatpak can take minutes, and silence for that long is
# indistinguishable from a hang.
cau_run_logged() {
	if [[ -n ${CAU_LOG_OPEN:-} ]]; then
		if [[ -n $CAU_INTERACTIVE ]]; then
			"$@" 2>&1 | tee -a "$CAU_RUNLOG"
			return "${PIPESTATUS[0]}"
		fi
		"$@" >> "$CAU_RUNLOG" 2>&1
	else
		"$@" >&2
	fi
}

# ---------------------------------------------------------------------------
# Terminal helpers for the CLI
# ---------------------------------------------------------------------------

# Set by cau_bad and cau_note. The menu redraws immediately after an action,
# which would wipe the screen; this marks that something was printed that the
# user still has to read, so only those cases wait for a keypress. A plain
# success needs no acknowledgement, because the status block at the top of the
# menu already shows the new state.
CAU_UI_NEEDS_ACK=''

cau_say()  { printf '%s\n' "$*"; }
cau_head() { printf '\n%s%s%s\n\n' "$CAU_C_BOLD$CAU_C_BLUE" "$*" "$CAU_C_RESET"; }
cau_ok()   { printf '%s✔%s %s\n' "$CAU_C_GREEN" "$CAU_C_RESET" "$*"; }
cau_bad()  { CAU_UI_NEEDS_ACK=1; printf '%s✘%s %s\n' "$CAU_C_RED" "$CAU_C_RESET" "$*" >&2; }
cau_note() { CAU_UI_NEEDS_ACK=1; printf '%s•%s %s\n' "$CAU_C_DIM" "$CAU_C_RESET" "$*"; }

# ---------------------------------------------------------------------------
# State files
# ---------------------------------------------------------------------------
# Every piece of state is one small file under CAU_STATEDIR. That keeps the
# CLI, the runner and the login-time notification delivery from needing any
# shared parsing code.

cau_state_read() {
	local key="$1" default="${2:-}"
	if [[ -r "$CAU_STATEDIR/$key" ]]; then
		cat "$CAU_STATEDIR/$key"
	else
		printf '%s\n' "$default"
	fi
}

cau_state_write() {
	local key="$1"
	shift
	mkdir -p "$CAU_STATEDIR" 2>/dev/null || return 1
	printf '%s\n' "$*" > "$CAU_STATEDIR/$key"
}

cau_state_clear() {
	rm -f "$CAU_STATEDIR/$1" 2>/dev/null || true
}

# ---------------------------------------------------------------------------
# Misc
# ---------------------------------------------------------------------------

cau_have() { command -v "$1" > /dev/null 2>&1; }

cau_is_root() { [[ $EUID -eq 0 ]]; }

# Turns a duration such as "1d", "6h", "30m" or a bare number of seconds into
# seconds. Returns the fallback for anything unparseable.
cau_duration_to_seconds() {
	local v="$1" fallback="${2:-86400}" num unit

	[[ $v =~ ^([0-9]+)([smhdw]?)$ ]] || { printf '%s\n' "$fallback"; return; }
	num="${BASH_REMATCH[1]}"
	unit="${BASH_REMATCH[2]}"

	case "$unit" in
		s|'') printf '%s\n' "$num" ;;
		m)    printf '%s\n' "$(( num * 60 ))" ;;
		h)    printf '%s\n' "$(( num * 3600 ))" ;;
		d)    printf '%s\n' "$(( num * 86400 ))" ;;
		w)    printf '%s\n' "$(( num * 604800 ))" ;;
	esac
}

# Human-readable "x minutes ago" for a unix timestamp. Empty input yields the
# translated "never".
cau_time_ago() {
	local ts="$1" now delta

	[[ $ts =~ ^[0-9]+$ ]] || { cau_msg "never"; printf '\n'; return; }

	now="$(date +%s)"
	delta=$(( now - ts ))
	(( delta < 0 )) && delta=0

	if   (( delta < 60 ));     then cau_msg "just now"
	elif (( delta < 120 ));    then cau_msg "1 minute ago"
	elif (( delta < 3600 ));   then cau_msg "%d minutes ago" "$(( delta / 60 ))"
	elif (( delta < 7200 ));   then cau_msg "1 hour ago"
	elif (( delta < 86400 ));  then cau_msg "%d hours ago" "$(( delta / 3600 ))"
	elif (( delta < 172800 )); then cau_msg "1 day ago"
	else                            cau_msg "%d days ago" "$(( delta / 86400 ))"
	fi
	printf '\n'
}
