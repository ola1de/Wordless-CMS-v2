#!/usr/bin/env bash
# WORDLESS v2 — social + Woo + school + SEO Workers for local smoke/parity runs.
#
# Sourced (not executed) by scripts/smoke-p7.sh, scripts/smoke-p8.sh,
# scripts/smoke-p9.sh, scripts/smoke-p10.sh and tests/parity/run-parity.sh.
# Expects $ROOT (workspace root) and $WRANGLER (wrangler binary) to be set by the caller.
#
#   source "$ROOT/scripts/split-workers-dev.sh"
#   start_social      # starts the social Worker on $SOCIAL_PORT
#                     # (default 8807), waits for its health endpoint
#   stop_social       # kills it — call from the script's cleanup trap
#   start_woo         # same for the Woo Worker ($WOO_PORT, default 8808)
#   stop_woo
#   start_school      # same for the school Worker ($SCHOOL_PORT, default 8809)
#   stop_school
#   start_seo         # same for the SEO Worker ($SEO_PORT, default 8812)
#   stop_seo
#
# Both start_* functions forward extra arguments to `wrangler dev`
# (e.g. `start_woo --var MOCK_PAYMENT_WEBHOOK_SECRET:$SECRET`):
# the Woo Worker reads its payment secrets from its own env, so a
# run that exercises checkout must pass them to the Woo Worker, not
# just to the main Worker.
#
# Sets SOCIAL_URL / WOO_URL / SCHOOL_URL / SEO_URL (http://127.0.0.1:$PORT) for
# the main Worker's WORDLESS_SOCIAL_URL / WORDLESS_WOO_URL /
# WORDLESS_SCHOOL_URL / WORDLESS_SEO_URL vars. Each Worker's own .wrangler state is
# cleaned first (same clean-room rule as the main worker).
#
# Shared local D1: the social, Woo, school and SEO Workers bind the SAME D1
# database as the main Worker (same database_id in their
# wrangler.tomls; the main Worker still runs all migrations — the
# sub-workers declare no migrations_dir). Under `wrangler dev
# --local` each Worker would otherwise keep its own SQLite file
# under its own .wrangler/state, so both are started with
# --persist-to "$ROOT/.wrangler/state" — the main Worker's local
# state dir — and read the same database file. Verified by the
# smoke suites (they seed users through the main API, then exercise
# social/Woo/school/SEO routes through the proxy).
#
# Added 2026-11-20 (size remedy): social networking (P7) and
# WooCommerce (P8) run in their own Workers, so every local run
# that exercises them must start them first. School (P9) joined
# them the same day, for the same reason. SEO (P10) joined them
# the same day too (2026-11-20 SEO split).
SOCIAL_PORT="${SOCIAL_PORT:-8807}"
SOCIAL_URL="http://127.0.0.1:$SOCIAL_PORT"
SOCIAL_PID=""
WOO_PORT="${WOO_PORT:-8808}"
WOO_URL="http://127.0.0.1:$WOO_PORT"
WOO_PID=""
SCHOOL_PORT="${SCHOOL_PORT:-8809}"
SCHOOL_URL="http://127.0.0.1:$SCHOOL_PORT"
SCHOOL_PID=""
SEO_PORT="${SEO_PORT:-8812}"
SEO_URL="http://127.0.0.1:$SEO_PORT"
SEO_PID=""

start_social() {
  rm -rf "$ROOT/crates/wordless-social-worker/.wrangler"
  echo "== social: starting wordless-social on :$SOCIAL_PORT =="
  (cd "$ROOT/crates/wordless-social-worker" && "$WRANGLER" dev --port "$SOCIAL_PORT" --local --persist-to "$ROOT/.wrangler/state" "$@" > /tmp/social-dev.log 2>&1) &
  SOCIAL_PID=$!
  for i in $(seq 1 240); do
    if curl -fsS -o /dev/null "$SOCIAL_URL/__social/health" 2>/dev/null; then
      echo "  social up after ~$((i * 2))s"
      return 0
    fi
    sleep 2
  done
  echo "FAIL: social did not answer $SOCIAL_URL/__social/health within 480s" >&2
  echo "--- social log (tail) ---" >&2
  tail -40 /tmp/social-dev.log >&2 || true
  return 1
}

stop_social() {
  [ -n "$SOCIAL_PID" ] || return 0
  kill "$SOCIAL_PID" 2>/dev/null || true
  for child in $(pgrep -P "$SOCIAL_PID" 2>/dev/null); do
    for grand in $(pgrep -P "$child" 2>/dev/null); do kill -9 "$grand" 2>/dev/null || true; done
    kill -9 "$child" 2>/dev/null || true
  done
  pkill -f "wrangle[r] dev --port $SOCIAL_PORT" 2>/dev/null || true
}

start_woo() {
  rm -rf "$ROOT/crates/wordless-woocommerce-worker/.wrangler"
  echo "== woo: starting wordless-woocommerce on :$WOO_PORT =="
  (cd "$ROOT/crates/wordless-woocommerce-worker" && "$WRANGLER" dev --port "$WOO_PORT" --local --persist-to "$ROOT/.wrangler/state" "$@" > /tmp/woo-dev.log 2>&1) &
  WOO_PID=$!
  for i in $(seq 1 240); do
    if curl -fsS -o /dev/null "$WOO_URL/__woo/health" 2>/dev/null; then
      echo "  woo up after ~$((i * 2))s"
      return 0
    fi
    sleep 2
  done
  echo "FAIL: woo did not answer $WOO_URL/__woo/health within 480s" >&2
  echo "--- woo log (tail) ---" >&2
  tail -40 /tmp/woo-dev.log >&2 || true
  return 1
}

stop_woo() {
  [ -n "$WOO_PID" ] || return 0
  kill "$WOO_PID" 2>/dev/null || true
  for child in $(pgrep -P "$WOO_PID" 2>/dev/null); do
    for grand in $(pgrep -P "$child" 2>/dev/null); do kill -9 "$grand" 2>/dev/null || true; done
    kill -9 "$child" 2>/dev/null || true
  done
  pkill -f "wrangle[r] dev --port $WOO_PORT" 2>/dev/null || true
}

start_school() {
  rm -rf "$ROOT/crates/wordless-school-worker/.wrangler"
  echo "== school: starting wordless-school on :$SCHOOL_PORT =="
  (cd "$ROOT/crates/wordless-school-worker" && "$WRANGLER" dev --port "$SCHOOL_PORT" --local --persist-to "$ROOT/.wrangler/state" "$@" > /tmp/school-dev.log 2>&1) &
  SCHOOL_PID=$!
  for i in $(seq 1 240); do
    if curl -fsS -o /dev/null "$SCHOOL_URL/__school/health" 2>/dev/null; then
      echo "  school up after ~$((i * 2))s"
      return 0
    fi
    sleep 2
  done
  echo "FAIL: school did not answer $SCHOOL_URL/__school/health within 480s" >&2
  echo "--- school log (tail) ---" >&2
  tail -40 /tmp/school-dev.log >&2 || true
  return 1
}

stop_school() {
  [ -n "$SCHOOL_PID" ] || return 0
  kill "$SCHOOL_PID" 2>/dev/null || true
  for child in $(pgrep -P "$SCHOOL_PID" 2>/dev/null); do
    for grand in $(pgrep -P "$child" 2>/dev/null); do kill -9 "$grand" 2>/dev/null || true; done
    kill -9 "$child" 2>/dev/null || true
  done
  pkill -f "wrangle[r] dev --port $SCHOOL_PORT" 2>/dev/null || true
}

start_seo() {
  rm -rf "$ROOT/crates/wordless-seo-worker/.wrangler"
  echo "== seo: starting wordless-seo on :$SEO_PORT =="
  (cd "$ROOT/crates/wordless-seo-worker" && "$WRANGLER" dev --port "$SEO_PORT" --local --persist-to "$ROOT/.wrangler/state" "$@" > /tmp/seo-dev.log 2>&1) &
  SEO_PID=$!
  for i in $(seq 1 240); do
    if curl -fsS -o /dev/null "$SEO_URL/__seo/health" 2>/dev/null; then
      echo "  seo up after ~$((i * 2))s"
      return 0
    fi
    sleep 2
  done
  echo "FAIL: seo did not answer $SEO_URL/__seo/health within 480s" >&2
  echo "--- seo log (tail) ---" >&2
  tail -40 /tmp/seo-dev.log >&2 || true
  return 1
}

stop_seo() {
  [ -n "$SEO_PID" ] || return 0
  kill "$SEO_PID" 2>/dev/null || true
  for child in $(pgrep -P "$SEO_PID" 2>/dev/null); do
    for grand in $(pgrep -P "$child" 2>/dev/null); do kill -9 "$grand" 2>/dev/null || true; done
    kill -9 "$child" 2>/dev/null || true
  done
  pkill -f "wrangle[r] dev --port $SEO_PORT" 2>/dev/null || true
}
