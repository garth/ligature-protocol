#!/bin/sh
# Wraps ligatured in a minimal application bundle, Ligature.app (LP-94):
#
#   packaging/macos-bundle.sh BIN_DIR OUT_DIR
#
# BIN_DIR holds ligatured; the result is OUT_DIR/Ligature.app, replaced if it
# is there. macOS shows notifications only from a process inside a bundle that
# the user has allowed once, so the daemon runs from this bundle: the
# permission prompt and every notification carry the name Ligature. The bundle
# has no Dock icon (LSUIElement) and holds the same binary, so nothing else
# changes. It is signed ad hoc unless LIGATURE_CODESIGN_IDENTITY names a
# Developer ID identity, which also turns on the hardened runtime that
# notarisation needs; notarising is a separate step.
set -eu

bin_dir=$1
out_dir=$2
identity=${LIGATURE_CODESIGN_IDENTITY:--}
bundle_id=org.ligatureprotocol.ligatured

[ -x "$bin_dir/ligatured" ] || { echo "macos-bundle.sh: $bin_dir/ligatured not found" >&2; exit 1; }
# "ligatured 0.1.1-alpha" to 0.1.1, the numeric part CFBundleVersion allows.
full=$("$bin_dir/ligatured" --version | awk '{print $2}')
short=$(printf '%s' "$full" | sed 's/[^0-9.].*//; s/\.$//')

app=$out_dir/Ligature.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
install -m 755 "$bin_dir/ligatured" "$app/Contents/MacOS/ligatured"
cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key>
  <string>$bundle_id</string>
  <key>CFBundleName</key>
  <string>Ligature</string>
  <key>CFBundleDisplayName</key>
  <string>Ligature</string>
  <key>CFBundleExecutable</key>
  <string>ligatured</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$short</string>
  <key>CFBundleVersion</key>
  <string>$short</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
  <key>LSUIElement</key>
  <true/>
  <key>NSHumanReadableCopyright</key>
  <string>Ligature Protocol $full, Apache-2.0</string>
</dict>
</plist>
EOF
plutil -lint -s "$app/Contents/Info.plist"

if [ "$identity" = "-" ]; then
    codesign --force --sign - --identifier "$bundle_id" "$app"
else
    codesign --force --sign "$identity" --identifier "$bundle_id" --options runtime --timestamp "$app"
fi
codesign --verify --strict "$app"
echo "$app"
