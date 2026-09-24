# Changelog

What each release brings, newest first. The [GitHub releases](https://github.com/LoonixTools/cachy-auto-update/releases) add every commit that went into it.

## v1.3.0

_2026-08-20_

Welcome to cachy-auto-update `v1.3.0`! This release is all about the progress bar. It now moves through the whole run, and you can finally see what an update is doing while it works.

### Highlights

- A progress bar that never stands still
- Live log under Details
- Steps with nothing to do are skipped

## v1.2.2

_2026-08-14_

A small patch for the progress bar. It now counts downloads too, and every step says clearly that an update is running, for example **Updates werden heruntergeladen** instead of just "Paketquellen".

## v1.2.1

_2026-08-14_

A quick fix for the new progress bar, which stayed at "0 of 218 items" for the whole run in `v1.2.0`. It now counts every package.

`cachy-auto-update enable` can also switch off CachyOS's **Reboot recommended!** popup, which shows up in the middle of an update. It only asks. To undo it, delete the link in `/etc/pacman.d/hooks`.

## v1.2.0

_2026-08-14_

Welcome to cachy-auto-update `v1.2.0`! Updates now show up on your desktop with a real progress bar, and the notifications around them finally behave.

### 🚨 Breaking changes

- The `NotifyReboot` setting is gone. Its "restart recommended" notice came while the update was still running, which invited a restart halfway through. `cachy-auto-update status` still tells you when a restart is due.

### Highlights

- A progress bar on the desktop (needs `python-gobject`)
- Notifications that stay only when they should

## v1.1.2

_2026-08-08_

Better defaults. The minimum battery level is now 30 % instead of 40 %, and the prompt to silence cachy-update's own "N updates available" notification now defaults to yes. German users can answer it with `j` now, too.

## v1.1.1

_2026-08-08_

The settings screen now reacts instantly. Moving the cursor went from 435 ms to 7.5 ms per key press.

## v1.1.0

_2026-08-08_

Welcome to cachy-auto-update `v1.1.0`! Every option can now be changed from the menu, so nothing needs a text editor any more.

### Highlights

- A settings screen

## v1.0.9

_2026-08-08_

cachy-auto-update now says so before an update starts, and asks you to leave the computer on (`NotifyOnStart`, on by default). The result then replaces that message instead of showing up next to it.

## v1.0.8

_2026-08-08_

An important fix for machines nobody watches. A power cut during an update left pacman's lock file behind, and every later run gave up because of it. A lock from before the last boot is now removed and the update runs again. A lock from the current boot is left alone.

## v1.0.7

_2026-08-08_

The package cache is now trimmed by default. It keeps the last three versions of each package (`KeepOldPackages=3`, like Arch), so it no longer grows to 23 GB. Unused Flatpak runtimes moved to `RemoveOrphans`, which stays off, and the log shows how much space a trim freed.

## v1.0.6

_2026-08-08_

A run started by hand now shows what pacman, the AUR helper and Flatpak are doing, while timer runs stay quiet. A run that gets stopped now shows as "interrupted" in the menu.

## v1.0.5

_2026-08-08_

One package that cannot be updated no longer holds back all the others. It is held back for this run, and everything else updates. On one machine, 213 updates were stuck behind a single package. Held-back packages are shown in the menu and in `cachy-auto-update status`.

## v1.0.4

_2026-08-08_

Switching automatic updates or notifications on or off no longer asks you to press a key. The menu just redraws with the new state.

## v1.0.3

_2026-08-08_

The menu now acts on a single key press, no Enter needed. Enter and the arrow keys can no longer close it by accident.

## v1.0.2

_2026-08-08_

`cachy-auto-update --version` now shows the right version. `v1.0.1` still called itself 1.0.0.

## v1.0.1

_2026-08-08_

The first fixes. pacman's errors were misread on systems that are not in English, "Update now" closed the menu instead of going back to it, and the log was unclear when `checkupdates` is missing.

## v1.0.0

_2026-08-08_

Welcome to the very first release of cachy-auto-update! It keeps CachyOS up to date on its own: pacman, AUR, Flatpak and AppImages, installed in the background with no password prompt and nothing for you to do.

<p align="center">
  <img width="620" alt="The cachy-auto-update menu in Konsole" src="https://raw.githubusercontent.com/LoonixTools/cachy-auto-update/a56e804c41923d7531d578728f43d152ec275caa/res/screenshots/menu.png">
</p>

### Highlights

- Updates at the right moment
- Speaks up only when it needs you
- No stored password
