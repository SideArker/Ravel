# Ravel

A native Rust desktop app for sharing a mouse and keyboard between two paired
devices on a LAN. Both devices can run Linux, both can run Windows, or one can
run each. **Development build: the full two-laptop release
acceptance checks are not yet complete.**

Implemented: egui GUI, mDNS discovery/manual addressing, persistent certificate
fingerprints and revocation, five-minute single-use CPace pairing with explicit
fingerprint approvals, TLS 1.3 input sessions, edge layouts, disconnect cleanup,
and native Windows, X11 and wlroots Wayland input paths.

Rust 1.99+ is required (MSRV 1.99). On Arch:

```sh
sudo pacman -S --needed base-devel pkgconf libx11 libxtst libxkbcommon mesa
export PATH="$HOME/.cargo/bin:$PATH"
rustup toolchain install stable --profile minimal
rustup component add rustfmt clippy
cargo build --locked
cargo run --locked -- --name "Arch laptop"
```

Follow the GUI steps: find the device, exchange a PIN, and compare and approve
fingerprints on both screens. Then configure the handoff edge, allow sharing, and
connect. Keyboard presses and releases follow the pointer after it crosses the
edge. Push outward at the handoff edge to switch; the receiving Ravel window
can return control with one button. The active controller is shown on both
windows. Minimize the window to keep Ravel running in the background; closing it
stops sharing. The GUI has **Stop sharing**; local `ravel --stop` also disables
control. Wayland users must configure the compositor stop binding described in
the operation guide.

Ravel checks GitHub releases when the window opens. If a newer release exists,
it shows a dismissible prompt with a link to that release. You can check again
under **Details & troubleshooting**. The app does not install updates automatically.

## Install a release

Install the latest published x86_64 release without administrator privileges.
Linux needs `curl`, `tar`, `cmp`, and the runtime libraries described in
[packaging](docs/packaging.md). In a terminal:

```sh
curl -fsSL https://raw.githubusercontent.com/Sidearker/Ravel/main/install.sh | sh
```

Run the same command again to upgrade a Ravel copy installed by this script.
It replaces a different binary in the install directory and skips an identical one.

On Windows, run these commands in PowerShell:

```powershell
curl.exe -fsSLo "$env:TEMP\ravel-install.ps1" https://raw.githubusercontent.com/Sidearker/Ravel/main/install.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File "$env:TEMP\ravel-install.ps1"
```

The Windows installer adds Ravel to the current user's PATH; on Linux, the script
prints a PATH hint when `~/.local/bin` is not already present. Releases are
published from `v*` tags. Binary installers currently support Linux x86_64 and
Windows x64.

The first scope is two devices with one selected handoff monitor each and private/link-local
IPv4 networking. Other local monitors remain accessible after handoff.
wlroots-compatible Wayland is implemented; GNOME/KDE portal-only
sessions and macOS are not supported yet. Windows and physical native X11/Wayland behavior still
need two-device certification. No clipboard, files, mobile app, cloud services,
macOS package or autostart are included.

- [Pairing, backends, stop bindings and firewall setup](docs/operation.md)
- [Protocol, limits and threat model](docs/protocol.md)
- [Build and packaging on Arch / Windows](docs/packaging.md)
- [Verification evidence and remaining release gates](docs/verification.md)
- [Backend research and dependency provenance](docs/backend-feasibility.md)
- [Compatibility matrix and expansion plan](docs/compatibility.md)

Validation: `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, and
`cargo test --workspace`. The tests cover security state and geometry; desktop
and multicast integration checks are opt-in. Source is GPL-3.0-or-later because
Ravel reuses pinned lan-mouse capture and input-event code; see `vendor/README.md`.
