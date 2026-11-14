#!/usr/bin/env bash
# Installs the launcher entries + icon for the current user, so that Wayland/X11 show the todo.txt icon for the window.
# Usage: tools/linux/install.sh [/path/to/todotxt]   (default: todotxt from PATH, else src-tauri/target/release/todotxt)
set -e
d="$(cd "$(dirname "$0")" && pwd)"
bin="${1:-$(command -v todotxt || echo "$d/../../src-tauri/target/release/todotxt")}"
bin="$(readlink -f "$bin")"
mkdir -p ~/.local/share/applications ~/.local/share/icons/hicolor/scalable/apps ~/.local/share/icons/hicolor/128x128/apps
cp "$d/dev.todotxt.desktop.svg" ~/.local/share/icons/hicolor/scalable/apps/
cp "$d/../../src-tauri/icons/128x128.png" ~/.local/share/icons/hicolor/128x128/apps/dev.todotxt.desktop.png
for f in dev.todotxt.desktop.desktop todotxt.desktop; do sed "s|^Exec=.*|Exec=$bin|" "$d/$f" > ~/.local/share/applications/$f; done
update-desktop-database ~/.local/share/applications 2>/dev/null || true
gtk-update-icon-cache -f ~/.local/share/icons/hicolor 2>/dev/null || true
echo "installed for $bin"
