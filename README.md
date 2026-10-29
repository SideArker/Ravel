# Ravel

A native Rust desktop app for sharing a mouse and keyboard between two paired
Windows / Linux devices on a LAN. **In development; native two-device acceptance
checks are required before calling this a usable release.**

Build with Rust 1.99 or newer (MSRV 1.99):

```sh
rustup toolchain install stable --profile minimal
rustup component add rustfmt clippy
cargo build --locked
cargo run --locked
```

Linux build prerequisites: a C compiler, pkg-config, X11/XTest and xkbcommon
libraries, and a working Wayland or X11 desktop. On Arch:
`sudo pacman -S --needed base-devel pkgconf libx11 libxtst libxkbcommon mesa`.
Windows uses the MSVC Rust target and Visual Studio Build Tools with C++ support.

The first implementation targets two devices, one contiguous display surface per
device, wlroots-compatible Wayland, native X11, and Windows. Unsupported backends
must fail closed. Clipboard, files, cloud services, installers and autostart are
outside this release. Small coherent commits are made throughout development.
