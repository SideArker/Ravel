# Running Ravel in the background

Run `ravel` normally. It starts one detached process for the current state
directory and returns control to the terminal. The same process owns the network
worker, tray icon and optional GUI. Running `ravel` again opens the existing GUI
instead of starting another instance. Errors from the detached process go to
`daemon.log` in the state directory.

On startup Ravel tries to reconnect automatically when exactly one paired device
is saved. The GUI stays open until a connection is established. Once connected,
Ravel hides the GUI in the system tray; if that session disconnects, the GUI
opens again. With no saved device, or with several saved devices, use the GUI to
choose one. The **Minimize to background** button and the window close button
also hide the GUI. Click the tray icon, choose **Open Ravel**, or run `ravel`
again to open it.

The header's **Stop sharing** button or `ravel --stop` stops control without exiting.
It becomes **Start sharing**, which resumes sharing and connection attempts.
The **Kill switch** button at the bottom of the window, tray **Quit Ravel and stop sharing**, or
`ravel --kill-all` sends Stop to active sessions and exits the process. If a
custom `--state-dir` was used to start Ravel, use it with these commands too.

## Start on login

Use **Settings → Startup, tray & updates → Enable autostart**, or:

```sh
ravel autostart enable
ravel autostart status
ravel autostart disable
```

On Linux this installs `ravel.service` in the systemd user configuration and
enables it under `graphical-session.target`. It uses the installed `ravel`
command with `--foreground`, so systemd tracks the actual process. Enabling
startup does not start a duplicate or stop current sharing. A previous
`autostart/ravel.desktop` entry is removed after successful migration.
Inspect startup with `systemctl --user status ravel.service` and
`journalctl --user -u ravel.service -b`.

The desktop must activate `graphical-session.target` and import its display
environment into the user manager. This host's Hyprland session already does
both. A custom session that does neither needs those session integration steps;
a service launched before the graphical session cannot access its Wayland socket.

On Windows the setting manages a shortcut in the user's Startup folder. On
macOS it manages a LaunchAgent. Autostart is optional on every platform.

On Linux, reversible window hiding requires X11 or Xwayland and a working tray.
Ravel uses Xwayland for its GUI when available, even if its input backend uses
Wayland. In a pure Wayland session without Xwayland, the GUI remains visible;
the network worker still runs. Start-on-login behavior and tray integration
need a native check on each target desktop.
