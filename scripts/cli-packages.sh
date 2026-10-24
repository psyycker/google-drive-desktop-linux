#!/usr/bin/env bash
# Builds the headless gdrive-linux-cli .deb and .rpm from release binaries:
#   cargo build --release -p gdrived -p gdrive-cli
#   scripts/cli-packages.sh [out-dir]          (default: dist)
# Needs nfpm (https://nfpm.goreleaser.com); set NFPM to use one that isn't on PATH.
# VERSION defaults to the workspace version.
set -euo pipefail
cd "$(dirname "$0")/.."
NFPM="${NFPM:-nfpm}"
OUT="${1:-dist}"

export VERSION="${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)}"
case "$(uname -m)" in
  x86_64) export ARCH=amd64 ;;
  aarch64 | arm64) export ARCH=arm64 ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac
for bin in gdrived gdrive; do
  [ -x "target/release/$bin" ] || { echo "target/release/$bin is missing; run: cargo build --release -p gdrived -p gdrive-cli" >&2; exit 1; }
done

mkdir -p "$OUT"
for packager in deb rpm; do
  "$NFPM" package --config packaging/nfpm-cli.yaml --packager "$packager" --target "$OUT/"
done
