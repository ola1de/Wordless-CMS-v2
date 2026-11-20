#!/usr/bin/env bash
# WORDLESS v2 P1 smoke test — runs the Rust worker under local wrangler dev
# against a local D1 (migrations applied, one seeded admin) and checks:
#   GET  /api/health        -> {ok:true, data:{status:"ok", version:"2.0.0-dev"}}
#   GET  /api/site-health   -> all checks pass on the seeded database
#   POST /api/auth/login    -> good password 200 + Set-Cookie; bad password 401
#   GET  /api/auth/me       -> 200 with the session cookie, 401 without
#   POST /api/auth/logout   -> 200, cookie cleared, session revoked
#   GET  /admin/            -> placeholder page served from ASSETS
#   GET  /api/nope          -> 404 envelope
#   GET  /api/health (POST) -> 405 envelope
#
# Usage: ./scripts/smoke-p1.sh [--keep]   (--keep leaves wrangler dev running)
# Requires: wrangler (TS repo's project-managed binary), worker-build,
# node. The admin seed password is fixed for local dev only:
#   login "admin", password "smoke-admin-2026"
# The local D1 lives in this workspace's .wrangler/ (never the TS repo's).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"

WRANGLER="$HOME/workspace/wordless/wordless-cms/node_modules/.bin/wrangler"
PORT="${SMOKE_PORT:-8787}"
BASE="http://127.0.0.1:$PORT"
PASS=0
FAIL=0

check() { # check <name> <expected-status> <actual-status> [extra]
  local name="$1" expected="$2" actual="$3"
  if [ "$expected" = "$actual" ]; then
    PASS=$((PASS + 1)); echo "  ok  $name ($actual)"
  else
    FAIL=$((FAIL + 1)); echo "  FAIL $name: expected $expected, got $actual ${4:-}"
  fi
}

check_contains() { # check_contains <name> <file> <substring>
  if grep -qF "$3" "$2"; then
    PASS=$((PASS + 1)); echo "  ok  $1"
  else
    FAIL=$((FAIL + 1)); echo "  FAIL $1: '$3' not found in $2"
  fi
}

echo "== P1 smoke: applying migrations (local D1) =="
"$WRANGLER" d1 migrations apply DB --local >/dev/null

echo "== P1 smoke: seeding admin (login admin / smoke-admin-2026) =="
SEED_SQL="$(mktemp)"
node > "$SEED_SQL" <<'NODE'
const { pbkdf2Sync, randomBytes } = require('node:crypto');
// NOTE: the salt is random per run, so the stored string is regenerated here;
// the Node vector test in the Rust suite covers the fixed-salt case.
const salt = randomBytes(16);
const h = pbkdf2Sync('smoke-admin-2026', salt, 100000, 32, 'sha256');
const stored = 'pbkdf2$100000$' + salt.toString('base64url') + '$' + h.toString('base64url');
const esc = (s) => "'" + s.replace(/'/g, "''") + "'";
const now = new Date().toISOString().replace('T', ' ').replace(/\.\d+Z$/, '');
console.log('INSERT OR IGNORE INTO wp_users (user_login, user_pass, user_nicename, user_email, user_url, user_registered, display_name) VALUES (' + esc('admin') + ', ' + esc(stored) + ', ' + esc('admin') + ', ' + esc('admin@example.com') + ', ' + esc('') + ', ' + esc(now) + ', ' + esc('admin') + ');');
console.log('INSERT OR IGNORE INTO wp_usermeta (user_id, meta_key, meta_value) VALUES ((SELECT ID FROM wp_users WHERE user_login = ' + esc('admin') + '), ' + esc('wp_capabilities') + ', ' + esc('{"administrator":true}') + ');');
NODE
"$WRANGLER" d1 execute DB --local --file "$SEED_SQL" >/dev/null
rm -f "$SEED_SQL"

echo "== P1 smoke: starting wrangler dev on :$PORT =="
"$WRANGLER" dev --port "$PORT" --local > /tmp/wordless-p1-wrangler.log 2>&1 &
DEV_PID=$!
cleanup() { kill "$DEV_PID" 2>/dev/null || true; }
trap cleanup EXIT

# Wait for the worker to answer (wrangler dev builds first — can take a while).
for i in $(seq 1 150); do
  if curl -fsS -o /dev/null "$BASE/api/health" 2>/dev/null; then break; fi
  sleep 2
done

echo "== P1 smoke: checks =="
TMP="$(mktemp -d)"
curl -sS -o "$TMP/health.json" -w "%{http_code}" "$BASE/api/health" > "$TMP/health.code"
check "GET /api/health status" 200 "$(cat "$TMP/health.code")"
check_contains "health envelope ok=true" "$TMP/health.json" '"ok":true'
check_contains "health version 2.0.0-dev" "$TMP/health.json" '"version":"2.0.0-dev"'
check_contains "health status ok" "$TMP/health.json" '"status":"ok"'

curl -sS -o "$TMP/site.json" -w "%{http_code}" "$BASE/api/site-health" > "$TMP/site.code"
check "GET /api/site-health status" 200 "$(cat "$TMP/site.code")"
check_contains "site-health ok" "$TMP/site.json" '"status":"ok"'
check_contains "site-health database pass" "$TMP/site.json" '"name":"database","status":"pass"'
check_contains "site-health tables pass" "$TMP/site.json" '"name":"tables","status":"pass"'
check_contains "site-health bindings pass" "$TMP/site.json" '"name":"bindings","status":"pass"'
check_contains "site-health content counts 1 user" "$TMP/site.json" '1 users'

# Bad password -> 401, generic message.
curl -sS -o "$TMP/bad.json" -w "%{http_code}" -X POST "$BASE/api/auth/login" \
  -H 'Content-Type: application/json' -d '{"login":"admin","password":"wrong"}' > "$TMP/bad.code"
check "POST /api/auth/login wrong password" 401 "$(cat "$TMP/bad.code")"
check_contains "login failure is generic" "$TMP/bad.json" 'Invalid login or password.'

# Good password -> 200 + Set-Cookie.
curl -sS -o "$TMP/login.json" -w "%{http_code}" -D "$TMP/login.headers" -X POST "$BASE/api/auth/login" \
  -H 'Content-Type: application/json' -d '{"login":"admin","password":"smoke-admin-2026"}' > "$TMP/login.code"
check "POST /api/auth/login good password" 200 "$(cat "$TMP/login.code")"
check_contains "login returns admin user" "$TMP/login.json" '"login":"admin"'
check_contains "login sets session cookie" "$TMP/login.headers" 'wordless_session='
COOKIE="$(grep -i '^set-cookie:' "$TMP/login.headers" | head -1 | sed 's/^[^:]*: *//; s/;.*//' | tr -d '\r')"

# /me with cookie -> 200; without -> 401.
curl -sS -o "$TMP/me.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/auth/me" > "$TMP/me.code"
check "GET /api/auth/me with cookie" 200 "$(cat "$TMP/me.code")"
check_contains "me returns admin" "$TMP/me.json" '"role":"administrator"'
curl -sS -o /dev/null -w "%{http_code}" "$BASE/api/auth/me" > "$TMP/me2.code"
check "GET /api/auth/me without cookie" 401 "$(cat "$TMP/me2.code")"

# Logout -> 200, cookie cleared; the same token then fails /me.
curl -sS -o "$TMP/logout.json" -w "%{http_code}" -D "$TMP/logout.headers" -X POST \
  -H "Cookie: $COOKIE" "$BASE/api/auth/logout" > "$TMP/logout.code"
check "POST /api/auth/logout" 200 "$(cat "$TMP/logout.code")"
check_contains "logout clears cookie" "$TMP/logout.headers" 'Max-Age=0'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/auth/me" > "$TMP/me3.code"
check "GET /api/auth/me after logout" 401 "$(cat "$TMP/me3.code")"

# Admin placeholder + unknown route + wrong method.
curl -sS -o "$TMP/admin.html" -w "%{http_code}" "$BASE/admin/" > "$TMP/admin.code"
check "GET /admin/ placeholder" 200 "$(cat "$TMP/admin.code")"
check_contains "admin placeholder content" "$TMP/admin.html" 'Wordless v2.0'
curl -sS -o /dev/null -w "%{http_code}" "$BASE/api/nope" > "$TMP/n.code"
check "GET /api/nope 404" 404 "$(cat "$TMP/n.code")"
curl -sS -o /dev/null -w "%{http_code}" -X POST "$BASE/api/health" > "$TMP/m.code"
check "POST /api/health 405" 405 "$(cat "$TMP/m.code")"

echo
echo "== P1 smoke: $PASS passed, $FAIL failed =="
rm -rf "$TMP"
if [ "${1:-}" = "--keep" ]; then
  echo "Leaving wrangler dev running (PID $DEV_PID) on :$PORT"
  trap - EXIT
else
  cleanup
fi
[ "$FAIL" -eq 0 ]
