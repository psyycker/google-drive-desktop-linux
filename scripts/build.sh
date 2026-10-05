#!/usr/bin/env bash
# Builds everything in release mode. The Tauri app needs WebKitGTK headers, which
# live in the `gdrive-dev` distrobox; the daemon and CLI build anywhere.
set -euo pipefail
cd "$(dirname "$0")/.."
CARGO="${CARGO:-$HOME/.cargo/bin/cargo}"
BOX="${GDRIVE_BOX:-gdrive-dev}"

if pkg-config --exists webkit2gtk-4.1 2>/dev/null; then
  "$CARGO" build --release --workspace
elif command -v distrobox >/dev/null && distrobox list 2>/dev/null | grep -q " $BOX "; then
  distrobox enter "$BOX" -- bash -lc "cd '$PWD' && '$CARGO' build --release --workspace"
else
  echo "WebKitGTK dev headers not found and no '$BOX' distrobox; building daemon + CLI only." >&2
  "$CARGO" build --release
fi
