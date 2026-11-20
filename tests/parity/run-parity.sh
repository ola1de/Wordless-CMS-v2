#!/usr/bin/env bash
# WORDLESS v2 P5 — Parity run orchestrator.
#
# Clean-room differential run: the same seeded D1, both workers
# (v1 TypeScript, v2 Rust) under local wrangler dev, then the harness
# diffs every corpus step. The TS repo is NEVER modified — v1 runs from a
# scratch copy under tests/parity/work/v1 (node_modules symlinked back).
# Nothing is deployed; no Cloudflare credentials are involved.
#
# Usage: ./tests/parity/run-parity.sh
# Outputs: tests/parity/results/parity-report.json + latency.json
# Exit code: the harness exit code (0 = zero unexplained diffs).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# Location of the v1 TypeScript repo. Override with V1SRC when it lives
# elsewhere, e.g. V1SRC=$HOME/Wordless-CMS ./tests/parity/run-parity.sh
V1SRC="${V1SRC:-$HOME/workspace/wordless/wordless-cms}"
PARITY="$ROOT/tests/parity"
WORK="$PARITY/work"
FIX="$PARITY/fixtures"
RESULTS="$PARITY/results"
V1_PORT="${V1_PORT:-8801}"
V2_PORT="${V2_PORT:-8802}"
WRANGLER="$V1SRC/node_modules/.bin/wrangler"

export TMPDIR="${TMPDIR:-$HOME/.tmp-cargo}"
mkdir -p "$TMPDIR"
export PATH="$HOME/.cargo/bin:$PATH"

# Webhook secrets for this local run only — injected identically into both
# workers via --var; they must match the harness's SECRETS table.
VARS=(
  --var "MOCK_PAYMENT_WEBHOOK_SECRET:parity-mock-secret"
  --var "PAYSTACK_SECRET_KEY:sk_test_parity_paystack"
  --var "FLUTTERWAVE_SECRET_KEY:FLWSECK_TEST-parity"
  --var "FLUTTERWAVE_SECRET_HASH:flw-parity-hash"
  --var "STRIPE_SECRET_KEY:sk_test_parity_stripe"
  --var "STRIPE_WEBHOOK_SECRET:whsec_parity_test"
)

# Split (2026-11-20): conversion runs in the converter Worker —
# start it first, and point the v2 Worker at it. Size remedy (same
# day): social and WooCommerce run in their own Workers — start them
# too (the Woo Worker gets the same webhook/provider secrets, since
# it verifies webhooks and resolves providers itself). School (P9)
# joined them the same day — start it too, and point v2 at it.
source "$ROOT/scripts/converter-dev.sh"
source "$ROOT/scripts/split-workers-dev.sh"

V1_PID=""
V2_PID=""
cleanup() {
  [ -n "$V1_PID" ] && kill "$V1_PID" 2>/dev/null || true
  [ -n "$V2_PID" ] && kill "$V2_PID" 2>/dev/null || true
  stop_converter
  stop_social
  stop_woo
  stop_school
  # workerd children can survive the npx wrapper; make sure the ports free up
  sleep 1
  for p in "$V1_PORT" "$V2_PORT" "$CONVERTER_PORT" "$SOCIAL_PORT" "$WOO_PORT" "$SCHOOL_PORT"; do
    if command -v fuser >/dev/null 2>&1; then fuser -k "${p}/tcp" 2>/dev/null || true; fi
  done
}
trap cleanup EXIT

echo "== P5 parity: clean room =="
rm -rf "$ROOT/.wrangler" "$WORK" "$RESULTS"
mkdir -p "$WORK" "$FIX" "$RESULTS"

echo "== P5 parity: v1 scratch copy (TS repo untouched) =="
mkdir -p "$WORK/v1"
# tar copy (excludes node_modules/.wrangler; non-root tar does not try to
# preserve ownership, unlike cp -a which fails on root-owned source files)
(cd "$V1SRC" && tar --exclude=node_modules --exclude=.wrangler --exclude=.git -cf - .) | (cd "$WORK/v1" && tar --no-same-owner -xf -)
ln -s "$V1SRC/node_modules" "$WORK/v1/node_modules"

echo "== P5 parity: fixtures =="
python3 - "$FIX" <<'PY'
import base64, io, sys, zipfile
from pathlib import Path
fix = Path(sys.argv[1])
# 1x1 transparent PNG
png = base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8Xw8AAwMCAO+ip1sAAAAASUVORK5CYII=")
(fix / "parity.png").write_bytes(png)
(fix / "parity-note.txt").write_text("Parity fixture note.\n")
def zip_to(name, files):
    with zipfile.ZipFile(fix / name, "w", zipfile.ZIP_DEFLATED) as z:
        for arc, data in files.items():
            z.writestr(arc, data)
zip_to("parity-theme.zip", {
    "style.css": "/*\nTheme Name: Parity Theme\nText Domain: parity-theme\nVersion: 1.0.0\n*/\n",
    "theme.json": '{\n  "$schema": "https://schemas.wp.org/trunk/theme.json",\n  "version": 2,\n  "settings": {\n    "color": { "palette": [{ "slug": "primary", "color": "#0d9488", "name": "Primary" }] }\n  }\n}\n',
})
zip_to("parity-classic.zip", {
    "style.css": "/*\nTheme Name: Parity Classic\nText Domain: parity-classic\nVersion: 1.0.0\n*/\n",
    "index.php": "<?php get_header(); ?>\n<main><h1>Parity Classic</h1></main>\n<?php get_footer(); ?>\n",
    "functions.php": "<?php\nfunction parity_classic_setup() {\n  add_theme_support('title-tag');\n}\nadd_action('after_setup_theme', 'parity_classic_setup');\n",
})
PY
# Hello Dolly fixture — the real plugin file from the reference, never modified.
# The marketplace installer needs the smoke-p3 shape: a folder
# `hello-dolly/` containing `wordless.json` + `hello-dolly/hello.php`
# (a flat hello.php at the ZIP root is rejected: no manifest).
HELLO_PHP="${HELLO_PHP:-$HOME/workspace/wordless/reference/wordpress-7.1.5/wp-content/plugins/hello.php}"
# Fallback: the fixture bundled in this repo, so the harness runs on a
# machine that has never seen the reference tree.
if [ ! -f "$HELLO_PHP" ] && [ -f "$ROOT/tests/parity/fixtures-src/hello.php" ]; then
  HELLO_PHP="$ROOT/tests/parity/fixtures-src/hello.php"
fi
if [ -f "$HELLO_PHP" ]; then
  HD_TMP="$(mktemp -d)"
  mkdir -p "$HD_TMP/hello-dolly"
  cp -f "$HELLO_PHP" "$HD_TMP/hello-dolly/hello.php"
  printf '%s' '{"slug":"hello-dolly","name":"Hello Dolly","version":"1.7.2","description":"A plugin that prints a random lyric."}' > "$HD_TMP/hello-dolly/wordless.json"
  # `zip -r` UPDATES an existing archive instead of replacing it, so a
  # stale flat hello.php from an older build would survive and break the
  # installer's prefix-strip. Remove first.
  rm -f "$FIX/hello-dolly.zip"
  (cd "$HD_TMP" && zip -q -r "$FIX/hello-dolly.zip" hello-dolly)
  rm -rf "$HD_TMP"
  cp -f "$HELLO_PHP" "$FIX/hello.php"
else
  echo "WARN: hello.php not found at $HELLO_PHP — hello-dolly.zip not built" >&2
fi
cp -f "$V1SRC/test/fixtures/wordpress/wordpress-export.xml" "$FIX/wordpress-export.xml"
cp -f "$V1SRC/test/fixtures/wordpress/wordpress-dump.sql" "$FIX/wordpress-dump.sql"
ls -la "$FIX"

echo "== P5 parity: v1 migrations + seed =="
(cd "$WORK/v1" && "$WRANGLER" d1 migrations apply DB --local >/dev/null)
(cd "$WORK/v1" && "$WRANGLER" d1 execute DB --local --file "$PARITY/seed.sql" >/dev/null)

echo "== P5 parity: v2 migrations + seed =="
(cd "$ROOT" && "$WRANGLER" d1 migrations apply DB --local >/dev/null)
(cd "$ROOT" && "$WRANGLER" d1 execute DB --local --file "$PARITY/seed.sql" >/dev/null)

start_converter
start_social
start_woo "${VARS[@]}"
start_school
start_seo
V2_VARS=("${VARS[@]}" --var "WORDLESS_CONVERTER_URL:$CONVERTER_URL" --var "WORDLESS_SOCIAL_URL:$SOCIAL_URL" --var "WORDLESS_WOO_URL:$WOO_URL" --var "WORDLESS_SCHOOL_URL:$SCHOOL_URL" --var "WORDLESS_SEO_URL:$SEO_URL")
echo "== P5 parity: starting v1 on :$V1_PORT =="
(cd "$WORK/v1" && exec "$WRANGLER" dev --port "$V1_PORT" --local "${VARS[@]}" >"$WORK/v1.log" 2>&1) &
V1_PID=$!
echo "== P5 parity: starting v2 on :$V2_PORT (cargo build runs first — can take minutes) =="
(cd "$ROOT" && exec "$WRANGLER" dev --port "$V2_PORT" --local "${V2_VARS[@]}" >"$WORK/v2.log" 2>&1) &
V2_PID=$!

wait_for() { # wait_for <base> <label> <max-tries>
  local base="$1" label="$2" max="$3"
  for i in $(seq 1 "$max"); do
    if curl -fsS -o /dev/null "$base/api/health" 2>/dev/null; then
      echo "  $label up after ~$((i * 2))s"
      return 0
    fi
    sleep 2
  done
  echo "FAIL: $label did not answer $base/api/health within $((max * 2))s" >&2
  echo "--- $label log (tail) ---" >&2
  tail -40 "$WORK/$label.log" >&2
  return 1
}
wait_for "http://127.0.0.1:$V1_PORT" v1 120
wait_for "http://127.0.0.1:$V2_PORT" v2 300

echo "== P5 parity: running harness =="
set +e
node "$PARITY/harness.mjs" \
  --v1 "http://127.0.0.1:$V1_PORT" \
  --v2 "http://127.0.0.1:$V2_PORT" \
  --fixtures "$FIX" \
  --out "$RESULTS/parity-report.json" \
  --latency-out "$RESULTS/latency.json"
HARNESS_RC=$?
set -e

echo "== P5 parity: harness exit code $HARNESS_RC =="
exit "$HARNESS_RC"
