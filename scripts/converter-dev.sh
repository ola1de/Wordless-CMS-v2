#!/usr/bin/env bash
# WORDLESS v2 — converter Worker for local smoke/parity runs.
#
# Sourced (not executed) by scripts/smoke-*.sh and
# tests/parity/run-parity.sh. Expects $ROOT (workspace root) and
# $WRANGLER (wrangler binary) to be set by the caller.
#
#   source "$ROOT/scripts/converter-dev.sh"
#   start_converter   # starts the converter Worker on $CONVERTER_PORT
#                     # (default 8805), waits for its health endpoint
#   stop_converter    # kills it — call from the script's cleanup trap
#
# Sets CONVERTER_URL (http://127.0.0.1:$CONVERTER_PORT) for the main
# Worker's WORDLESS_CONVERTER_URL var. The converter's own .wrangler
# state is cleaned first (same clean-room rule as the main worker).
#
# Added 2026-11-20 (converter split): the PHP → TypeScript engine
# and the Elementor renderer run in a second Worker, so every local
# run that exercises conversion must start it first.
CONVERTER_PORT="${CONVERTER_PORT:-8805}"
CONVERTER_URL="http://127.0.0.1:$CONVERTER_PORT"
CONVERTER_PID=""

start_converter() {
  rm -rf "$ROOT/crates/wordless-converter/.wrangler"
  echo "== converter: starting wordless-converter on :$CONVERTER_PORT =="
  (cd "$ROOT/crates/wordless-converter" && "$WRANGLER" dev --port "$CONVERTER_PORT" --local > /tmp/converter-dev.log 2>&1) &
  CONVERTER_PID=$!
  for i in $(seq 1 240); do
    if curl -fsS -o /dev/null "$CONVERTER_URL/__convert/health" 2>/dev/null; then
      echo "  converter up after ~$((i * 2))s"
      return 0
    fi
    sleep 2
  done
  echo "FAIL: converter did not answer $CONVERTER_URL/__convert/health within 480s" >&2
  echo "--- converter log (tail) ---" >&2
  tail -40 /tmp/converter-dev.log >&2 || true
  return 1
}

stop_converter() {
  [ -n "$CONVERTER_PID" ] || return 0
  kill "$CONVERTER_PID" 2>/dev/null || true
  for child in $(pgrep -P "$CONVERTER_PID" 2>/dev/null); do
    for grand in $(pgrep -P "$child" 2>/dev/null); do kill -9 "$grand" 2>/dev/null || true; done
    kill -9 "$child" 2>/dev/null || true
  done
  pkill -f "wrangle[r] dev --port $CONVERTER_PORT" 2>/dev/null || true
}
