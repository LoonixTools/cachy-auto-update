<p align="center">
  <img width="200" src="res/cachy-auto-update.svg" alt="cachy-auto-update">
</p>

<h1 align="center">cachy-auto-update</h1>

<h3 align="center">Unattended background updates for CachyOS.</h3>

<p align="center">
  Supports pacman, AUR, Flatpak and AppImages.
</p>

<h5 align="center">
  <a href="#usage">How to use</a> |
  <a href="#install">Install</a> |
  <a href="https://github.com/LoonixTools/cachy-auto-update/issues">Report a bug</a>
</h5>

<p align="center">
  <a href="https://buymeacoffee.com/felitendo"><img src="https://cdn.buymeacoffee.com/buttons/v2/default-yellow.png" alt="Buy Me A Coffee" height="48"></a>
</p>

Built for the machine you set up for somebody else and would rather not maintain.
Stays out of the way while they are gaming or on battery, never touches the
package database while `pacman` is already running, and only bothers you if something
went wrong.

## Install

```bash
paru -S cachy-auto-update
sudo cachy-auto-update enable
```

Updates are **off** until you enable them.

## Usage

```bash
sudo cachy-auto-update
```

```
  CachyOS Auto-Update

  Automatic updates            ON
  Notifications                ON

  Last check                   3 hours ago
  Last successful update       Sat 08 Aug 2026 04:12:03 CEST (23 packages)
  Next scheduled run           Sat 08 Aug 2026 05:00:00 CEST

  [1] Toggle automatic updates
  [2] Toggle notifications
  [3] Update now
  [4] Show log
  [5] Show current conditions
  [6] Settings
  [q] Quit
```

Everything is configurable from **[6] Settings**.
The interface is also translated to German (more langs coming soon (maybe)).

## What it updates

| | |
|---|---|
| Repository packages | `pacman -Syu` |
| AUR | `paru` or `yay`, whichever is installed |
| Flatpak | system and per-user installations |
| AppImages | via [Gear Lever](https://github.com/mijorus/gearlever), if installed |


## When it doesn't update

A run is postponed (retried an hour later) when:

- battery is below 30 % (ignored on mains power),
- a game is running,
- pacman's database is locked or another package manager is running.

The machine will obviously **never** automatically restart (this is not windows)

## Safety measures

- A **progress bar** shows which step is running and which package is being updated.
- **Suspend and shutdown are blocked** via `systemd-inhibit` during updates.
- A `db.lck` left by a crash is removed on the next run.
- On Btrfs with `snapper`/`snap-pac` (the CachyOS default), every transaction
  gets a pre/post snapshot.

## Package conflicts

`AutoResolveConflicts=yes` (default) retries conflicting package replacements
automatically.

File conflicts (`exists in filesystem`) are **not** force-overwritten.
Signature failures trigger one keyring refresh and one retry.

## Logs

```bash
cachy-auto-update log        # last run
cachy-auto-update log -a     # rolling log
journalctl -u cachy-auto-update
```

## Building from source

```bash
make && sudo make install
sudo systemd-sysusers && sudo systemd-tmpfiles --create
sudo cachy-auto-update enable
```

`make check` runs shellcheck, syntax checks, and validates the sudoers drop-in.

Optional runtime dependencies (degrades gracefully without them):
`pacman-contrib`, an AUR helper, `flatpak`, Gear Lever, `libnotify`,
`python-gobject`.

## Relationship to cachy-update

[cachy-update](https://github.com/CachyOS/cachy-update) is CachyOS's
interactive updater which checks for updates and notifies, but that needs manual intervention.
This is the unattended counterpart. The two can coexist; `enable` offers
to switch off cachy-update's notification since it becomes obsolete.

## License

GPL-3.0-or-later.
