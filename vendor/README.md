# Input capture provenance

`input-capture` is a GPL-3.0-or-later source subset from
https://github.com/feschber/lan-mouse at
`69958c625163649b69e27d0a6fb5a3edd596a467` (version 0.4.0).
Original authors: Ferdinand Schober and lan-mouse contributors. The upstream
license is in the root `LICENSE`. This application uses the same license.

Layer-shell, portal/libei, macOS and Windows capture are enabled. Local changes expose output
names and geometry, global edge entry coordinates, and selection of one output
for handoff. The layer surface is renamed, and the pinned input-event dependency
is used. X11 capture is implemented separately because upstream's X11 capture
is a stub.

The portal and macOS modules were imported from the same pinned upstream revision.
The portal module now reports zones, respects the selected monitor, checks rejected
barriers and handles version 2's Start flow. The macOS module reports the entry
point and uses the selected monitor. Native QA is still required. No dummy
backend is selected by Ravel.

Further local hardening: require shortcut inhibition; reject capture queue
congestion; ignore injected Windows events; remove key/cursor event logging;
use Tokio's explicit I/O-safety registration contract; detect the target rather
than build host in build.rs; wait for Windows hook installation before reporting
readiness, propagate startup errors and unhook on thread exit. Native Windows
runtime validation is still required despite passing cross compilation.
