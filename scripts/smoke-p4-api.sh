#!/usr/bin/env bash
# WORDLESS v2 P4 (API) smoke test — runs the Rust worker under local wrangler dev
# against a local D1 (migrations applied, one seeded admin) and checks the
# P4 API surface end-to-end:
#   CPT/taxonomy REST: CRUD, duplicates 409, reserved slugs 400, unknown 404,
#     admin-only writes (401 unauthenticated, 403 editor), delete-in-use 409
#   user profiles: GET single, PUT profile (displayName/bio/theme/avatar),
#     theme persists, invalid theme 400, duplicate email 409, self-vs-other
#     rules (403 editing another user without edit_users), theme null when unset
#   forced password reset: admin reset -> temp-password login with
#     mustChangePassword=true -> /me flag -> wrong-current 400 ->
#     change-password -> temp dead, new password logs in, flag false
#   scheduling: past publishAt publishes on update, future publishAt stays
#     draft + hidden from the public site (anonymous single 401, home has no
#     title), clearing the schedule, invalid publishAt 400
#   plugins: GET /api/plugins (401 unauthenticated, empty before install),
#     marketplace install -> list shows verdict + active state, activate /
#     deactivate aliases, unknown slug 404
#   per-plugin settings: GET returns carried-over defaults from the
#     report's optionDefaults, PUT merges overrides, null deletes an
#     override (default shows again), non-object 400, unknown 404
#   featured image: POST/PUT /api/posts featuredImageId writes
#     _thumbnail_id and reads it back; null clears; bad type/id 400
#   post meta: PUT /api/posts/:id/meta sets/deletes keys, blocks
#     _wl_publish_at, validates _thumbnail_id, unknown post 404
#   login branding: public GET /api/login-branding defaults (v1 login look),
#     write via PUT /api/settings (branding fields), bad accent 400
#
# Usage: ./scripts/smoke-p4-api.sh [--keep]   (--keep leaves wrangler dev running)
# Clean room first: rm -rf .wrangler   (the script applies migrations itself)
# Requires: wrangler (TS repo's project-managed binary), worker-build, node.
# The admin seed password is fixed for local dev only:
#   login "admin", password "smoke-admin-2026"
# The local D1 lives in this workspace's .wrangler/ (never the TS repo's).
# NOT covered live here (unit-tested in their crates): the cron sweep firing
# (local wrangler dev fires no real cron — the lazy publish check is the
# live half; the sweep is the same service code, unit-tested), TOTP,
# reset-password email.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"

WRANGLER="$HOME/workspace/wordless/wordless-cms/node_modules/.bin/wrangler"
HELLO_PHP="$HOME/workspace/wordless/reference/wordpress-7.1.5/wp-content/plugins/hello.php"
PORT="${SMOKE_PORT:-8799}"
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

check_absent() { # check_absent <name> <file> <substring>
  if grep -qF "$3" "$2"; then
    FAIL=$((FAIL + 1)); echo "  FAIL $1: '$3' unexpectedly present in $2"
  else
    PASS=$((PASS + 1)); echo "  ok  $1"
  fi
}

# jget <file> <js-expression-on-d> — extracts a value from an envelope JSON.
jget() {
  node -e 'const fs=require("fs");const d=JSON.parse(fs.readFileSync(process.argv[1],"utf8"));let v="";try{v=eval(process.argv[2]);}catch(e){v="";}console.log(v===undefined||v===null?"":v);' "$1" "$2"
}

echo "== P4 smoke: applying migrations (local D1) =="
"$WRANGLER" d1 migrations apply DB --local >/dev/null

echo "== P4 smoke: seeding admin (login admin / smoke-admin-2026) =="
SEED_SQL="$(mktemp)"
node > "$SEED_SQL" <<'NODE'
const { pbkdf2Sync, randomBytes } = require('node:crypto');
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

# Split (2026-11-20): conversion runs in the converter Worker — start
# it first, and point the main Worker at it.
source "$ROOT/scripts/converter-dev.sh"
start_converter
echo "== P4 smoke: starting wrangler dev on :$PORT =="
"$WRANGLER" dev --port "$PORT" --local --var "WORDLESS_CONVERTER_URL:$CONVERTER_URL" >/dev/null 2>&1 &
DEV_PID=$!
cleanup() { kill "$DEV_PID" 2>/dev/null || true; stop_converter; }
trap cleanup EXIT

# Wait for the worker to answer (wrangler dev builds first — can take a while).
for i in $(seq 1 240); do
  if curl -fsS -o /dev/null "$BASE/api/health" 2>/dev/null; then break; fi
  sleep 2
done

echo "== P4 smoke: checks =="
TMP="$(mktemp -d)"

# login_as <login> <password> <cookie-out-file> — sets $LOGIN_CODE
login_as() {
  curl -sS -o "$TMP/login-$1.json" -w "%{http_code}" -D "$TMP/login-$1.headers" -X POST "$BASE/api/auth/login" \
    -H 'Content-Type: application/json' -d "{\"login\":\"$1\",\"password\":\"$2\"}" > "$TMP/login-$1.code"
  LOGIN_CODE="$(cat "$TMP/login-$1.code")"
  # NOTE: the grep finds nothing on a failed login (no Set-Cookie) —
  # the `|| true` keeps `set -euo pipefail` from aborting the script.
  COOKIE="$(grep -i '^set-cookie:' "$TMP/login-$1.headers" | head -1 | sed 's/^[^:]*: *//;s/;.*//' | tr -d '\r' || true)"
  printf '%s' "$COOKIE" > "$3"
}
cookie_of() { cat "$1"; }

login_as admin smoke-admin-2026 "$TMP/cookie-admin"
check "login as admin" 200 "$LOGIN_CODE"
ADMIN_COOKIE="$(cookie_of "$TMP/cookie-admin")"

# --- health ---
curl -sS -o "$TMP/health.json" -w "%{http_code}" "$BASE/api/health" > "$TMP/health.code"
check "GET /api/health status" 200 "$(cat "$TMP/health.code")"
check_contains "health envelope ok=true" "$TMP/health.json" '"ok":true'

# --- login branding (public defaults) ---
curl -sS -o "$TMP/branding.json" -w "%{http_code}" "$BASE/api/login-branding" > "$TMP/branding.code"
check "GET /api/login-branding (no auth)" 200 "$(cat "$TMP/branding.code")"
check_contains "branding default accent is teal" "$TMP/branding.json" '"accent":"#0d9488"'
check_contains "branding default deep is teal sidebar" "$TMP/branding.json" '"deep":"#07332e"'
check_contains "branding default bgMode soft" "$TMP/branding.json" '"bgMode":"soft"'
check_contains "branding default layout card" "$TMP/branding.json" '"layout":"card"'
check_contains "branding default heading" "$TMP/branding.json" '"heading":"Welcome back"'
check_contains "branding default showBackLink true" "$TMP/branding.json" '"showBackLink":true'
check_contains "branding default showSocial false" "$TMP/branding.json" '"showSocial":false'

# --- post types ---
curl -sS -o "$TMP/cpt.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/post-types" \
  -H 'Content-Type: application/json' -d '{"slug":"book","label":"Books","singularLabel":"Book","description":"Long-form works.","supports":["title","editor","thumbnail"],"rewriteSlug":"books"}' > "$TMP/cpt.code"
check "POST /api/post-types book" 201 "$(cat "$TMP/cpt.code")"
check_contains "CPT book created" "$TMP/cpt.json" '"slug":"book"'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/post-types" \
  -H 'Content-Type: application/json' -d '{"slug":"book","label":"Books"}' > "$TMP/cpt-dup.code"
check "POST /api/post-types duplicate book" 409 "$(cat "$TMP/cpt-dup.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/post-types" \
  -H 'Content-Type: application/json' -d '{"slug":"post","label":"Posts"}' > "$TMP/cpt-res.code"
check "POST /api/post-types reserved slug post" 400 "$(cat "$TMP/cpt-res.code")"
curl -sS -o /dev/null -w "%{http_code}" -X POST "$BASE/api/post-types" \
  -H 'Content-Type: application/json' -d '{"slug":"movie","label":"Movies"}' > "$TMP/cpt-noauth.code"
check "POST /api/post-types unauthenticated" 401 "$(cat "$TMP/cpt-noauth.code")"
curl -sS -o "$TMP/cpt-list.json" -w "%{http_code}" "$BASE/api/post-types" > "$TMP/cpt-list.code"
check "GET /api/post-types (public)" 200 "$(cat "$TMP/cpt-list.code")"
check_contains "CPT list has built-in post" "$TMP/cpt-list.json" '"slug":"post"'
check_contains "CPT list has book" "$TMP/cpt-list.json" '"slug":"book"'
curl -sS -o "$TMP/cpt-one.json" -w "%{http_code}" "$BASE/api/post-types/book" > "$TMP/cpt-one.code"
check "GET /api/post-types/book" 200 "$(cat "$TMP/cpt-one.code")"
curl -sS -o /dev/null -w "%{http_code}" "$BASE/api/post-types/nope" > "$TMP/cpt-404.code"
check "GET /api/post-types/nope" 404 "$(cat "$TMP/cpt-404.code")"
curl -sS -o "$TMP/cpt-put.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/post-types/book" \
  -H 'Content-Type: application/json' -d '{"label":"Books (Revised)"}' > "$TMP/cpt-put.code"
check "PUT /api/post-types/book" 200 "$(cat "$TMP/cpt-put.code")"
check_contains "CPT label updated" "$TMP/cpt-put.json" 'Books (Revised)'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/post-types/post" \
  -H 'Content-Type: application/json' -d '{"label":"Nope"}' > "$TMP/cpt-put-builtin.code"
check "PUT /api/post-types/post (built-in)" 400 "$(cat "$TMP/cpt-put-builtin.code")"

# --- taxonomies ---
curl -sS -o "$TMP/tax.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/taxonomies" \
  -H 'Content-Type: application/json' -d '{"slug":"genre","label":"Genres","singularLabel":"Genre","hierarchical":true,"objectTypes":["book"]}' > "$TMP/tax.code"
check "POST /api/taxonomies genre" 201 "$(cat "$TMP/tax.code")"
check_contains "taxonomy genre created" "$TMP/tax.json" '"slug":"genre"'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/taxonomies" \
  -H 'Content-Type: application/json' -d '{"slug":"genre","label":"Genres"}' > "$TMP/tax-dup.code"
check "POST /api/taxonomies duplicate genre" 409 "$(cat "$TMP/tax-dup.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/taxonomies" \
  -H 'Content-Type: application/json' -d '{"slug":"category","label":"Categories"}' > "$TMP/tax-res.code"
check "POST /api/taxonomies reserved slug category" 400 "$(cat "$TMP/tax-res.code")"
curl -sS -o "$TMP/tax-list.json" -w "%{http_code}" "$BASE/api/taxonomies" > "$TMP/tax-list.code"
check "GET /api/taxonomies (public)" 200 "$(cat "$TMP/tax-list.code")"
check_contains "taxonomy list has category" "$TMP/tax-list.json" '"slug":"category"'
check_contains "taxonomy list has genre" "$TMP/tax-list.json" '"slug":"genre"'
curl -sS -o "$TMP/tax-put.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/taxonomies/genre" \
  -H 'Content-Type: application/json' -d '{"label":"Genres (Revised)"}' > "$TMP/tax-put.code"
check "PUT /api/taxonomies/genre" 200 "$(cat "$TMP/tax-put.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X DELETE "$BASE/api/taxonomies/category" > "$TMP/tax-del-builtin.code"
check "DELETE /api/taxonomies/category (built-in)" 400 "$(cat "$TMP/tax-del-builtin.code")"

# --- users: create editor + subscriber for the profile/reset checks ---
curl -sS -o "$TMP/ed.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/users" \
  -H 'Content-Type: application/json' -d '{"login":"ed","email":"ed@example.com","password":"ed-pass-123","displayName":"Ed Editor","role":"editor"}' > "$TMP/ed.code"
check "POST /api/users ed (editor)" 201 "$(cat "$TMP/ed.code")"
ED_ID="$(jget "$TMP/ed.json" 'd.data.id')"
curl -sS -o "$TMP/carol.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/users" \
  -H 'Content-Type: application/json' -d '{"login":"carol","email":"carol@example.com","password":"carol-pass-123","displayName":"Carol","role":"subscriber"}' > "$TMP/carol.code"
check "POST /api/users carol (subscriber)" 201 "$(cat "$TMP/carol.code")"
CAROL_ID="$(jget "$TMP/carol.json" 'd.data.id')"
echo "  (ed id: $ED_ID, carol id: $CAROL_ID)"
login_as ed ed-pass-123 "$TMP/cookie-ed"
check "login as ed" 200 "$LOGIN_CODE"
ED_COOKIE="$(cookie_of "$TMP/cookie-ed")"

# editor cannot manage post types (manage_options is admin-only)
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ED_COOKIE" -X POST "$BASE/api/post-types" \
  -H 'Content-Type: application/json' -d '{"slug":"movie","label":"Movies"}' > "$TMP/cpt-ed.code"
check "POST /api/post-types as editor" 403 "$(cat "$TMP/cpt-ed.code")"

# --- profiles ---
curl -sS -o "$TMP/prof.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/users/1" > "$TMP/prof.code"
check "GET /api/users/1 (self)" 200 "$(cat "$TMP/prof.code")"
check_contains "profile theme is null when unset" "$TMP/prof.json" '"theme":null'
check_contains "profile mustChangePassword false" "$TMP/prof.json" '"mustChangePassword":false'
curl -sS -o /dev/null -w "%{http_code}" "$BASE/api/users/1" > "$TMP/prof-noauth.code"
check "GET /api/users/1 unauthenticated" 401 "$(cat "$TMP/prof-noauth.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ED_COOKIE" "$BASE/api/users/1" > "$TMP/prof-other.code"
check "GET /api/users/1 as editor (other user, no edit_users)" 403 "$(cat "$TMP/prof-other.code")"
# avatar: upload a media attachment first, then set it as the avatar
printf 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8Xw8AAwMCAO+ip1sAAAAASUVORK5CYII=' | base64 -d > "$TMP/smoke.png"
curl -sS -o "$TMP/upload.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/media" \
  -F "file=@$TMP/smoke.png;filename=smoke.png;type=image/png" -F "title=Avatar Image" > "$TMP/upload.code"
check "POST /api/media (avatar image)" 201 "$(cat "$TMP/upload.code")"
MEDIA_ID="$(jget "$TMP/upload.json" 'd.data.id')"
curl -sS -o "$TMP/prof-put.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/users/1/profile" \
  -H 'Content-Type: application/json' -d "{\"displayName\":\"Admin Renamed\",\"bio\":\"Site owner and editor.\",\"theme\":\"forest\",\"avatarId\":$MEDIA_ID}" > "$TMP/prof-put.code"
check "PUT /api/users/1/profile" 200 "$(cat "$TMP/prof-put.code")"
check_contains "profile displayName updated" "$TMP/prof-put.json" 'Admin Renamed'
check_contains "profile bio updated" "$TMP/prof-put.json" 'Site owner and editor.'
check_contains "profile theme forest" "$TMP/prof-put.json" '"theme":"forest"'
check_contains "profile avatarId set" "$TMP/prof-put.json" "\"avatarId\":$MEDIA_ID"
curl -sS -o "$TMP/prof2.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/users/1" > "$TMP/prof2.code"
check "GET /api/users/1 after profile update" 200 "$(cat "$TMP/prof2.code")"
check_contains "profile theme persists" "$TMP/prof2.json" '"theme":"forest"'
check_contains "profile bio persists" "$TMP/prof2.json" 'Site owner and editor.'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/users/1/profile" \
  -H 'Content-Type: application/json' -d '{"theme":"neon"}' > "$TMP/prof-bad-theme.code"
check "PUT profile invalid theme" 400 "$(cat "$TMP/prof-bad-theme.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/users/1/profile" \
  -H 'Content-Type: application/json' -d '{"email":"carol@example.com"}' > "$TMP/prof-dup-email.code"
check "PUT profile duplicate email" 409 "$(cat "$TMP/prof-dup-email.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/users/1/profile" \
  -H 'Content-Type: application/json' -d '{"avatarId":999999}' > "$TMP/prof-bad-avatar.code"
check "PUT profile unknown avatar attachment" 400 "$(cat "$TMP/prof-bad-avatar.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ED_COOKIE" -X PUT "$BASE/api/users/1/profile" \
  -H 'Content-Type: application/json' -d '{"displayName":"Hacked"}' > "$TMP/prof-ed-other.code"
check "PUT profile of another user as editor" 403 "$(cat "$TMP/prof-ed-other.code")"
curl -sS -o "$TMP/prof-ed-self.json" -w "%{http_code}" -H "Cookie: $ED_COOKIE" -X PUT "$BASE/api/users/$ED_ID/profile" \
  -H 'Content-Type: application/json' -d '{"theme":"ocean"}' > "$TMP/prof-ed-self.code"
check "PUT own profile as editor" 200 "$(cat "$TMP/prof-ed-self.code")"
check_contains "editor own theme ocean" "$TMP/prof-ed-self.json" '"theme":"ocean"'

# --- login branding write path (PUT /api/settings) ---
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/settings" \
  -H 'Content-Type: application/json' -d '{"loginAccent":"#2563eb","loginHeading":"Sign in to Wordless","loginShowSocial":true}' > "$TMP/brand-put.code"
check "PUT /api/settings branding fields" 200 "$(cat "$TMP/brand-put.code")"
curl -sS -o "$TMP/branding2.json" -w "%{http_code}" "$BASE/api/login-branding" > "$TMP/branding2.code"
check "GET /api/login-branding after write" 200 "$(cat "$TMP/branding2.code")"
check_contains "branding accent updated" "$TMP/branding2.json" '"accent":"#2563eb"'
check_contains "branding heading updated" "$TMP/branding2.json" '"heading":"Sign in to Wordless"'
check_contains "branding showSocial updated" "$TMP/branding2.json" '"showSocial":true'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/settings" \
  -H 'Content-Type: application/json' -d '{"loginAccent":"red"}' > "$TMP/brand-bad.code"
check "PUT /api/settings bad accent" 400 "$(cat "$TMP/brand-bad.code")"
# restore the v1-look defaults for later runs
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/settings" \
  -H 'Content-Type: application/json' -d '{"loginAccent":"#0d9488","loginHeading":"Welcome back","loginShowSocial":false}' > "$TMP/brand-restore.code"
check "PUT /api/settings restore branding" 200 "$(cat "$TMP/brand-restore.code")"

# --- forced password reset flow (carol) ---
curl -sS -o "$TMP/reset.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/users/$CAROL_ID/reset-password" > "$TMP/reset.code"
check "POST /api/users/:id/reset-password" 200 "$(cat "$TMP/reset.code")"
TEMP_PASS="$(jget "$TMP/reset.json" 'd.data.temporaryPassword')"
if [ -z "$TEMP_PASS" ]; then echo "  FAIL temp password missing from reset response"; FAIL=$((FAIL + 1)); else PASS=$((PASS + 1)); echo "  ok  reset returns a temporary password"; fi
login_as carol "$TEMP_PASS" "$TMP/cookie-carol"
check "login as carol with temp password" 200 "$LOGIN_CODE"
check_contains "temp login signals mustChangePassword" "$TMP/login-carol.json" '"mustChangePassword":true'
CAROL_COOKIE="$(cookie_of "$TMP/cookie-carol")"
curl -sS -o "$TMP/me-carol.json" -w "%{http_code}" -H "Cookie: $CAROL_COOKIE" "$BASE/api/auth/me" > "$TMP/me-carol.code"
check "GET /api/auth/me as carol (temp login)" 200 "$(cat "$TMP/me-carol.code")"
check_contains "me signals mustChangePassword" "$TMP/me-carol.json" '"mustChangePassword":true'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $CAROL_COOKIE" -X POST "$BASE/api/auth/change-password" \
  -H 'Content-Type: application/json' -d '{"currentPassword":"wrong-current","newPassword":"carol-new-pass-1"}' > "$TMP/chg-wrong.code"
check "POST /api/auth/change-password wrong current" 400 "$(cat "$TMP/chg-wrong.code")"
curl -sS -o "$TMP/chg.json" -w "%{http_code}" -H "Cookie: $CAROL_COOKIE" -X POST "$BASE/api/auth/change-password" \
  -H 'Content-Type: application/json' -d "{\"currentPassword\":\"$TEMP_PASS\",\"newPassword\":\"carol-new-pass-1\"}" > "$TMP/chg.code"
check "POST /api/auth/change-password with temp as current" 200 "$(cat "$TMP/chg.code")"
check_contains "change-password reports changed" "$TMP/chg.json" '"changed":true'
login_as carol "$TEMP_PASS" "$TMP/cookie-carol2"
check "login as carol with old temp password after change" 401 "$LOGIN_CODE"
login_as carol carol-new-pass-1 "$TMP/cookie-carol3"
check "login as carol with new password" 200 "$LOGIN_CODE"
check_contains "new-password login has no forced change" "$TMP/login-carol.json" '"mustChangePassword":false'

# --- scheduling ---
FUTURE_ISO="$(node -e 'console.log(new Date(Date.now()+3600*1000).toISOString())')"
curl -sS -o "$TMP/sched-past.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/posts" \
  -H 'Content-Type: application/json' -d '{"title":"Sched Past Post","content":"Due already.","status":"draft"}' > "$TMP/sched-past.code"
check "POST /api/posts sched-past draft" 201 "$(cat "$TMP/sched-past.code")"
PAST_ID="$(jget "$TMP/sched-past.json" 'd.data.id')"
curl -sS -o "$TMP/sched-past-put.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/posts/$PAST_ID" \
  -H 'Content-Type: application/json' -d '{"publishAt":"2026-01-01T09:00:00.000Z"}' > "$TMP/sched-past-put.code"
check "PUT publishAt in the past" 200 "$(cat "$TMP/sched-past-put.code")"
check_contains "past publishAt publishes now" "$TMP/sched-past-put.json" '"status":"publish"'
curl -sS -o "$TMP/home.json" -w "%{http_code}" "$BASE/" > "$TMP/home.code"
check "GET / home" 200 "$(cat "$TMP/home.code")"
check_contains "published scheduled post is on the home page" "$TMP/home.json" 'Sched Past Post'
curl -sS -o "$TMP/sched-future.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/posts" \
  -H 'Content-Type: application/json' -d "{\"title\":\"Sched Future Post\",\"content\":\"Not yet.\",\"status\":\"publish\",\"publishAt\":\"$FUTURE_ISO\"}" > "$TMP/sched-future.code"
check "POST /api/posts with future publishAt" 201 "$(cat "$TMP/sched-future.code")"
check_contains "future publishAt stores as draft" "$TMP/sched-future.json" '"status":"draft"'
check_contains "future publishAt is exposed" "$TMP/sched-future.json" "\"publishAt\":\"$FUTURE_ISO\""
FUTURE_ID="$(jget "$TMP/sched-future.json" 'd.data.id')"
curl -sS -o /dev/null -w "%{http_code}" "$BASE/api/posts/$FUTURE_ID" > "$TMP/sched-future-anon.code"
check "GET future scheduled post anonymously" 401 "$(cat "$TMP/sched-future-anon.code")"
curl -sS -o "$TMP/home2.html" -w "%{http_code}" "$BASE/" > "$TMP/home2.code"
check "GET / home again" 200 "$(cat "$TMP/home2.code")"
check_absent "future scheduled post is NOT on the home page" "$TMP/home2.html" 'Sched Future Post'
curl -sS -o "$TMP/sched-future-get.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/posts/$FUTURE_ID" > "$TMP/sched-future-get.code"
check "GET future scheduled post as admin" 200 "$(cat "$TMP/sched-future-get.code")"
check_contains "admin still sees the draft + schedule" "$TMP/sched-future-get.json" '"status":"draft"'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/posts/$FUTURE_ID" \
  -H 'Content-Type: application/json' -d '{"publishAt":"not-a-date"}' > "$TMP/sched-bad.code"
check "PUT invalid publishAt" 400 "$(cat "$TMP/sched-bad.code")"
curl -sS -o "$TMP/sched-clear.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/posts/$FUTURE_ID" \
  -H 'Content-Type: application/json' -d '{"publishAt":null}' > "$TMP/sched-clear.code"
check "PUT publishAt null clears the schedule" 200 "$(cat "$TMP/sched-clear.code")"
check_absent "cleared schedule has no publishAt" "$TMP/sched-clear.json" 'publishAt'

# --- featured image + post meta (MEDIA_ID is the avatar upload above) ---
curl -sS -o "$TMP/feat-create.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/posts" \
  -H 'Content-Type: application/json' -d "{\"title\":\"Featured Post\",\"content\":\"Has an image.\",\"status\":\"draft\",\"featuredImageId\":$MEDIA_ID}" > "$TMP/feat-create.code"
check "POST /api/posts with featuredImageId" 201 "$(cat "$TMP/feat-create.code")"
check_contains "create response carries featuredImageId" "$TMP/feat-create.json" "\"featuredImageId\":$MEDIA_ID"
FEAT_ID="$(jget "$TMP/feat-create.json" 'd.data.id')"
curl -sS -o "$TMP/feat-get.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/posts/$FEAT_ID" > "$TMP/feat-get.code"
check "GET post with featured image" 200 "$(cat "$TMP/feat-get.code")"
check_contains "single-post read carries featuredImageId" "$TMP/feat-get.json" "\"featuredImageId\":$MEDIA_ID"
curl -sS -o "$TMP/feat-clear.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/posts/$FEAT_ID" \
  -H 'Content-Type: application/json' -d '{"featuredImageId":null}' > "$TMP/feat-clear.code"
check "PUT featuredImageId null clears it" 200 "$(cat "$TMP/feat-clear.code")"
check_absent "cleared post has no featuredImageId" "$TMP/feat-clear.json" 'featuredImageId'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/posts/$FEAT_ID" \
  -H 'Content-Type: application/json' -d '{"featuredImageId":"not-a-number"}' > "$TMP/feat-bad-type.code"
check "PUT featuredImageId wrong type" 400 "$(cat "$TMP/feat-bad-type.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/posts/$FEAT_ID" \
  -H 'Content-Type: application/json' -d '{"featuredImageId":999999}' > "$TMP/feat-bad-id.code"
check "PUT featuredImageId unknown attachment" 400 "$(cat "$TMP/feat-bad-id.code")"
# PAST_ID is the published schedule test post: a real post, not an attachment.
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/posts/$FEAT_ID" \
  -H 'Content-Type: application/json' -d "{\"featuredImageId\":$PAST_ID}" > "$TMP/feat-bad-nonthumb.code"
check "PUT featuredImageId non-attachment post" 400 "$(cat "$TMP/feat-bad-nonthumb.code")"
# generic meta route: set thumbnail + a custom key, read back via the post
curl -sS -o "$TMP/meta-put.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/posts/$FEAT_ID/meta" \
  -H 'Content-Type: application/json' -d "{\"_thumbnail_id\":$MEDIA_ID,\"custom_key\":\"v\"}" > "$TMP/meta-put.code"
check "PUT /api/posts/:id/meta" 200 "$(cat "$TMP/meta-put.code")"
check_contains "meta map has custom key" "$TMP/meta-put.json" '"custom_key":"v"'
check_contains "meta map has thumbnail" "$TMP/meta-put.json" "\"_thumbnail_id\":\"$MEDIA_ID\""
curl -sS -o "$TMP/feat-get2.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/posts/$FEAT_ID" > "$TMP/feat-get2.code"
check "GET post after meta-set thumbnail" 200 "$(cat "$TMP/feat-get2.code")"
check_contains "meta-set thumbnail reads back as featuredImageId" "$TMP/feat-get2.json" "\"featuredImageId\":$MEDIA_ID"
curl -sS -o "$TMP/meta-del.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/posts/$FEAT_ID/meta" \
  -H 'Content-Type: application/json' -d '{"custom_key":null}' > "$TMP/meta-del.code"
check "PUT meta null deletes the key" 200 "$(cat "$TMP/meta-del.code")"
check_absent "deleted meta key is gone" "$TMP/meta-del.json" 'custom_key'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/posts/$FEAT_ID/meta" \
  -H 'Content-Type: application/json' -d '{"_wl_publish_at":"2026-12-01T09:00:00.000Z"}' > "$TMP/meta-blocked.code"
check "PUT meta _wl_publish_at blocked" 400 "$(cat "$TMP/meta-blocked.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/posts/$FEAT_ID/meta" \
  -H 'Content-Type: application/json' -d '{"_thumbnail_id":999999}' > "$TMP/meta-bad-thumb.code"
check "PUT meta unknown thumbnail" 400 "$(cat "$TMP/meta-bad-thumb.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/posts/999999/meta" \
  -H 'Content-Type: application/json' -d '{"k":"v"}' > "$TMP/meta-404.code"
check "PUT meta unknown post" 404 "$(cat "$TMP/meta-404.code")"
curl -sS -o /dev/null -w "%{http_code}" -X PUT "$BASE/api/posts/$FEAT_ID/meta" \
  -H 'Content-Type: application/json' -d '{"k":"v"}' > "$TMP/meta-noauth.code"
check "PUT meta unauthenticated" 401 "$(cat "$TMP/meta-noauth.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ED_COOKIE" -X PUT "$BASE/api/posts/$FEAT_ID/meta" \
  -H 'Content-Type: application/json' -d '{"k":"v"}' > "$TMP/meta-ed.code"
check "PUT meta as editor (edit_posts guard)" 200 "$(cat "$TMP/meta-ed.code")"

# --- CPT delete-in-use, then cleanup ---
curl -sS -o "$TMP/book-post.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/posts" \
  -H 'Content-Type: application/json' -d '{"title":"Dune","content":"Spice.","status":"draft","type":"book"}' > "$TMP/book-post.code"
check "POST /api/posts type book" 201 "$(cat "$TMP/book-post.code")"
BOOK_POST_ID="$(jget "$TMP/book-post.json" 'd.data.id')"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X DELETE "$BASE/api/post-types/book" > "$TMP/cpt-del-inuse.code"
check "DELETE /api/post-types/book while in use" 409 "$(cat "$TMP/cpt-del-inuse.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X DELETE "$BASE/api/posts/$BOOK_POST_ID" > "$TMP/book-post-del.code"
check "DELETE the book post" 200 "$(cat "$TMP/book-post-del.code")"
curl -sS -o "$TMP/cpt-del.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X DELETE "$BASE/api/post-types/book" > "$TMP/cpt-del.code"
check "DELETE /api/post-types/book" 200 "$(cat "$TMP/cpt-del.code")"
check_contains "CPT deleted" "$TMP/cpt-del.json" '"deleted":true'
curl -sS -o "$TMP/tax-del.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X DELETE "$BASE/api/taxonomies/genre" > "$TMP/tax-del.code"
check "DELETE /api/taxonomies/genre" 200 "$(cat "$TMP/tax-del.code")"
check_contains "taxonomy deleted" "$TMP/tax-del.json" '"deleted":true'

# --- plugins ---
curl -sS -o /dev/null -w "%{http_code}" "$BASE/api/plugins" > "$TMP/plugins-noauth.code"
check "GET /api/plugins unauthenticated" 401 "$(cat "$TMP/plugins-noauth.code")"
curl -sS -o "$TMP/plugins0.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/plugins" > "$TMP/plugins0.code"
check "GET /api/plugins before install" 200 "$(cat "$TMP/plugins0.code")"
check_contains "plugins list is empty before install" "$TMP/plugins0.json" '"data":[]'
python3 - "$TMP/hello-dolly.zip" "$HELLO_PHP" <<'PYEOF'
import json, sys, zipfile
out, hello = sys.argv[1], sys.argv[2]
manifest = {"slug": "hello-dolly", "name": "Hello Dolly", "version": "1.7.2",
            "description": "A plugin that prints a random lyric."}
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
    z.writestr("hello-dolly/wordless.json", json.dumps(manifest))
    z.writestr("hello-dolly/hello.php", open(hello, encoding="utf-8").read())
PYEOF
curl -sS -o "$TMP/mkt-install.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/marketplace/install" \
  -F "file=@$TMP/hello-dolly.zip;filename=hello-dolly.zip;type=application/zip" > "$TMP/mkt-install.code"
check "POST /api/marketplace/install hello-dolly" 201 "$(cat "$TMP/mkt-install.code")"
curl -sS -o "$TMP/plugins.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/plugins" > "$TMP/plugins.code"
check "GET /api/plugins after install" 200 "$(cat "$TMP/plugins.code")"
check_contains "plugins list has hello-dolly" "$TMP/plugins.json" '"slug":"hello-dolly"'
check_contains "plugins verdict compatible" "$TMP/plugins.json" '"verdict":"compatible"'
check_contains "plugins hello-dolly inactive" "$TMP/plugins.json" '"active":false'
curl -sS -o "$TMP/plugins-act.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/plugins/hello-dolly/activate" > "$TMP/plugins-act.code"
check "POST /api/plugins/hello-dolly/activate" 200 "$(cat "$TMP/plugins-act.code")"
check_contains "plugin activated" "$TMP/plugins-act.json" '"status":"active"'
curl -sS -o "$TMP/plugins2.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/plugins" > "$TMP/plugins2.code"
check "GET /api/plugins after activate" 200 "$(cat "$TMP/plugins2.code")"
check_contains "plugins hello-dolly active" "$TMP/plugins2.json" '"active":true'
curl -sS -o "$TMP/plugins-deact.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/plugins/hello-dolly/deactivate" > "$TMP/plugins-deact.code"
check "POST /api/plugins/hello-dolly/deactivate" 200 "$(cat "$TMP/plugins-deact.code")"
check_contains "plugin deactivated" "$TMP/plugins-deact.json" '"status":"inactive"'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/plugins/nope/activate" > "$TMP/plugins-404.code"
check "POST /api/plugins/nope/activate" 404 "$(cat "$TMP/plugins-404.code")"

# --- per-plugin settings (demo-plugin fixture with literal option defaults) ---
python3 - "$TMP/demo-plugin.zip" <<'PYEOF'
import json, sys, zipfile
out = sys.argv[1]
manifest = {"slug": "demo-plugin", "name": "Demo Plugin", "version": "1.0.0",
            "description": "Fixture plugin with literal option defaults."}
php = """<?php
// Demo plugin: literal get_option defaults for the settings smoke test.
$enabled = get_option('demo_enabled', true);
$label = get_option('demo_label', 'Demo');
$limit = get_option('demo_limit', 10);
"""
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
    z.writestr("demo-plugin/wordless.json", json.dumps(manifest))
    z.writestr("demo-plugin/demo.php", php)
PYEOF
curl -sS -o "$TMP/demo-install.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/marketplace/install" \
  -F "file=@$TMP/demo-plugin.zip;filename=demo-plugin.zip;type=application/zip" > "$TMP/demo-install.code"
check "POST /api/marketplace/install demo-plugin" 201 "$(cat "$TMP/demo-install.code")"
curl -sS -o "$TMP/psettings.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/plugins/demo-plugin/settings" > "$TMP/psettings.code"
check "GET /api/plugins/demo-plugin/settings" 200 "$(cat "$TMP/psettings.code")"
check_contains "settings default demo_enabled true" "$TMP/psettings.json" '"demo_enabled":true'
check_contains "settings default demo_label Demo" "$TMP/psettings.json" '"demo_label":"Demo"'
check_contains "settings default demo_limit 10" "$TMP/psettings.json" '"demo_limit":10'
curl -sS -o "$TMP/psettings-put.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/plugins/demo-plugin/settings" \
  -H 'Content-Type: application/json' -d '{"demo_label":"Changed","demo_extra":"extra"}' > "$TMP/psettings-put.code"
check "PUT /api/plugins/demo-plugin/settings" 200 "$(cat "$TMP/psettings-put.code")"
check_contains "settings override demo_label" "$TMP/psettings-put.json" '"demo_label":"Changed"'
check_contains "settings new key demo_extra" "$TMP/psettings-put.json" '"demo_extra":"extra"'
check_contains "settings keeps default demo_enabled" "$TMP/psettings-put.json" '"demo_enabled":true'
curl -sS -o "$TMP/psettings-del.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/plugins/demo-plugin/settings" \
  -H 'Content-Type: application/json' -d '{"demo_label":null}' > "$TMP/psettings-del.code"
check "PUT settings null deletes the override" 200 "$(cat "$TMP/psettings-del.code")"
check_contains "settings demo_label falls back to default" "$TMP/psettings-del.json" '"demo_label":"Demo"'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/plugins/demo-plugin/settings" \
  -H 'Content-Type: application/json' -d '[1,2]' > "$TMP/psettings-bad.code"
check "PUT settings non-object" 400 "$(cat "$TMP/psettings-bad.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/plugins/nope/settings" > "$TMP/psettings-404.code"
check "GET settings unknown plugin" 404 "$(cat "$TMP/psettings-404.code")"
curl -sS -o /dev/null -w "%{http_code}" "$BASE/api/plugins/demo-plugin/settings" > "$TMP/psettings-noauth.code"
check "GET settings unauthenticated" 401 "$(cat "$TMP/psettings-noauth.code")"

# --- logout ---
curl -sS -o "$TMP/logout.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/auth/logout" > "$TMP/logout.code"
check "POST /api/auth/logout" 200 "$(cat "$TMP/logout.code")"

echo
echo "== P4 smoke: $PASS passed, $FAIL failed =="
rm -rf "$TMP"
if [ "${1:-}" = "--keep" ]; then
  echo "Leaving wrangler dev running (PID $DEV_PID) on :$PORT"
  trap - EXIT
else
  cleanup
fi
[ "$FAIL" -eq 0 ]
