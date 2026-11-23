<p align="center">
  <img src="assets/logo.svg" alt="Ravel Logo" width="112" height="112" />
</p>

<h1 align="center">Ravel</h1>

<p align="center">
  <strong>Weave your devices together — fast, secure LAN mouse &amp; keyboard sharing for Linux, Windows, and macOS.</strong>
</p>

---

**Ravel** is a native Rust desktop application that lets you seamlessly share one mouse and keyboard across multiple computers on your local network. Move your pointer to the edge of your screen and push through to control another device — across Linux (Wayland & X11), Windows, and macOS.

## Features

- **Multi-device connections:** Connect and switch between multiple paired devices simultaneously (e.g., a laptop on your left and another machine on your right), with live per-device connection status.
- **Interactive 2D screen layout:** Grab and drag your devices' boxes around your screen to position them on any edge (`Left`, `Right`, `Top`, `Bottom`) with custom alignment and live handoff updates — no reconnect required.
- **Zero-trust LAN pairing:** Uses TLS 1.3 with persistent ECDSA P-256 device certificates, single-use 8-digit CPace PAKE PINs, and mutual fingerprint verification.
- **Single-instance background daemon & tray:** Runs as a single background instance. Closing the window (`✕`) minimizes Ravel to the system tray (and you can even hide the tray icon and summon Ravel anytime from your system app launcher).
- **Login autostart & automatic updates:** Optional one-click login autostart prompt on first launch, automatic hourly release checks, and built-in self-updating.
- **Cross-platform input backends:**
  - **Linux Wayland (wlroots / Hyprland / Sway):** `wlr-layer-shell` edge capture + virtual keyboard & pointer.
  - **Linux Wayland (GNOME / KDE Plasma):** XDG `InputCapture` + `RemoteDesktop` (libei / EIS) portals.
  - **Linux X11:** Native X11 grabs + `XTest` + `RandR` monitor geometry.
  - **Windows:** Raw Input mouse motion, low-level button/keyboard hooks, and `SendInput`.
  - **macOS:** Core Graphics event taps + event posting.

---

## Why Antivirus / Windows Defender May Flag Ravel ("RAT" / False Positive)

On Windows (and occasionally other heuristic security scanners), `ravel.exe` may be flagged as a **RAT (Remote Access Trojan)**, **Keylogger**, or **PUA / Unknown Publisher**. **This is a false positive.**

### Why does this happen?
1. **Global input APIs:** To capture your mouse and keyboard when you cross the screen edge on one PC and move the cursor/type on another PC, Ravel uses Windows Raw Input, `SetWindowsHookExW(WH_KEYBOARD_LL / WH_MOUSE_LL)`, and `SendInput`. Heuristic antivirus engines flag programs that combine keyboard hooks with network sockets because remote administration tools use those same OS APIs.
2. **Unsigned community binaries:** Commercial Extended Validation (EV) code-signing certificates cost hundreds of dollars per year. Because Ravel's prebuilt release binaries are currently unsigned, Windows SmartScreen and Defender apply aggressive heuristic rules by default.

### How Ravel is architected for safety (and why it is NOT a RAT)
- **No remote shell, file access, or screen capture:** Ravel's protocol ([`src/protocol.rs`](src/protocol.rs)) can only transmit pointer motion, mouse buttons, scroll, and keyboard scancodes. There is no command execution, no file transfer, no clipboard reading, and no screen recording.
- **Private LAN IPv4 only:** Public Internet connections are rejected at the socket level ([`src/discovery.rs`](src/discovery.rs)). Ravel never contacts external servers (except GitHub's release API to check for updates).
- **Explicit pairing + mutual fingerprint approval:** No device can send or receive input without completing a CPace PAKE PIN exchange *and* being manually approved on **both** screens.
- **Instant emergency kill switch:** Press **`Ctrl + Alt + Shift + Esc`** at any time (or run `ravel --stop` / `ravel --kill-all`, or click **Kill switch** in the UI) to immediately release all held keys/buttons and sever all control sessions.
- **100% auditable open source:** Every line of code is in this repository, and releases are built directly by GitHub Actions ([`.github/workflows/release.yml`](.github/workflows/release.yml)). If you prefer not to trust prebuilt binaries, you can compile Ravel from source in under a minute (see below).

> **Tip for Windows users:** If Windows Defender blocks or quarantines `ravel.exe`, you can either build from source with `cargo build --release --locked` or add `%LOCALAPPDATA%\Programs\Ravel` as an exclusion in **Windows Security → Virus & threat protection → Manage settings → Exclusions**.

---

## Install a Release

Install the latest published x86_64 release without administrator privileges. Both installers register Ravel in your system's application launcher so you can press **Super / Windows key** and search **"Ravel"**.

### Linux (x86_64)

Requires `curl`, `tar`, `cmp`, and the runtime libraries in [packaging](docs/packaging.md):

```sh
curl -fsSL https://raw.githubusercontent.com/Sidearker/Ravel/main/install.sh | sh
```

This installs `~/.local/bin/ravel` and registers the desktop launcher (`~/.local/share/applications/ravel.desktop`) and icon.

### Windows (x64)

Run in PowerShell:

```powershell
curl.exe -fsSLo "$env:TEMP\ravel-install.ps1" https://raw.githubusercontent.com/Sidearker/Ravel/main/install.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File "$env:TEMP\ravel-install.ps1"
```

This installs `%LOCALAPPDATA%\Programs\Ravel\ravel.exe`, adds it to your user `PATH`, and creates a **Ravel** shortcut in your Start Menu.

---

## Build from Source

Rust 1.99+ is required. On Arch Linux:

```sh
sudo pacman -S --needed base-devel pkgconf libx11 libxtst libxkbcommon mesa
cargo build --release --locked
./target/release/ravel
```

## Quick Start

1. **Launch Ravel** on each computer on the same LAN.
2. **Set your device name** right at the top of the window (`This device:`) if you want a custom name.
3. **Pair once:** Click **Show PIN on this device** on one computer, then click **Pair** next to the discovered device on the other computer, enter the 8-digit PIN, and verify that the fingerprints match on both screens.
4. **Arrange your screens:** In **Screen layout & handoff**, grab the other device's box and drag it to the `Left`, `Right`, `Top`, or `Bottom` of your screen (aligned however you like). Changes apply immediately.
5. **Cross the edge:** Push your cursor outward through the shared edge to switch control to the other computer. Move back across the return edge — or click **Take control on this device** — to return locally.

## Documentation

- [Pairing, backends, stop bindings and firewall setup](docs/operation.md)
- [Background operation and login autostart](docs/background.md)
- [Desktop crossover effect design](docs/desktop-effect.md)
- [Protocol, limits and threat model](docs/protocol.md)
- [Build and packaging on Arch / Windows / macOS](docs/packaging.md)
- [Verification evidence and release gates](docs/verification.md)
- [Backend research and dependency provenance](docs/backend-feasibility.md)
- [Compatibility matrix and expansion plan](docs/compatibility.md)

## Validation & License

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
sh tests/install.sh
```

Licensed under **GPL-3.0-or-later** (see [`LICENSE`](LICENSE) and [`vendor/README.md`](vendor/README.md)).
