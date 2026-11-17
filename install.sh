#!/bin/sh
set -eu

repo='Sidearker/Ravel'
asset='ravel-linux-x86_64.tar.gz'

if [ "$(uname -s)" != Linux ]; then
    echo "Ravel's binary installer currently supports Linux x86_64 only." >&2
    exit 1
fi
case "$(uname -m)" in
    x86_64|amd64) ;;
    *) echo "Ravel's binary installer currently supports Linux x86_64 only." >&2; exit 1 ;;
esac

command -v curl >/dev/null 2>&1 || { echo 'curl is required.' >&2; exit 1; }
command -v tar >/dev/null 2>&1 || { echo 'tar is required.' >&2; exit 1; }
command -v cmp >/dev/null 2>&1 || { echo 'cmp is required.' >&2; exit 1; }

tmpdir=$(mktemp -d)
staged=''
cleanup() {
    [ -z "$staged" ] || rm -f "$staged"
    rm -rf "$tmpdir"
}
trap 'cleanup' EXIT
trap 'exit 1' HUP INT TERM
url="https://github.com/$repo/releases/latest/download/$asset"
curl --fail --location --silent --show-error "$url" -o "$tmpdir/$asset"
tar -xzf "$tmpdir/$asset" -C "$tmpdir" ravel

bindir=${RAVEL_INSTALL_DIR:-"$HOME/.local/bin"}
mkdir -p "$bindir"
abs_bindir=$(CDPATH= cd -- "$bindir" && pwd)
target="$abs_bindir/ravel"
if [ -d "$target" ]; then
    printf 'Cannot install Ravel: %s is a directory.\n' "$target" >&2
    exit 1
fi
if [ -f "$target" ] && [ -x "$target" ] && cmp -s "$tmpdir/ravel" "$target"; then
    printf 'Ravel is already up to date at %s\n' "$target"
else
    if [ -e "$target" ]; then
        action='Updated'
    else
        action='Installed'
    fi
    # Rename on the same filesystem so upgrades work even while Ravel is running.
    staged=$(mktemp "$abs_bindir/.ravel.XXXXXX")
    install -m 755 "$tmpdir/ravel" "$staged"
    mv -f "$staged" "$target"
    staged=''
    printf '%s Ravel at %s\n' "$action" "$target"
fi

datadir=${XDG_DATA_HOME:-"$HOME/.local/share"}
appdir="$datadir/applications"
icondir="$datadir/icons/hicolor/scalable/apps"
mkdir -p "$appdir" "$icondir"

cat > "$icondir/ravel.svg" <<'EOF'
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128" width="128" height="128">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0%" stop-color="#1e2233"/>
      <stop offset="100%" stop-color="#121420"/>
    </linearGradient>
    <linearGradient id="threadA" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0%" stop-color="#5ee7df"/>
      <stop offset="100%" stop-color="#6b8cff"/>
    </linearGradient>
    <linearGradient id="threadB" x1="1" y1="0" x2="0" y2="1">
      <stop offset="0%" stop-color="#ff8a65"/>
      <stop offset="100%" stop-color="#b465da"/>
    </linearGradient>
  </defs>
  <rect x="8" y="8" width="112" height="112" rx="28" fill="url(#bg)" stroke="#2e3552" stroke-width="3"/>
  <path d="M 28 84 C 24 54, 48 30, 74 36 C 96 41, 100 68, 78 82 C 58 95, 42 74, 64 56 L 98 28"
        fill="none" stroke="url(#threadA)" stroke-width="11" stroke-linecap="round" stroke-linejoin="round"/>
  <path d="M 30 30 L 64 58 C 86 76, 70 96, 50 83 C 28 69, 32 42, 54 37 C 80 31, 104 55, 100 85"
        fill="none" stroke="url(#threadB)" stroke-width="11" stroke-linecap="round" stroke-linejoin="round"/>
  <path d="M 53 66 C 60 60, 68 54, 75 48"
        fill="none" stroke="#181b28" stroke-width="17" stroke-linecap="round"/>
  <path d="M 51 68 C 59 61, 69 53, 77 46"
        fill="none" stroke="url(#threadA)" stroke-width="11" stroke-linecap="round"/>
</svg>
EOF

escaped_target=$(printf '%s' "$target" | sed 's/\\/\\\\/g; s/"/\\"/g')
cat > "$appdir/ravel.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Ravel
Comment=Share a mouse and keyboard over your LAN
Exec="$escaped_target"
Icon=ravel
Terminal=false
Categories=Utility;Network;
StartupWMClass=ravel
Keywords=mouse;keyboard;kvm;lan;share;
EOF
chmod 644 "$appdir/ravel.desktop" "$icondir/ravel.svg"
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$appdir" >/dev/null 2>&1 || :
fi

case ":${PATH}:" in
    *":$bindir:"*|*":$abs_bindir:"*) ;;
    *) printf 'Add %s to your PATH to run ravel from any shell.\n' "$bindir" ;;
esac
