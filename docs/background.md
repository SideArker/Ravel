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

Start Ravel after the user logs in, when the desktop session, tray and input
permissions are available. On Linux, add this user autostart entry at
`~/.config/autostart/ravel.desktop`, replacing the path with the installed
executable's absolute path:

```ini
[Desktop Entry]
Type=Application
Name=Ravel
Exec=/absolute/path/to/ravel
Terminal=false
X-GNOME-Autostart-enabled=true
```

On Windows, place a shortcut to `ravel.exe` in the current user's Startup
folder. On macOS, add the packaged Ravel app to Login Items. Ravel does not
install these entries automatically.

On Linux, reversible window hiding requires X11 or Xwayland and a working tray.
Ravel uses Xwayland for its GUI when available, even if its input backend uses
Wayland. In a pure Wayland session without Xwayland, the GUI remains visible;
the network worker still runs. Start-on-login behavior and tray integration
need a native check on each target desktop.
