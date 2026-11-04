#!/bin/sh
# Builds mdwriter.app and installs it in /Applications, restarting it if it was open.
set -eu
cd "$(dirname "$0")/.."

cargo tauri build

app=src-tauri/target/release/bundle/macos/mdwriter.app
dest=/Applications/mdwriter.app

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
