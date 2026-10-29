# Ravel

A native Rust desktop app for sharing a mouse and keyboard between two paired
Windows / Linux devices on a LAN. **Development build: the full two-laptop release
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

Use **Check input backend**, pair with a PIN, compare and approve fingerprints on
both screens, configure the peer's edge, then allow sharing and connect. The GUI
has **Stop / Disconnect**; local `ravel --stop` also disables control. Wayland users
must configure the compositor stop binding described in the operation guide.

The first scope is two devices with one active monitor each and private/link-local
IPv4 networking. wlroots-compatible Wayland is implemented; GNOME/KDE portal-only
sessions are unsupported. Windows and physical native X11/Wayland behavior still
need two-device certification. No clipboard, files, mobile app, cloud services,
installers or autostart are included.

- [Pairing, backends, stop bindings and firewall setup](docs/operation.md)
- [Protocol, limits and threat model](docs/protocol.md)
- [Build and packaging on Arch / Windows](docs/packaging.md)
- [Verification evidence and remaining release gates](docs/verification.md)
- [Backend research and dependency provenance](docs/backend-feasibility.md)

Validation: `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, and
`cargo test --workspace`. The tests cover security state and geometry; desktop
and multicast integration checks are opt-in. Source is GPL-3.0-or-later because
Ravel reuses pinned lan-mouse capture and input-event code; see `vendor/README.md`.
