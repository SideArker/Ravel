# Build and package

The initial packaging format is a native release binary plus documentation. There
is no installer, service, autostart integration or code signing in this release.
Do not distribute the state directory, identity keys, PINs, logs or `target` tree.

## Arch Linux

```sh
sudo pacman -S --needed base-devel pkgconf libx11 libxtst libxkbcommon mesa
export PATH="$HOME/.cargo/bin:$PATH"
rustup toolchain install stable --profile minimal
rustup component add clippy rustfmt
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release --locked
./target/release/ravel --probe-backend
./target/release/ravel
```

Runtime needs the native GUI/graphics stack, X11 libraries for the X11 path, and
libxkbcommon. `ldd target/release/ravel` identifies the binary's actual linked
libraries on the build machine. Build on Arch for the Arch package; a binary
built on a different glibc distribution is not an Arch compatibility test.

For a portable source-tree package, copy `target/release/ravel`, `README.md`,
`LICENSE` and `docs/` into a staging directory and archive it as a tarball. Preserve
execute permission on the binary. Install it to a chosen user binary directory,
then point the compositor stop binding at that absolute path. Source distribution
must include the Cargo lockfile and GPL dependency provenance in `vendor/`.
A real PKGBUILD should be added after the repository release URL is established.

## Windows

On native Windows, install Visual Studio Build Tools with the C++ desktop tools
and Windows SDK, then install stable Rust using rustup (MSVC default target).
In a Developer PowerShell:

```powershell
rustup component add rustfmt clippy
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release --locked --target x86_64-pc-windows-msvc
.\target\x86_64-pc-windows-msvc\release\ravel.exe
```

ZIP the executable with README, LICENSE and docs. Use a per-user extraction path;
run without administrator elevation. Permit the executable only on the firewall's
private network profile. Test SendInput failure against elevated/secure windows;
do not bypass UIPI by recommending elevation of Ravel.

The Linux-side `cargo check --target x86_64-pc-windows-gnu --all-targets` uses a
cross C compiler/sysroot for ring. It checks conditional Rust code and API usage,
but does not link an MSVC executable or prove DPAPI, hooks, GUI startup or physical
input. The CI workflow defines native Windows jobs; merely adding that workflow
is not evidence that it ran.

## Isolated integration checks

The ordinary test suite requires no desktop or multicast. Explicit environment
checks are ignored by default:

```sh
cargo test discovery::native_tests::lan_discovery_resolves_and_removes_services -- --ignored
# Run only with a dedicated Xvfb server on :99, never the user's working display:
env -u WAYLAND_DISPLAY XDG_SESSION_TYPE=x11 DISPLAY=:99 RAVEL_TEST_X11=1 \
  cargo test input::native_tests::x11_edge_capture_injection_stop_and_cleanup -- --ignored
```

The mDNS check advertises two temporary services and verifies both resolution and
withdrawal. The X11 check synthesizes input on the isolated display, validates edge
capture and stop detection, and checks that keys/buttons/grabs are released. Both
need separate two-device/native desktop checks before release.
