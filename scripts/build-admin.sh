#!/usr/bin/env bash
# Build the Wordless v2.0 admin SPA (React 18 + Vite) into admin/dist/admin/.
#
# TWO PATHS — which one runs depends on the machine:
#
# 1. Normal machine (user's Windows box / CI): if admin/node_modules is
#    absent, this script runs `npm install` inside admin/ first (using
#    admin/package.json), then `vite build -c vite.config.ts`. The vite
#    binary used is the locally installed one (admin/node_modules/.bin/vite).
#
# 2. This Linux VM (offline): the npm registry is unreachable here (TLS
#    failures), so `npm install` cannot run. admin/node_modules is instead a
#    SYMLINK to the v1 TypeScript repo's node_modules
#    (~/workspace/wordless/wordless-cms/node_modules) — a VM-local build aid,
#    never committed. When that symlink exists, the script skips npm install
#    and builds with the TS repo's vite binary:
#        ~/workspace/wordless/wordless-cms/node_modules/.bin/vite build -c vite.config.ts
#    The TS repo is read-only reference; this script never writes to it.
#
# Output layout MUST stay dist/admin/ (vite outDir "dist/admin", base
# "/admin/") so that wrangler [assets] directory = ./admin/dist serves the
# SPA entry at /admin/index.html without shadowing the public home page.
#
# Usage: bash scripts/build-admin.sh   (from the wordless-rust root)
# Typecheck separately: <vite-bin-dir>/tsc -p admin/tsconfig.json --noEmit
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ADMIN_DIR="$ROOT/admin"
TS_REPO="$HOME/workspace/wordless/wordless-cms"

cd "$ADMIN_DIR"

if [ -x "node_modules/.bin/vite" ]; then
  # Path 1 (normal machine, after npm install) or path 2 (VM symlink):
  # a usable local vite binary already exists. Use it.
  echo "[build-admin] using local vite: node_modules/.bin/vite"
  ./node_modules/.bin/vite build -c vite.config.ts
  exit 0
fi

if [ -d "$TS_REPO/node_modules/.bin" ] && [ -x "$TS_REPO/node_modules/.bin/vite" ]; then
  # Path 2 (VM): fall back to the TS repo's vite binary. Do NOT run
  # npm install here — the registry is unreachable in this VM.
  echo "[build-admin] no local vite; using TS repo vite binary (VM fallback)"
  "$TS_REPO/node_modules/.bin/vite" build -c vite.config.ts
  exit 0
fi

# Path 1 (normal machine, fresh checkout): install then build.
echo "[build-admin] node_modules absent; running npm install in admin/"
npm install
./node_modules/.bin/vite build -c vite.config.ts
