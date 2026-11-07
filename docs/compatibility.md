# Compatibility plan

Ravel's TLS protocol does not require a particular operating system on either
side. Each endpoint needs a working local capture and injection backend. Two
Linux devices or two Windows devices can connect just as a Linux and Windows
pair can. Protocol version 3 requires both devices to update together.

| Local desktop | Current backend | Status |
| --- | --- | --- |
| Hyprland/Sway and other wlroots-compatible Wayland compositors | Layer-shell edge capture and wlroots virtual input | Implemented; physical two-device validation pending. |
| Native X11 | X11 grabs and XTest | Implemented; isolated Xvfb tested, physical desktop validation pending. |
| Windows standard desktop | Low-level hooks and SendInput | Implemented; native two-device validation pending. Elevated and secure desktops are excluded. |
| GNOME/KDE Plasma Wayland | InputCapture and RemoteDesktop portals with libei/EIS | Code path added; GNOME and Plasma native validation pending. |
| macOS | Core Graphics capture and injection | Code path added; native build, signing and real-Mac validation pending. |

## Backend boundaries

Keep session ownership, TLS, pairing, geometry, and the wire input format shared.
Select capture and injection APIs by capability, not by a hard-coded compositor
name. The `Backend` in `src/input/mod.rs` separates native X11, wlroots Wayland,
Windows, portal capture/injection, and macOS paths. The portal variants have
their own permission and lifecycle rules.
Do not route an unsupported Wayland session through Xwayland or a dummy backend.

For GNOME/KDE, the [InputCapture portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.InputCapture.html)
for source edge barriers and libei events, then the
[RemoteDesktop portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html)
with ConnectToEIS for receiving input are wired into separate capture and injection
variants. Ravel probes portal versions and capabilities at runtime; the GUI shows
permission failures returned by the compositor. The portal
spec says the compositor alone activates capture at a barrier; a Ravel button
cannot force immediate capture. Test permission denial, zone changes, keyboard
layout, release, and reconnect on both GNOME and Plasma before calling either
desktop supported.

For macOS, a native capture/injection variant uses
[Core Graphics event taps and posting](https://developer.apple.com/documentation/coregraphics/cgevent).
Mac virtual key codes map to Ravel's wire scancodes and back. Accessibility,
Input Monitoring and Input Control denial returns an error instead of a
zero-event backend. The package script requires a Developer ID signing identity;
a signed build has not run. Test edge capture,
modifiers, layouts, repeat, stop, sleep/wake, and release on real Macs.

## Release gates

1. Keep the protocol and session tests platform neutral, including control
   return and held-key cleanup.
2. Implement the portal backend and run GNOME and Plasma tests in both sender
   and receiver roles.
3. Implement the macOS backend, native build, permissions, and key mapping tests.
4. Exercise each backend in both directions against another machine, including
   same-OS pairs and at least one mixed-OS pair. Record which compositor and OS
   versions passed; a build alone is not interoperability evidence.

The animated handoff indicator currently lives inside each Ravel window. A
desktop-wide edge overlay would require separate overlay surfaces/windows on
each backend and must not take focus or interfere with emergency stop.
