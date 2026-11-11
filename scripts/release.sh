#!/bin/sh
# Builds mdwriter.app and installs it in /Applications, restarting it if it was open.
# Rune has no bundler, so this assembles the bundle itself: the release binary, the icon and
# an Info.plist.
set -eu
cd "$(dirname "$0")/.."

cargo build --release

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
app=target/release/mdwriter.app
dest=/Applications/mdwriter.app

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/mdwriter "$app/Contents/MacOS/mdwriter"
cp assets/icons/icon.icns "$app/Contents/Resources/icon.icns"
cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>mdwriter</string>
  <key>CFBundleDisplayName</key><string>mdwriter</string>
  <key>CFBundleIdentifier</key><string>com.richardparayno.mdwriter</string>
  <key>CFBundleExecutable</key><string>mdwriter</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
# An ad-hoc signature: Apple silicon refuses to run an unsigned bundle.
codesign --force --sign - "$app"

was_running=false
if pgrep -xq mdwriter; then
  was_running=true
  osascript -e 'quit app "mdwriter"'
  # Wait up to 10 s for it to exit.
  i=0
  while pgrep -xq mdwriter && [ $i -lt 20 ]; do sleep 0.5; i=$((i + 1)); done
  if pgrep -xq mdwriter; then
    echo "mdwriter didn't quit; close it and run this again." >&2
    exit 1
  fi
fi

rm -rf "$dest"
ditto "$app" "$dest"
echo "Installed $dest ($(git rev-parse --short HEAD)$(git diff --quiet HEAD || echo ', with uncommitted changes'))"

if $was_running; then open "$dest"; fi
