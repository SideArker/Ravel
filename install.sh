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

tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT
trap 'exit 1' HUP INT TERM
url="https://github.com/$repo/releases/latest/download/$asset"
curl --fail --location --silent --show-error "$url" -o "$tmpdir/$asset"
tar -xzf "$tmpdir/$asset" -C "$tmpdir" ravel

bindir=${RAVEL_INSTALL_DIR:-"$HOME/.local/bin"}
mkdir -p "$bindir"
install -m 755 "$tmpdir/ravel" "$bindir/ravel"
printf 'Installed Ravel to %s/ravel\n' "$bindir"
case ":${PATH}:" in
    *":$bindir:"*) ;;
    *) printf 'Add %s to your PATH to run ravel from any shell.\n' "$bindir" ;;
esac
