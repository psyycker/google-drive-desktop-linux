#!/usr/bin/env bash
# Builds a single-file AppImage containing the app, the daemon and the CLI:
#   dist/gdrive-linux-<version>-x86_64.AppImage            → tray + window
#   dist/gdrive-linux-<version>-x86_64.AppImage --daemon   → sync daemon
#   dist/gdrive-linux-<version>-x86_64.AppImage --cli ...  → `gdrive` CLI
# Runs inside the `gdrive-dev` distrobox when the host lacks WebKitGTK headers.
set -euo pipefail
cd "$(dirname "$0")/.."
CARGO="${CARGO:-$HOME/.cargo/bin/cargo}"
BOX="${GDRIVE_BOX:-gdrive-dev}"

if ! pkg-config --exists webkit2gtk-4.1 2>/dev/null && [ -z "${GDRIVE_IN_BOX:-}" ]; then
  exec distrobox enter "$BOX" -- env GDRIVE_IN_BOX=1 bash -lc "cd '$PWD' && scripts/appimage.sh"
fi

"$CARGO" tauri --version >/dev/null 2>&1 || "$CARGO" install tauri-cli --version '^2' --locked

RUSTC="$(dirname "$CARGO")/rustc"
[ -x "$RUSTC" ] || RUSTC=rustc
TRIPLE="$("$RUSTC" -vV | sed -n 's/^host: //p')"
"$CARGO" build --release -p gdrived -p gdrive-cli
mkdir -p app/src-tauri/binaries
cp target/release/gdrived "app/src-tauri/binaries/gdrived-$TRIPLE"
cp target/release/gdrive "app/src-tauri/binaries/gdrive-$TRIPLE"

# linuxdeploy is itself an AppImage; containers usually lack FUSE, so let it self-extract.
# Its bundled `strip` is too old for current Fedora binaries, hence NO_STRIP.
(cd app/src-tauri && APPIMAGE_EXTRACT_AND_RUN=1 NO_STRIP=true \
  "$CARGO" tauri build --config tauri.bundle.conf.json --bundles appimage)

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
mkdir -p dist
OUT="dist/gdrive-linux-$VERSION-x86_64.AppImage"
# Copy then rename, so a copy of the old AppImage that is running is not in the way.
cp "$(ls -t target/release/bundle/appimage/*.AppImage | head -1)" "$OUT.new"
mv -f "$OUT.new" "$OUT"
chmod +x "$OUT"
echo "Built $OUT ($(du -h "$OUT" | cut -f1))"
