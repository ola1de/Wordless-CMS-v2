#!/usr/bin/env bash
# WORDLESS v2 — local split dev: converter + social + Woo + school + SEO + main Workers.
#
#   npm run dev:split
#   # or: bash scripts/dev-split.sh
#
# Starts the converter Worker on :8805, the social Worker on :8807,
# the Woo Worker on :8808, the school Worker on :8809 and the SEO Worker on :8812 first, then
# the main Worker on :8787 with WORDLESS_CONVERTER_URL /
# WORDLESS_SOCIAL_URL / WORDLESS_WOO_URL / WORDLESS_SCHOOL_URL / WORDLESS_SEO_URL
# pointed at them. All six run in the foreground of this script's
# process group; Ctrl-C stops all of them.
# (This is for interactive local development only — the smoke and
# parity scripts start the same Workers programmatically.)
#
# The social, Woo, school and SEO Workers share the main Worker's local D1 (see
# scripts/split-workers-dev.sh): their --persist-to points at the
# workspace .wrangler/state dir, so seed data created through the
# main Worker is visible to them.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
WRANGLER="${WRANGLER:-$ROOT/node_modules/.bin/wrangler}"
CONVERTER_PORT=8805
SOCIAL_PORT=8807
WOO_PORT=8808
SCHOOL_PORT=8809
SEO_PORT=8812
MAIN_PORT="${MAIN_PORT:-8787}"

cleanup() {
  [ -n "${CONV_PID:-}" ] && kill "$CONV_PID" 2>/dev/null || true
  [ -n "${SOCIAL_PID:-}" ] && kill "$SOCIAL_PID" 2>/dev/null || true
  [ -n "${WOO_PID:-}" ] && kill "$WOO_PID" 2>/dev/null || true
  [ -n "${SCHOOL_PID:-}" ] && kill "$SCHOOL_PID" 2>/dev/null || true
  [ -n "${SEO_PID:-}" ] && kill "$SEO_PID" 2>/dev/null || true
  [ -n "${MAIN_PID:-}" ] && kill "$MAIN_PID" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

rm -rf "$ROOT/crates/wordless-converter/.wrangler"
rm -rf "$ROOT/crates/wordless-social-worker/.wrangler"
rm -rf "$ROOT/crates/wordless-woocommerce-worker/.wrangler"
rm -rf "$ROOT/crates/wordless-school-worker/.wrangler"
rm -rf "$ROOT/crates/wordless-seo-worker/.wrangler"
echo "== converter: :$CONVERTER_PORT =="
(cd "$ROOT/crates/wordless-converter" && "$WRANGLER" dev --port "$CONVERTER_PORT" --local) &
CONV_PID=$!
for i in $(seq 1 240); do
  if curl -fsS -o /dev/null "http://127.0.0.1:$CONVERTER_PORT/__convert/health" 2>/dev/null; then
    echo "  converter up after ~$((i * 2))s"
    break
  fi
  sleep 2
done

echo "== social: :$SOCIAL_PORT =="
(cd "$ROOT/crates/wordless-social-worker" && "$WRANGLER" dev --port "$SOCIAL_PORT" --local --persist-to "$ROOT/.wrangler/state") &
SOCIAL_PID=$!
for i in $(seq 1 240); do
  if curl -fsS -o /dev/null "http://127.0.0.1:$SOCIAL_PORT/__social/health" 2>/dev/null; then
    echo "  social up after ~$((i * 2))s"
    break
  fi
  sleep 2
done

echo "== woo: :$WOO_PORT =="
(cd "$ROOT/crates/wordless-woocommerce-worker" && "$WRANGLER" dev --port "$WOO_PORT" --local --persist-to "$ROOT/.wrangler/state") &
WOO_PID=$!
for i in $(seq 1 240); do
  if curl -fsS -o /dev/null "http://127.0.0.1:$WOO_PORT/__woo/health" 2>/dev/null; then
    echo "  woo up after ~$((i * 2))s"
    break
  fi
  sleep 2
done

echo "== school: :$SCHOOL_PORT =="
(cd "$ROOT/crates/wordless-school-worker" && "$WRANGLER" dev --port "$SCHOOL_PORT" --local --persist-to "$ROOT/.wrangler/state") &
SCHOOL_PID=$!
for i in $(seq 1 240); do
  if curl -fsS -o /dev/null "http://127.0.0.1:$SCHOOL_PORT/__school/health" 2>/dev/null; then
    echo "  school up after ~$((i * 2))s"
    break
  fi
  sleep 2
done

echo "== seo: :$SEO_PORT =="
(cd "$ROOT/crates/wordless-seo-worker" && "$WRANGLER" dev --port "$SEO_PORT" --local --persist-to "$ROOT/.wrangler/state") &
SEO_PID=$!
for i in $(seq 1 240); do
  if curl -fsS -o /dev/null "http://127.0.0.1:$SEO_PORT/__seo/health" 2>/dev/null; then
    echo "  seo up after ~$((i * 2))s"
    break
  fi
  sleep 2
done

echo "== main: :$MAIN_PORT (WORDLESS_CONVERTER_URL/SOCIAL_URL/WOO_URL/SCHOOL_URL/SEO_URL set) =="
"$WRANGLER" dev --port "$MAIN_PORT" --local \
  --var "WORDLESS_CONVERTER_URL:http://127.0.0.1:$CONVERTER_PORT" \
  --var "WORDLESS_SOCIAL_URL:http://127.0.0.1:$SOCIAL_PORT" \
  --var "WORDLESS_WOO_URL:http://127.0.0.1:$WOO_PORT" \
  --var "WORDLESS_SCHOOL_URL:http://127.0.0.1:$SCHOOL_PORT" \
  --var "WORDLESS_SEO_URL:http://127.0.0.1:$SEO_PORT" &
MAIN_PID=$!
wait
