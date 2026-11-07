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
target="$bindir/ravel"
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
    staged=$(mktemp "$bindir/.ravel.XXXXXX")
    install -m 755 "$tmpdir/ravel" "$staged"
    mv -f "$staged" "$target"
    staged=''
    printf '%s Ravel at %s\n' "$action" "$target"
fi
case ":${PATH}:" in
    *":$bindir:"*) ;;
    *) printf 'Add %s to your PATH to run ravel from any shell.\n' "$bindir" ;;
esac
