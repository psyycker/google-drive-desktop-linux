#!/usr/bin/env bash
# Installs gdrive-linux for the current user (no root needed):
#   binaries → ~/.local/bin, systemd user service, desktop entries, .gdoc MIME type.
set -euo pipefail
cd "$(dirname "$0")/.."
BIN="$HOME/.local/bin"
APPS="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
MIME="${XDG_DATA_HOME:-$HOME/.local/share}/mime"
ICONS="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor"
UNITS="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
DOC="${XDG_DATA_HOME:-$HOME/.local/share}/doc/gdrive-linux"
REL=target/release

[ -x "$REL/gdrived" ] || { echo "Run scripts/build.sh first." >&2; exit 1; }
mkdir -p "$BIN" "$APPS" "$MIME/packages" "$UNITS" "$DOC"

install -m755 "$REL/gdrived" "$BIN/gdrived"
install -m755 "$REL/gdrive" "$BIN/gdrive"
if [ -x "$REL/gdrive-app" ]; then
  install -m755 "$REL/gdrive-app" "$BIN/gdrive-app"
  install -m644 packaging/gdrive-linux.desktop "$APPS/gdrive-linux.desktop"
else
  echo "note: gdrive-app not built; skipping the tray/window app." >&2
fi
install -m644 packaging/gdrive-open.desktop "$APPS/gdrive-open.desktop"
install -m644 packaging/gdrive-linux-mime.xml "$MIME/packages/gdrive-linux.xml"
install -m644 README.md "$DOC/README.md"

for size in 32 128 512; do
  src="app/src-tauri/icons/${size}x${size}.png"
  [ "$size" = 512 ] && src="app/src-tauri/icons/icon.png"
  if [ -f "$src" ]; then
    mkdir -p "$ICONS/${size}x${size}/apps"
    install -m644 "$src" "$ICONS/${size}x${size}/apps/gdrive-linux.png"
  fi
done

update-mime-database "$MIME" >/dev/null 2>&1 || true
update-desktop-database "$APPS" >/dev/null 2>&1 || true
xdg-mime default gdrive-open.desktop application/x-gdrive-link 2>/dev/null || true
gtk-update-icon-cache -q -t "$ICONS" 2>/dev/null || true

install -m644 packaging/gdrived.service "$UNITS/gdrived.service"
systemctl --user daemon-reload
systemctl --user enable --now gdrived.service
systemctl --user restart gdrived.service

case ":$PATH:" in *":$BIN:"*) ;; *) echo "note: add $BIN to your PATH." >&2 ;; esac
echo "Installed. Launch 'Google Drive (gdrive-linux)' from your app grid, or run: gdrive-app --show"
