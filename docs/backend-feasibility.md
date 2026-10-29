# Backend feasibility — 2026-10-29

## Evidence collected

The initially empty repository has no existing integration to preserve. Host:
Arch Linux x86_64, Hyprland 0.56.2, Wayland socket available, Xwayland DISPLAY=:1,
one 1920×1080 output at scale 1. Native X11 and Windows sessions are unavailable.
Rust 1.99.1 stable installed with rustup; system Rust is also 1.99.1.

Reviewed upstream [lan-mouse](https://github.com/feschber/lan-mouse) at
69958c625163649b69e27d0a6fb5a3edd596a467, specifically input-capture/src/{lib,
layer_shell,windows,x11}.rs and input-emulation/src/{lib,wlroots,windows}.rs.
X11 capture explicitly returns NotImplemented. Wayland layer-shell requires
layer-shell, relative-pointer, pointer-constraints, xdg-output and a keyboard
seat; shortcuts-inhibit is necessary to suppress compositor bindings. Wlroots
injection uses virtual-pointer and virtual-keyboard, with a local XKB keymap.
Windows uses low-level keyboard/mouse hooks and SendInput. Input-event provides
Linux/Windows scancode translation. This is dependency evidence, not native QA.

## Selected paths

- Wayland: layer-shell capture plus wlroots virtual input; Hyprland/Sway-class
  compositors exposing all required protocols. Missing protocols must report an
  error, never fall through to Xwayland or a dummy backend. GNOME and KDE portal
  support is excluded from this initial backend implementation; these desktops
  must report unsupported rather than imply Wayland-wide support.
- X11: native X11 pointer/keyboard grabs, edge polling and XTest injection. Detect
  Wayland first so Xwayland is never mistaken for native X11.
- Windows: upstream low-level capture, native SendInput injection, injected-event
  filtering and native cursor positioning. Standard-user apps only: UIPI blocks
  elevated windows; secure desktops/UAC are unsupported. Native verification is
  still required.

Single contiguous output per device is the first geometry scope. Mixed DPI is
represented through logical display sizes. Multi-monitor gaps and rotation must
not silently be treated as a supported display surface.

## Required native evidence

For each target: prove acquire/release, permission/denial handling, edge position,
mouse, modifiers, key repeat, wheel, stop hotkey and held-input cleanup. Two native
machines are needed for both directions and network-change validation. A build
or successful backend constructor alone is not proof of input integration.

## Cryptography selection

Use [1Password's CPace implementation](https://github.com/1Password/cpace), pinned
to 7e2e42c7d359969f4356292c2bd2cf3ff6eeeecc. It reports a Cure53 audit and supplies
mutual key confirmation. It implements draft revision 6, not a claim of final RFC
interoperability. Use its channel identifier context for both certificate
fingerprints and the TLS exporter. Rustls TLS 1.3 supplies record encryption,
replay protection and certificate proof of possession. The application binding
and authorization state machine still require independent security review;
the library's audit does not certify Ravel's composition.
