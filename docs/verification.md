# Implementation evidence and release gates

Status on 2026-10-29: **development implementation; first usable release acceptance
is incomplete.** The code and build path exist. Native Windows and two physical
laptops were not available, so input interoperability cannot be certified here.

## Coverage against the handoff

| Plan area | Implemented / evidence | Remaining acceptance evidence |
| --- | --- | --- |
| Bootstrap and commits | Rustup stable 1.99.1 installed, MSRV 1.99, Cargo lockfile, incremental scoped commits on existing main branch. | Native Windows clean-checkout build/launch. |
| Backend feasibility | Upstream lan-mouse source examined; upstream X11 capture was a stub. Own X11 backend implemented. Hyprland 0.56.2 constructor probe passed; isolated Xvfb capture/injection check passed; Windows GNU target check and Clippy passed. | Physical Windows hooks/SendInput/DPAPI, native X11 login session, physical Wayland input. Feasibility certification was deferred while implementation continued; source review/cross compilation were not treated as hardware proof. |
| Protocol and security state | Version 2 bounded JSON/TLS framing, threat model, certificate fingerprints, explicit pairing and control authorization, one-session ownership. | Independent review of the CPace/TLS identity binding and all injection paths. |
| Pairing | Five-minute eight-digit single-use PIN, global five-attempt budget and backoff, CPace mutual confirmation bound to TLS exporter and both fingerprints, two explicit GUI approvals, private persisted identity/trust, revoke. Tests use real mutual TLS sockets. | Human pairing exercise on two actual machines, native Windows DPAPI behavior. |
| Discovery/lifecycle | Actual LAN multicast resolution and graceful withdrawal passed; two-second interface checks, auto address updates, manual LAN-only resolution, fingerprint-pinned reconnect, timeouts, congestion/length checks. | Real DHCP reassignment, interface loss/return, multicast-blocked manual connection between laptops. |
| Input transport | All input follows authorized TLS; capture disabled while receiving; held keys/buttons tracked and released; stop disables reconnect. Full control-loop tests cover disconnect, stop, timeout, revocation, invalid input and injection errors; an actual mutual TLS test carries captured input and checks receiver cleanup. Xvfb tested capture, injection, stop chord and cleanup. | Both physical directions across Windows ↔ X11 and Windows ↔ Wayland; modifiers, repeat, wheels, failures and disconnect under load. |
| Geometry | Four placements, reciprocal alignment, proportional handoff mapping, full-desktop motion, nonoverlap rejection, persisted per-peer settings. Unit coverage includes negative origin, selected-monitor handoff and invalid geometry. | Native multi-monitor handoff on Windows, X11 and Wayland; mixed scale/DPI and edge calibration. |
| GUI/packaging | Arch release binary built; native Wayland window mapped and exited cleanly; isolated native X11 GUI screenshot inspected. Build/package documentation and native CI workflow added. | Native Windows GUI and packaged executable. The CI workflow has not been run remotely. |
| Documentation/history | Protocol/threat model, backend provenance, operation, firewall, stop bindings, key storage, revocation, release exclusions and packaging documented. Scoped commit history and tracked file hygiene reviewed. | Release notes must be updated after physical certification. |

## Checks performed

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo test --workspace`: 35 passed, 2 environment-specific checks ignored by
  default; those two were run separately and passed as described below.
- `cargo build --locked` and `cargo build --release --locked`: passed on Arch.
  A separate `git archive HEAD` source export also built with `cargo build
  --locked`, sharing only the compiled dependency cache; it contained no untracked
  project files or local identity state. After the lifecycle follow-up, the full
  35-test suite also passed from a fresh Git source export with the lockfile.
  The stripped Linux release executable is approximately 11.3 MiB.
- `cargo check --target x86_64-pc-windows-gnu --all-targets`: passed. Also ran
  Clippy for that target with `-D warnings`. Used the installed Clang/LLVM tools
  and a temporary extracted official Arch mingw headers sysroot for ring; the
  first attempt failed because a MinGW C compiler was not installed. This does
  not link or run a native MSVC executable.
- `ravel --probe-backend`: passed on the real Hyprland Wayland socket with one
  1920×1080 logical output; no input was injected by this check. A repeated live
  probe exposed a shared keymap-file offset bug: a normal read through a duplicated
  descriptor consumed the compositor-owned offset. Positional reads from offset
  zero fixed it; a regression test verifies offset independence and preservation,
  and consecutive live probes now pass.
- `ravel --smoke-gui --port 0 --state-dir <temporary-directory>`: exit 0. Hyprland
  reported a native Wayland Ravel window. The desktop lock screen prevented a
  useful visible Wayland screenshot. An isolated X11 launch under Xvfb exited 0;
  its screenshot was inspected and main controls/status fit the viewport.
- `cargo test input::native_tests::x11_edge_capture_injection_stop_and_cleanup --
  --ignored`: passed on dedicated Xvfb :99 (1280×720), with WAYLAND_DISPLAY unset,
  XDG_SESSION_TYPE=x11 and RAVEL_TEST_X11=1. It proves capture, synthetic key/button
  injection, stop-chord detection, release of held inputs and ungrabbing on that
  isolated server; it is not a physical X11 desktop certification.
- `cargo test discovery::native_tests::lan_discovery_resolves_and_removes_services
  -- --ignored`: passed on this LAN. The first run failed removal; source tracing
  showed that mDNS shutdown does not withdraw advertisements. Ravel now unregisters
  the service before shutting down, and the unchanged check passed.
- State-directory locking: a second owner is rejected, another directory remains
  usable, dropping the guard permits restart, and Unix tests check private mode
  and rejection of lock-file symlinks. The worker acquires its lock before loading
  identity/trust state; Windows startup failures are displayed in a native dialog
  (cross-compiled, not displayed on native Windows here). A release GUI smoke
  check in isolated X11 also verified duplicate-process refusal, clean restart
  and unchanged identity bytes.
- Session startup: a regression test first reproduced an indefinitely blocked
  display-frame write. The complete exchange now has a five-second deadline and
  stop cancellation; both stalled-write tests pass and verify backend closure.
- Session lifecycle: test-only input adapters run the real event loop and held-input
  tracking. Disconnect, stop, inactivity timeout, malformed frames, active revocation
  and injection failure release held keys/buttons and disarm capture. Leave restores
  local capture. An actual mutual TLS connection transports captured key/button
  events between two session loops and proves receiver cleanup when the sender stops.
  These tests do not simulate the platform APIs or certify physical input behavior.
- Linux CI now defines an explicit Xvfb input lifecycle step in addition to the
  ordinary suite. The same native test passes locally; remote CI has not been run.
- Pairing/injection-gate tests: correct PIN plus both approvals succeeds; wrong,
  expired, reused, throttled and closed offers fail. Unknown, changed, revoked and
  stopped identities fail ordinary authorization. An untrusted input frame cannot
  pass the hello/authorization gate. Separate tests reject exporter/fingerprint
  substitution, invalid frames, NaN/range errors, and conflicting input ownership.

## Explicit boundaries

Implemented Wayland coverage is **wlroots-compatible protocols**, not all Wayland
compositors. GNOME/KDE portal/libei support is not implemented. Missing protocols
and multiple outputs report errors. Windows elevated/secure desktops are excluded.
There is no clipboard/file transfer, cloud relay, IPv6, autostart or installer.

Mixed scale pointer speed is not calibrated on real machines. Wayland receiver
emergency stop requires the documented compositor binding; a generic unfocused
global hotkey is not available to an arbitrary Wayland client. Concurrent app
instances using one state directory are refused by an OS file lock. Manual literal-IP fallback
cannot learn DHCP replacements while multicast remains blocked.

TLS configuration and real TLS tests provide encryption/authentication evidence;
no independent packet-capture confidentiality audit, external security audit,
compositor matrix certification, or two-device sustained input test was performed.
The library's CPace audit does not certify Ravel's application composition. These
are release gates, not claims of completed acceptance.
