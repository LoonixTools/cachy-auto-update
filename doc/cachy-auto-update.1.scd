cachy-auto-update(1)

# NAME

cachy-auto-update - unattended background updates for CachyOS

# SYNOPSIS

*cachy-auto-update* [_command_] [_options_]

# DESCRIPTION

*cachy-auto-update* keeps a CachyOS machine current without anybody having to
think about it: repository packages, AUR packages, Flatpaks and AppImages are
updated in the background, with no password prompt and no terminal.

Run without a command it opens a small interactive menu with the two switches
that matter (automatic updates on/off and notifications on/off), plus the
current status, and a settings screen covering every remaining option so the
configuration file never has to be edited by hand. In the settings screen the
arrow keys select, Space or Right changes a value, and _q_ goes back; every
change is written out immediately.

The actual work is done by a systemd system service. The timer ticks hourly;
whether a tick does anything is decided by _UpdateInterval_ (daily by default).
A run that is postponed (low battery, a game running, somebody else using
pacman) is simply retried at the next tick.

# COMMANDS

*enable*
	Turn automatic updates on and enable the systemd timer.

*disable*
	Turn automatic updates off and stop the timer.

*notifications* on|off
	Turn desktop notifications on or off.

*status*
	Show the current state plus an evaluation of every condition that can
	postpone a run. This is the first thing to look at when the updater
	appears to be doing nothing.

*run* [--force] [--dry-run]
	Update now. *--force* skips the interval, battery and gaming checks;
	*--dry-run* lists what would be updated and changes nothing. The check
	for another running package manager is never skipped.

*log* [-n _num_] [-f] [-a]
	Show the log of the last run. *-a* shows the rolling log instead, *-f*
	follows it.

*-h*, *--help*
	Show a short help text.

*-V*, *--version*
	Show the version.

# CONDITIONS

Before anything is installed, a run is postponed when:

- the battery is below _MinBatteryPercent_ (ignored on mains power, and on
  machines without a battery);
- _RequireAC_ is set and the machine is not plugged in;
- a game is running, detected via GameMode, a list of known game processes, or
  an application holding a blocking idle inhibitor;
- pacman's database is locked, or pacman, yay, paru, pamac or a similar tool is
  running.

The last check is what keeps *cachy-auto-update* out of the way of manual
package management. It cannot work in the other direction: if a manual pacman
run starts while an update is already in flight, that run will report the usual
locked-database error.

# HOW UPDATES ARE APPLIED

Repository packages are updated with *pacman -Syu --noconfirm*, but only after
*checkupdates*(8) has confirmed there is something to do, so on a quiet day
pacman's lock is never taken at all.

A package that has to replace another one is handled silently. If pacman would
stop to ask whether a conflicting package may be removed, the transaction is
retried once with that question answered affirmatively, unless
_AutoResolveConflicts_ is turned off. Files on disk that collide with a package
are *not* forced. That stays a human decision. A signature failure triggers one
keyring refresh and one retry.

AUR packages are built and installed as the locked *cachy-auto-update* system
account, because *makepkg*(8) refuses to run as root. That account has no
password and no shell. Through _/etc/sudoers.d/cachy-auto-update_ it is allowed
to invoke *pacman* without one. No user password is ever stored anywhere.

A failed AUR build is not reported the first time it happens; only a failure
that repeats is worth waking somebody up for.

Flatpak system installations are updated directly as root, user installations
inside each user's own account. AppImages are updated through Gear Lever, for
users with a graphical session, and AppImages whose application is currently
running are skipped.

The machine is never restarted automatically. A kernel update that makes a
restart necessary is reported by *cachy-auto-update status*, not by a
notification: the running kernel loses its module tree as soon as pacman
unpacks the new one, so a bubble would fire while the run is still working
through AUR packages and Flatpaks, and reads as an invitation to restart in the
middle of it.

# PROGRESS

For as long as a run is working, the notification area carries a live progress
entry: the step being performed, how many items it has got through, an overall
percentage, and the package currently being unpacked under "Details". This is a
job in the sense of *org.kde.JobViewServer*, the same mechanism a file manager
uses while copying, rather than a notification. That is what makes it a bar
instead of a line of text.

The desktop withdraws a job as soon as the D-Bus connection that requested it
closes, so a helper process runs inside each graphical session for the duration
of the update and holds that connection open. It requires _python-gobject_.
Where that is missing, or on a desktop with no job interface, there is no
progress entry and nothing else is affected.

Working out the upgrade, fetching it and unpacking it are three steps rather
than one. Between "Starting full system upgrade" and the transaction it
eventually prepares, pacman prints nothing at all, and on a large backlog that
silence runs to minutes; that stretch therefore carries a label of its own and
deliberately no item count, because a counter frozen at "0 of 161" reads as a
stuck update. On a domestic line the download is then the longest of the three,
and a bar that called the whole thing "installing" would sit near its beginning
for minutes at a time looking stuck.

Expanding "Details" shows the last five lines of the run log as they are
written, alongside the package currently being worked on. The percentage says
the update is alive; these lines say what it is alive doing, which matters most
during the stretches that have nothing countable to report, above all an AUR
package being compiled. The job interface carries exactly two description
fields, so those are the two things shown.

A step that turns out to have no work is dropped from the bar instead of being
handed its share for nothing. Packages already in the cache are never
announced, so a run with everything already fetched skips the download step
outright rather than jumping when unpacking starts; the same applies to AUR
with nothing pending, or a machine with no Flatpaks installed.

Neither counted phase gets a counter from pacman on an unattended run, so both
are counted here, a line at a time: "foo-1.2-1-x86_64 downloading..." for the
first, "upgrading foo..." for the second. pacman's output is line-buffered
through *stdbuf*(1) so those lines arrive as they happen. Writing to a log
rather than a terminal, libc would otherwise release them in 4KB blocks, around
a hundred and sixty packages at a time. pacman's other (n/m) sequences
(checking keys, package integrity, loading package files) each count up to the
same total and are deliberately ignored.

# HOW LONG NOTIFICATIONS STAY

A message that reports the machine still needing a person stays on screen
until it is dismissed. For example: an update that failed, a package database
left locked, or packages that had to be held back. A message like that is only worth
sending if it is still there when somebody comes back to the machine.

Everything else times out by itself, an update that simply worked included.
Nothing should have to be clicked away for having gone right.

This is set per message rather than left to the notification daemon. Daemons do
keep critical-urgency messages up, and the specification asks them to, but that
is a recommendation, it does not cover the normal-urgency messages here that
still need somebody to act, and urgency separately governs sound and whether
do-not-disturb is overridden.

Where nobody is logged in the message is spooled and delivered at the next
login, with the same distinction preserved.

# INTERRUPTED UPDATES

Before a transaction starts, a notification says an update is running and asks
for the machine to be left on. It is withdrawn again when the result arrives,
so one bubble is used rather than two.

While a transaction is running, *cachy-auto-update* holds a
*systemd-inhibit*(1) lock on _sleep_ and _shutdown_ in blocking mode, so a
suspend, a lid close or a normal shutdown request cannot cut it short.

What a user sees when they try anyway is worth knowing: logind refuses the
request and falls back to the polkit action
_org.freedesktop.login1.power-off-ignore-inhibit_, which is _auth_admin_keep_,
so the desktop presents an administrator password prompt reading "Power off the
system while an application is inhibiting this". That string is shipped
untranslated by systemd and does not mention updates, and no KDE dialog
explains the situation either. Only *systemctl*(1) names the inhibitor and its
reason. Hence the notification.

A hard power-off (holding the power button, or losing mains power) is not
preventable. On the next run a leftover _/var/lib/pacman/db.lck_ is removed if
it is older than the current boot, since no process able to hold it can still
exist; the upgrade is then repeated and pacman reinstalls whatever was caught
half-written. A lock file that is unheld but was created during the current
boot is reported rather than removed, because there is no way to prove it is
abandoned.

Without that recovery a single power cut would leave a lock that makes every
subsequent run defer, silently stopping updates for good.

On Btrfs with *snapper*(8) and *snap-pac*, each pacman transaction is bracketed
by a pre and post snapshot, so a broken upgrade remains rollbackable.

# FILES

_/etc/cachy-auto-update/cachy-auto-update.conf_
	Configuration. See the comments in the file itself for every option.

_/etc/sudoers.d/cachy-auto-update_
	Lets the build account call pacman without a password.

_/var/log/cachy-auto-update/last-run.log_
	Full output of the most recent run.

_/var/log/cachy-auto-update/cachy-auto-update.log_
	Rolling log across runs.

_/var/lib/cachy-auto-update/_
	State: timestamps, counters, queued notifications.

# SEE ALSO

*pacman*(8), *checkupdates*(8), *paru*(8), *yay*(8), *flatpak*(1),
*systemd.timer*(5)

# AUTHOR

Felitendo. Source and bug reports:
https://github.com/LoonixTools/cachy-auto-update
