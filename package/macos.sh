#!/bin/sh
set -eu

: "${RAVEL_CODESIGN_IDENTITY:?Set a Developer ID Application signing identity}"
: "${RAVEL_VERSION:?Set the release version, for example 0.3.0}"
case "$RAVEL_VERSION" in
    *[!0-9.]*|'') echo "RAVEL_VERSION must contain only digits and dots" >&2; exit 1 ;;
esac
case "$(uname -m)" in
    arm64) archive_arch=aarch64 ;;
    x86_64) archive_arch=x86_64 ;;
    *) echo "Unsupported macOS architecture" >&2; exit 1 ;;
esac

cargo build --release --locked
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
app="$stage/Ravel.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/docs" dist
cp target/release/ravel "$app/Contents/MacOS/ravel"
cp README.md LICENSE "$app/Contents/Resources/"
cp docs/*.md "$app/Contents/Resources/docs/"
cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleDevelopmentRegion</key><string>en</string>
<key>CFBundleExecutable</key><string>ravel</string>
<key>CFBundleIdentifier</key><string>com.sidearker.ravel</string>
<key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
<key>CFBundleName</key><string>Ravel</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$RAVEL_VERSION</string>
<key>CFBundleVersion</key><string>$RAVEL_VERSION</string>
<key>LSMinimumSystemVersion</key><string>11.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
EOF
codesign --force --options runtime --timestamp --sign "$RAVEL_CODESIGN_IDENTITY" "$app"
codesign --verify --deep --strict --verbose=2 "$app"
ditto -c -k --sequesterRsrc --keepParent "$app" "dist/ravel-macos-$archive_arch.zip"
