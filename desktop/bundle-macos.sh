#!/bin/sh
# Wrap the release binary in a minimal Clecta.app so Finder launches it as a GUI app
# instead of through Terminal — see PLAN §11. Run after `cargo build --release`.
#
# The bundle is also the only way to exercise the `.app/Contents/MacOS` walk-up that
# paths.rs does: launched from here, clecta-data/ appears *beside* Clecta.app, not
# inside it. That is the half of the portability check a unit test cannot make.
#
# Pass a binary path to bundle something other than the host build — the shipped
# artifact is Intel (PLAN §11), so on an Apple Silicon machine that is:
#   cargo build --release --target x86_64-apple-darwin
#   ./bundle-macos.sh target/x86_64-apple-darwin/release/clecta
set -eu

BIN="${1:-target/release/clecta}"
APP="$(dirname "$BIN")/Clecta.app"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"

[ -x "$BIN" ] || { echo "missing $BIN — run: cargo build --release" >&2; exit 1; }

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp "$BIN" "$APP/Contents/MacOS/clecta"

# No usage-description keys: clecta only ever opens an output device, and macOS asks
# permission for input. No CFBundleDocumentTypes either — dropping a file on the
# *window* needs nothing declared (PLAN §10); only dropping on the Dock icon would.
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key><string>clecta</string>
	<key>CFBundleDisplayName</key><string>clecta</string>
	<key>CFBundleIdentifier</key><string>com.spirtech.clecta</string>
	<key>CFBundleExecutable</key><string>clecta</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleVersion</key><string>$VERSION</string>
	<key>CFBundleShortVersionString</key><string>$VERSION</string>
	<key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
	<key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

echo "built $APP — launch: open $APP"
echo "settings will be written beside it, in $(dirname "$APP")/clecta-data/"
