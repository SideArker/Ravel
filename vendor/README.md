# Input capture provenance

`input-capture` is a GPL-3.0-or-later source subset from
https://github.com/feschber/lan-mouse at
`69958c625163649b69e27d0a6fb5a3edd596a467` (version 0.4.0).
Original authors: Ferdinand Schober and lan-mouse contributors. The upstream
license is in the root `LICENSE`. This application uses the same license.

Only layer-shell and Windows capture are enabled. Local changes expose the edge
entry coordinates and single-output logical bounds for proportional handoff,
rename the layer surface, and use the pinned input-event dependency. X11 capture
is implemented separately because upstream's X11 capture is a stub.

The subset is one dependency import; subsequent local changes are reviewed as
normal small commits. Do not enable excluded backends without importing their
sources and verifying their behavior. No dummy backend is selected by Ravel.

Further local hardening: require shortcut inhibition; reject capture queue
congestion; ignore injected Windows events; remove key/cursor event logging;
use Tokio's explicit I/O-safety registration contract; detect the target rather
than build host in build.rs; wait for Windows hook installation before reporting
readiness, propagate startup errors and unhook on thread exit. Native Windows
runtime validation is still required despite passing cross compilation.
