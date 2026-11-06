# Running and pairing Ravel

Ravel currently implements one selected handoff monitor per device and one active control
connection. **This is a development build, not a certified two-laptop release.**
See [verification](verification.md) for exactly what has been exercised.

## First connection

1. Run `ravel --name "Arch laptop"` and `ravel --name "Windows laptop"` on the
   same trusted LAN. The default TCP port is 4243; `--port` changes it.
2. Choose **Find monitors / check input** on each device. If there are multiple active
   monitors, select the monitor whose outer edge should connect to the peer.
   Fix any reported missing protocol, display, or permission issue before sharing.
3. On one machine select **Show PIN on this device**. On the other select the
   discovered peer (or enter `192.168.x.y:4243`), enter that eight-digit PIN, and
   choose **Pair using PIN**.
4. Compare the full peer fingerprint shown in each approval dialog against the
   fingerprint under **Details & troubleshooting** on the other screen. Approve
   on **both** devices only when they match. A discovery name or fingerprint hint
   is not proof.
5. Select the paired device and configure Left/Right/Top/Bottom under
   **Handoff edge**. Adjust **Fine tune alignment** if needed and choose
   **Save handoff edge**. Then choose **Allow incoming control** on the receiving
   device and **Connect to device** on the initiating device. Connecting also
   allows sharing locally.
6. Move through the configured edge. Both devices can be the sender; local
   capture is disabled while receiving. Crossing back returns control.

A positive alignment shifts the peer down along a vertical edge, or right along
a horizontal edge. Values are fractions of the edge length, so 0.25 is 25%.
Handoff position is proportional between the selected monitors' logical sizes;
subsequent motion spans the receiving device's desktop. Nonoverlapping
edge segments do not transfer. The initiating device's saved layout determines
that connection, and the receiver saves the reciprocal layout. Changes apply on
reconnection. Mixed scales, rotation, and physical pointer speed require native
calibration; they have not been certified. The OS continues to handle movement
between local monitors. Ravel captures only the selected monitor's handoff edge;
the selected monitor must be at the outer edge in the chosen direction. A monitor
change takes effect on the next connection.

## Stop and revoke

**Stop sharing** disables incoming control, closes sessions, cancels PINs,
releases held keys/buttons, and cancels automatic reconnect. Closing the window
also stops sharing. Ctrl+Alt+Shift+Esc stops while sending; Windows and native X11
also poll the chord while receiving. Local `ravel --stop` is an independent path
that the worker checks every 100 ms. Use the same `--state-dir` if one was supplied.
Do not kill the process to stop normal sharing: OS-level abrupt termination cannot
guarantee release delivery.

Wayland does not permit an arbitrary app to monitor all local keys. Configure a
compositor binding to `ravel --stop` so the receiving user has an immediate stop
hotkey even when Ravel is unfocused. For Hyprland, using the installed absolute
binary path:

```ini
bind = CTRL ALT SHIFT, Escape, exec, /absolute/path/to/ravel --stop
```

For Sway:

```text
bindsym Ctrl+Mod1+Shift+Escape exec /absolute/path/to/ravel --stop
```

The in-capture chord remains available when shortcut inhibition is active. Verify
both local stop paths before sharing passwords or using an unattended receiver.

**Unpair selected device** under **Details & troubleshooting** removes the
selected certificate fingerprint and stops all
sharing. Revoking on one side prevents reconnection. Re-pair explicitly with a
new PIN and approvals on both devices if needed. A different fingerprint never
inherits trust from a matching name or address.

## Backend setup and limits

| Session | Implemented path | Setup and boundary |
| --- | --- | --- |
| Hyprland / Sway-class Wayland | layer-shell capture; wlroots virtual keyboard/pointer | Needs layer-shell v3+, relative-pointer, pointer-constraints, xdg-output, shortcuts-inhibit, virtual-keyboard, virtual-pointer, seat v7+, local XKB keymap. Native constructor tested on Hyprland 0.56.2. Full physical input/handoff not yet certified. |
| Native X11 | X11 grabs; XTest injection; RandR geometry | Requires XTest and RandR 1.5 and a usable DISPLAY/Xauthority. Multi-monitor handoff requires native testing; isolated Xvfb covers one monitor. Xwayland is deliberately rejected. |
| Windows | low-level hooks; SendInput | Multi-monitor handoff requires native Windows validation. Standard desktop only; elevated apps and UAC/secure desktops are unsupported. Windows key repeat currently uses a fixed 500 ms delay/32 ms interval. |
| GNOME/KDE Wayland without the selected protocols | unavailable | No portal/libei adapter in this build. Reported backend failure; no Xwayland/dummy fallback. |

No root/input group access is required by the chosen Linux backends. Wayland
protocol availability is controlled by the compositor; there is no portal prompt
or portal permission override in this implementation. A missing or refused
backend is an error, not a supported zero-event backend. The GUI stays usable for
pairing and diagnostics even when a backend is unavailable.

## Discovery, reconnect and firewall

mDNS/DNS-SD uses UDP 5353 multicast (`_ravel._tcp.local.`). It announces a public
fingerprint hint, name and service port. The library checks local addresses every
two seconds and reannounces changes; the browser verifies known services every
15 seconds. Graceful shutdown withdraws the advertised service. Network loss also
closes a control session after four seconds without inbound frames, releasing held
input. Explicit connections retry every three seconds while sharing stays allowed,
using newly discovered addresses for the selected fingerprint. Stop cancels this.

Allow TCP 4243 (or the configured port) and UDP 5353 on the **private LAN profile /
interface only** in the existing firewall. Ravel never changes firewall rules or
opens router ports. Manual host:port entry works when multicast is blocked, but a
literal IP cannot discover its replacement if multicast remains unavailable.
IPv4 private/link-local source and local endpoint checks reject public-interface
connections. IPv6, loopback in normal operation, public Internet, CGNAT addressing
and routed/cloud relays are outside this build's supported network scope.

mDNS can be blocked by Wi-Fi client isolation, VPN routing or multicast filtering.
The GUI reports discovery errors and retains manual entry. A spoofed announcement
can cause a failed connection, but it cannot authorize input.

## State and identity

Default state is the per-user data-local directory resolved by `directories`:
Linux normally `$XDG_DATA_HOME/ravel` or `~/.local/share/ravel`; Windows normally
`%LOCALAPPDATA%\Ravel\Ravel\data`. `--state-dir PATH` selects an explicit directory.
The directory contains `identity.json`, `peers.json`, `instance.lock` and transient
`stop.request`. One process owns the directory at a time, using an OS file lock
acquired before loading identity or trust state. A second instance is refused.
Closing or crashing releases the lock automatically; the empty lock file remains
and must not be deleted while the application is running.
Linux enforces directory 0700 and files 0600; overly public files are refused.
The private identity key is additionally protected with current-user DPAPI on
Windows. Use separate state directories only when separate device identities are intended.

The PIN is single-use, lives only in memory, and expires five minutes after the
receiver generates it. All admitted exchanges count toward a global five-attempt
limit, including abandoned exchanges. Backoff is 1/2/4/8/16 seconds, with one PIN
exchange admitted at a time. A local user may explicitly issue a fresh offer.
Expiry, cancellation or replacement invalidates a pending authorization. Restart
closes the offer. Losing/corrupting identity state is an error; deleting it manually
creates a new identity that must be paired again. Keep backups private.

The PIN runs audited-library CPace bound to both identities and the TLS session;
it is never an encryption key or a substitute for fingerprint comparison. Read
[the protocol/threat model](protocol.md) and [release evidence](verification.md).
