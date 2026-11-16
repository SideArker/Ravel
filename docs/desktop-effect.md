# Desktop crossover effect design

The existing handoff progress bar is inside the Ravel window. A visible effect
over other applications needs a separate desktop overlay. It must be optional:
input transfer, cleanup and the kill switch cannot depend on rendering.

The first effect should be a short, click-through "break-in" at the selected
monitor edge: a bright seam appears at the cursor, opens as outward motion
fills the existing 48-unit gate, then expands and fades after Enter. On the
receiving device, a shorter seam appears where the pointer arrives. Reset or
timeout fades the seam without completing it. Keep the effect under 300 ms
after crossing and avoid large opaque areas.

Implementation needs the handoff feedback to include the selected monitor,
edge side and entry point. Today `Feedback::Handoff` carries only progress and
an incoming flag, so a renderer cannot place the effect correctly. The event
can remain local; no new network frame or peer permission is needed. A bounded,
lossy render queue should drop old progress frames, while Stop and backend
cleanup retain priority.

For wlroots-compatible Wayland compositors, use a separate `wlr-layer-shell`
overlay surface on the selected output. Set its input region to an empty region,
keyboard interactivity to none and exclusive zone to -1, so it does not steal
mouse or keyboard input or reserve screen space. Draw transparent ARGB frames
only during the effect and destroy the surface on stop/disconnect. The
[layer-shell protocol](https://wayland.app/protocols/wlr-layer-shell-unstable-v1)
defines these controls. Hyprland also exposes
[layer rules](https://wiki.hypr.land/configuring/core/rules/layer-rules/)
for optional compositor animation of a named overlay layer. A native compositor
plugin is unnecessary for the first version.

Windows and macOS need their own overlay backends and native checks. The
GNOME/KDE portal input backend does not itself provide a general desktop
overlay API, so its animation needs a separate supported surface mechanism or
an in-window fallback. An overlay must be verified against fullscreen windows,
mixed monitor scale, lock screens, screen sharing, and input focus before it is
enabled by default on any platform.
