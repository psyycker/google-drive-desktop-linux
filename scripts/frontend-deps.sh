#!/usr/bin/env bash
# Installs the frontend's npm dependencies (app/node_modules) when they are missing or
# older than the lockfile. Needs Node.js 20.19+ and npm.
set -euo pipefail
cd "$(dirname "$0")/../app"
if [ ! -f node_modules/.package-lock.json ] || [ package-lock.json -nt node_modules/.package-lock.json ]; then
  npm ci --no-audit --no-fund
fi
