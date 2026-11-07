#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
tmpdir=$(mktemp -d)
running=''
cleanup() {
    if [ -n "$running" ]; then
        kill "$running" 2>/dev/null || :
        wait "$running" 2>/dev/null || :
    fi
    rm -rf "$tmpdir"
}
trap 'cleanup' EXIT
trap 'exit 1' HUP INT TERM
mkdir -p "$tmpdir/bin" "$tmpdir/package"

cat > "$tmpdir/bin/curl" <<'EOF'
#!/bin/sh
set -eu
previous=''
for argument do
    if [ "$previous" = '-o' ]; then
        output=$argument
        break
    fi
    previous=$argument
done
[ "${RAVEL_TEST_FAIL:-}" != 1 ] || exit 22
cp "$RAVEL_TEST_ARCHIVE" "$output"
EOF
chmod +x "$tmpdir/bin/curl"

printf '#!/bin/sh\nprintf "new release\\n"\n' > "$tmpdir/package/ravel"
chmod +x "$tmpdir/package/ravel"
tar -czf "$tmpdir/release.tar.gz" -C "$tmpdir/package" ravel

run_installer() {
    PATH="$tmpdir/bin:$PATH" RAVEL_INSTALL_DIR="$tmpdir/install" \
        RAVEL_TEST_ARCHIVE="$tmpdir/release.tar.gz" sh "$root/install.sh"
}

output=$(run_installer)
case "$output" in *"Installed Ravel at $tmpdir/install/ravel"*) ;; *) exit 1 ;; esac
[ "$("$tmpdir/install/ravel")" = 'new release' ]

printf '#!/bin/sh\nprintf "old release\\n"\n' > "$tmpdir/install/ravel"
chmod +x "$tmpdir/install/ravel"
output=$(run_installer)
case "$output" in *"Updated Ravel at $tmpdir/install/ravel"*) ;; *) exit 1 ;; esac
[ "$("$tmpdir/install/ravel")" = 'new release' ]

output=$(run_installer)
case "$output" in *"already up to date"*) ;; *) exit 1 ;; esac

cp "$(command -v sleep)" "$tmpdir/install/ravel"
"$tmpdir/install/ravel" 30 &
running=$!
sleep 0.1
output=$(run_installer)
case "$output" in *"Updated Ravel at $tmpdir/install/ravel"*) ;; *) exit 1 ;; esac
[ "$("$tmpdir/install/ravel")" = 'new release' ]
kill "$running"
wait "$running" 2>/dev/null || :
running=''

RAVEL_TEST_FAIL=1
export RAVEL_TEST_FAIL
if run_installer 2>/dev/null; then
    echo 'Installer unexpectedly succeeded after download failure.' >&2
    exit 1
fi
[ "$("$tmpdir/install/ravel")" = 'new release' ]

printf 'Installer checks passed.\n'
