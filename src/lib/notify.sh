# shellcheck shell=bash
#
# Desktop notifications from a root system service.
#
# Two paths exist:
#   * live:   somebody has a graphical session, so notify-send is run inside
#             it via runuser with the session bus address set;
#   * queued: nobody is logged in, so the message is appended to a spool that
#             the XDG autostart entry replays at the next login.
#
# Messages travel as a msgid plus printf arguments rather than as finished
# text, so a notification queued at 04:00 is still rendered in whatever locale
# the user's desktop turns out to be running in.

CAU_NOTIFY_ICON="system-software-update"
CAU_NOTIFY_QUEUE_MAX=20

# cau_notify <urgency> <linger: yes|no> <title-msgid> <body-msgid> [args...]
# Never fails: a machine without libnotify, or with nobody logged in, is a
# normal state, not an error.
cau_notify() {
	cau_notify_tagged '' yes "$@"
}

# cau_notify_close <user> <uid> <id>
# notify-send can create and replace notifications but not withdraw one, so
# this goes to the bus directly. gdbus comes from glib2, which libnotify itself
# links against, so it is present wherever notify-send is.
cau_notify_close() {
	cau_as_user "$1" "$2" gdbus call --session \
		--dest org.freedesktop.Notifications \
		--object-path /org/freedesktop/Notifications \
		--method org.freedesktop.Notifications.CloseNotification \
		"$3" > /dev/null 2>&1 || true
}

# cau_notify_tagged <tag> <queue: yes|no> <urgency> <linger: yes|no> \
#                   <title> <body> [args...]
#
# linger=yes keeps the message on screen until somebody dismisses it; anything
# else lets it time out on its own. The line it draws is whether the machine
# still needs a person: a finished update is over and done with and should not
# have to be clicked away, while a failure, a paused queue or a package that
# had to be skipped is only ever seen if it waits.
#
# Set explicitly rather than left to the server. Notification daemons do keep
# critical-urgency messages up (the spec asks them to, and Plasma obliges).
# But that is a "should", it says nothing about the normal-urgency messages
# here that still need somebody to act, and urgency separately controls sound
# and whether do-not-disturb is overridden. Those are not the same question.
#
# A tag means "at most one bubble of this kind on screen at a time": the
# previous one carrying the same tag is withdrawn first, so "installing
# updates" gives way to "system updated" instead of leaving two messages that
# contradict each other.
#
# Withdraw-then-post rather than the obvious --replace-id, because replacing
# only works while the old bubble is still on screen. Plasma's server drops a
# Notify() whose replaces_id names an expired notification: no bubble, no
# error, and the id it hands back is the dead one it just ignored. An update
# run lasts minutes and the start bubble times out after seconds, so the
# finished message landed in exactly that hole and was never seen. Closing an
# id that is already gone is a no-op, which makes this safe either way.
#
# queue=no is for messages that only mean anything while somebody is looking.
# Telling a user at next login that an update started an hour ago is noise.
cau_notify_tagged() {
	local tag="$1" queue="$2" urgency="$3" linger="$4" title="$5" body="$6"
	shift 6
	local -a args=("$@")
	local delivered=0 user uid locale t b prev newid

	[[ $CFG_NOTIFICATIONS == yes ]] || return 0

	# -1 is "whatever the server thinks", which is what a message nobody has to
	# act on wants; 0 is "until dismissed".
	local -a expiry=(--expire-time=-1)
	[[ $linger == yes ]] && expiry=(--expire-time=0)

	while read -r user uid; do
		[[ -n $user ]] || continue
		cau_as_user "$user" "$uid" sh -c 'command -v notify-send >/dev/null' || continue

		locale="$(cau_user_locale "$user" "$uid")"
		t="$(cau_msg_in "$locale" "$title")"
		b="$(cau_msg_in "$locale" "$body" "${args[@]}")"

		if [[ -n $tag ]]; then
			prev="$(cau_state_read "notify_id_${tag}_${user}" 0)"
			if [[ $prev =~ ^[0-9]+$ ]] && (( prev > 0 )); then
				cau_notify_close "$user" "$uid" "$prev"
			fi
			cau_state_clear "notify_id_${tag}_${user}"
		fi

		if newid="$(cau_as_user "$user" "$uid" notify-send \
			--app-name="$CAU_PRETTY" \
			--icon="$CAU_NOTIFY_ICON" \
			--urgency="$urgency" \
			"${expiry[@]}" \
			--print-id \
			-- "$t" "$b" 2>/dev/null)"
		then
			delivered=1
			if [[ -n $tag && $newid =~ ^[0-9]+$ ]]; then
				cau_state_write "notify_id_${tag}_${user}" "$newid"
			fi
		fi
	done < <(cau_active_session_users)

	(( delivered )) && return 0
	[[ $queue == yes ]] || return 0

	cau_notify_enqueue "$urgency" "$linger" "$title" "$body" "${args[@]}"
}

# cau_notify_enqueue <urgency> <linger> <title-msgid> <body-msgid> [args...]
# Tab-separated records, oldest first. The file is world-readable on purpose:
# the login-time delivery runs unprivileged and only ever reads it.
#
# The key is a nanosecond timestamp rather than a second one: delivery marks
# progress by "last key seen", so two records sharing a key could make the
# second one unreachable forever if a login landed between them.
cau_notify_enqueue() {
	local urgency="$1" linger="$2" title="$3" body="$4"
	shift 4
	local record tmp

	mkdir -p "$CAU_STATEDIR" 2>/dev/null || return 0

	record="$(date +%s%N)"$'\t'"$urgency"$'\t'"$linger"$'\t'"$title"$'\t'"$body"
	local arg
	for arg in "$@"; do
		record+=$'\t'"${arg//$'\t'/ }"
	done

	printf '%s\n' "$record" >> "$CAU_NOTIFY_QUEUE" 2>/dev/null || return 0

	# keep the spool bounded: nobody wants three weeks of backlog at login
	if (( $(wc -l < "$CAU_NOTIFY_QUEUE" 2>/dev/null || echo 0) > CAU_NOTIFY_QUEUE_MAX )); then
		tmp="$(mktemp "${CAU_NOTIFY_QUEUE}.XXXXXX")" || return 0
		tail -n "$CAU_NOTIFY_QUEUE_MAX" "$CAU_NOTIFY_QUEUE" > "$tmp"
		mv -f "$tmp" "$CAU_NOTIFY_QUEUE"
	fi
	chmod 0644 "$CAU_NOTIFY_QUEUE" 2>/dev/null || true
}

# cau_notify_deliver_queue
# Runs unprivileged, as the freshly logged-in user, from the XDG autostart
# entry. State about what has already been seen lives in the user's own home,
# so no write access to /var/lib is needed and each user is tracked separately.
cau_notify_deliver_queue() {
	local seen_file seen ts urgency linger title body rest
	local -a args expiry

	[[ -r $CAU_NOTIFY_QUEUE ]] || return 0
	cau_have notify-send || return 0

	seen_file="${XDG_STATE_HOME:-$HOME/.local/state}/cachy-auto-update/notify-seen"
	mkdir -p "$(dirname "$seen_file")" 2>/dev/null || return 0

	seen=0
	[[ -r $seen_file ]] && seen="$(< "$seen_file")"
	[[ $seen =~ ^[0-9]+$ ]] || seen=0

	local newest="$seen"
	while IFS=$'\t' read -r ts urgency linger title body rest; do
		[[ $ts =~ ^[0-9]+$ ]] || continue
		(( ts > seen )) || continue

		# Records spooled before the linger field existed have the title where
		# the flag now sits. Shift them back rather than announcing an update
		# under the headline "yes".
		if [[ $linger != yes && $linger != no ]]; then
			rest="${body}${rest:+$'\t'}${rest:-}"
			body="$title"
			title="$linger"
			linger=no
		fi

		# remaining tab-separated fields are the body's printf arguments
		args=()
		if [[ -n ${rest:-} ]]; then
			IFS=$'\t' read -r -a args <<< "$rest"
		fi

		expiry=(--expire-time=-1)
		[[ $linger == yes ]] && expiry=(--expire-time=0)

		notify-send \
			--app-name="$CAU_PRETTY" \
			--icon="$CAU_NOTIFY_ICON" \
			--urgency="${urgency:-normal}" \
			"${expiry[@]}" \
			-- "$(cau_msg "$title")" "$(cau_msg "$body" "${args[@]}")" 2>/dev/null || true

		(( ts > newest )) && newest="$ts"
	done < "$CAU_NOTIFY_QUEUE"

	printf '%s\n' "$newest" > "$seen_file" 2>/dev/null || true
}
