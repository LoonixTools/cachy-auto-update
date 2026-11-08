<p align="center">
  <img width="200" src="res/cachy-auto-update.svg" alt="cachy-auto-update">
</p>

<h1 align="center">cachy-auto-update</h1>

<h3 align="center">Unattended background updates for CachyOS.</h3>

<p align="center">
  pacman, AUR, Flatpak and AppImages. In the background, with no password prompt.
</p>

<h5 align="center">
  <a href="#install">Install</a> |
  <a href="#how-to-use">How to use</a> |
  <a href="https://github.com/LoonixTools/cachy-auto-update/issues">Report a bug</a>
</h5>

<p align="center">
  <a href="https://ko-fi.com/felitendo"><img src="https://storage.ko-fi.com/cdn/kofi5.png?v=6" alt="Buy me a coffee on Ko-fi" height="48"></a>
</p>

## Install

1. Install the package from the AUR:

   ```bash
   paru -S cachy-auto-update
   ```

   paru first asks to show the build files. Press **q** to close them, then **Enter** to install.
   No paru yet? Get it with `sudo pacman -S paru`.

2. Turn automatic updates on:

   ```bash
   sudo cachy-auto-update enable
   ```

## How to use

```bash
sudo cachy-auto-update
```

<p align="center">
  <img src="res/screenshots/menu.png" alt="The cachy-auto-update menu in Konsole: automatic updates and notifications on" width="620">
</p>

Press **1** to turn automatic updates on or off. **6** has all the settings.

Updates run once a week. To change that, use **How often to update** in the settings.

### Tray icon

An icon in the system tray shows if updates are waiting. Click it to install them all now, no password needed.
Right-click it for **Show log**, **Check for updates** and **Settings**.

<p align="center">
  <img src="res/screenshots/tray.png" alt="The tray icon's menu with Update now, Show log, Check for updates and Settings" width="420">
</p>

Do not want it? Turn off **Show an icon in the system tray** in the settings.

While an update runs, a progress bar shows how far it is. You can **Pause** or **Cancel** it there.

<p align="center">
  <img src="res/screenshots/progress.png" alt="The progress bar of a running update, with Pause and Cancel buttons" width="600">
</p>

To keep it out of the way, turn on **Start the progress bar minimized** in the settings. The bar then stays in the
notification area and does not pop up. Click the notification icon to see it.

<p align="center">
  <img src="res/screenshots/settings.png" alt="The settings screen in Konsole, with Start the progress bar minimized turned on" width="620">
</p>

## When does it update?

| | |
|---|---|
| Once a week | ✅ Updates (change it in the settings) |
| Battery below 30 % | ⏸️ Waits (on power it runs) |
| A game is running | ⏸️ Waits |
| pacman, yay or paru is running | ⏸️ Waits |
| Shutdown or suspend during an update | 🚫 Blocked until it is done, or until you click **Cancel** |
| Restart after an update | ❌ Never on its own, you get a notification |

## More

<details>
<summary>What it updates</summary>

| | |
|---|---|
| Packages | `pacman -Syu` |
| AUR | `paru` or `yay` |
| Flatpak | system and user installs |
| AppImages | through [Gear Lever](https://github.com/mijorus/gearlever) |

</details>

<details>
<summary>Safety</summary>

- A progress bar shows the step and the package.
- **Cancel** and **Pause** on the progress bar never stop pacman while it installs. They wait until that part is done.
- On Btrfs with snapper (the CachyOS default) every update gets a snapshot before and after.
- A lock file left by a power cut is removed on the next run.
- Package replacements are handled on their own. File conflicts are never overwritten.
- A signature error refreshes the keyring once and tries again.
- Old package versions and AUR build leftovers are cleaned up, so the caches do not fill the disk.

</details>

<details>
<summary>Logs</summary>

```bash
cachy-auto-update log        # last run
cachy-auto-update log -a     # all runs
journalctl -u cachy-auto-update
```

</details>

<details>
<summary>cachy-update</summary>

[cachy-update](https://github.com/CachyOS/cachy-update) tells you about updates, you install them.
This installs them for you. Both work side by side, and `enable` offers to silence
cachy-update's notification.

</details>

<details>
<summary>Build from source</summary>

```bash
make && sudo make install
sudo systemd-sysusers && sudo systemd-tmpfiles --create
sudo cachy-auto-update enable
```

Optional: `pacman-contrib`, an AUR helper, `flatpak`, Gear Lever, `libnotify`, `python-gobject`.

</details>

GPL-3.0-or-later.
