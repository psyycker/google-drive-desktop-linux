#!/usr/bin/env bash
# Removes gdrive-linux. Your synced files, config and sync state are left in place;
# delete ~/.config/gdrive-linux and ~/.local/share/gdrive-linux to remove those too.
set -euo pipefail
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
systemctl --user disable --now gdrived.service 2>/dev/null || true
rm -f "${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/gdrived.service"
systemctl --user daemon-reload
pkill -x gdrive-app 2>/dev/null || true
rm -f "$HOME/.local/bin/gdrived" "$HOME/.local/bin/gdrive" "$HOME/.local/bin/gdrive-app"
rm -f "$DATA/applications/gdrive-linux.desktop" "$DATA/applications/gdrive-open.desktop"
rm -f "${XDG_CONFIG_HOME:-$HOME/.config}/autostart/gdrive-linux.desktop"
rm -f "$DATA/mime/packages/gdrive-linux.xml"
rm -f "$DATA"/icons/hicolor/*/apps/gdrive-linux.png
rm -rf "$DATA/doc/gdrive-linux"
update-mime-database "$DATA/mime" >/dev/null 2>&1 || true
echo "Uninstalled."
