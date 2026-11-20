#!/usr/bin/env bash
# WORDLESS v2 P3 smoke test — runs the Rust worker under local wrangler dev
# against a local D1 (migrations applied, one seeded admin) and checks the
# P2 route surface end-to-end:
#   auth: guarded login (bad 401 / good 200 + cookie), me, sessions list +
#     revoke-all, logout, lockout (5 failed in 15 min -> 429, LAST)
#   posts: create/list/get-by-slug/update/revisions/autosave/trash/restore
#   terms/comments/menus/search/permalinks/options/settings/activity
#   media: multipart upload, serve by key, derivatives list, delete
#   files: list   themes: list/active/theme.css
#   seo: per-post seo get/put/analysis, robots get/put, templates,
#     calendar, import, redirects CRUD, 404 log
#   public: home, single post, feed, sitemap, robots.txt, 404 page
#   admin: /admin/ serves the real v1 SPA build (copied, not modified)
# P3 additions (this script): conversion upload + fixture, AI task ->
#   approval -> exactly one draft, integrations, marketplace install +
#   compat report, commerce product/cart/mock checkout/webhook
#   (paid once, replay duplicate, bad signature 401), security
#   overview, WXR + SQL import (v1 counts), GraphQL (same post as
#   REST, guards, operationName), outbound webhooks (subscribe, fire
#   on post create, retry, delete), public shop flow, CPT archive,
#   redirect-update purge, backup create/verify/download/restore.
# The mock payment secret is injected via --var (local dev only).
#
# Usage: ./scripts/smoke-p2.sh [--keep]   (--keep leaves wrangler dev running)
# Clean room first: rm -rf .wrangler   (the script applies migrations itself)
# Requires: wrangler (TS repo's project-managed binary), worker-build, node.
# The admin seed password is fixed for local dev only:
#   login "admin", password "smoke-admin-2026"
# The local D1 lives in this workspace's .wrangler/ (never the TS repo's).
# NOT covered live here (unit-tested in their crates): TOTP setup/confirm,
# password-reset email (no mail integration in P2), user creation.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"

WRANGLER="$HOME/workspace/wordless/wordless-cms/node_modules/.bin/wrangler"
PORT="${SMOKE_PORT:-8798}"
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

# jget <file> <js-expression-on-d> — extracts a value from an envelope JSON.
jget() {
  node -e 'const fs=require("fs");const d=JSON.parse(fs.readFileSync(process.argv[1],"utf8"));let v="";try{v=eval(process.argv[2]);}catch(e){v="";}console.log(v===undefined||v===null?"":v);' "$1" "$2"
}

echo "== P3 smoke: applying migrations (local D1) =="
"$WRANGLER" d1 migrations apply DB --local >/dev/null

echo "== P3 smoke: seeding admin (login admin / smoke-admin-2026) =="
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

echo "== P3 smoke: starting wrangler dev on :$PORT =="
# Split (2026-11-20): conversion runs in the converter Worker — start
# it first, and point the main Worker at it.
source "$ROOT/scripts/converter-dev.sh"
start_converter
source "$ROOT/scripts/split-workers-dev.sh"
# Size remedy (same day): the shop checkout runs in the
# Woo Worker — start it too, with the same mock webhook
# secret (the Woo Worker resolves the mock provider and
# creates the order itself).
start_woo --var MOCK_PAYMENT_WEBHOOK_SECRET:smoke-mock-secret
start_seo
"$WRANGLER" dev --port "$PORT" --local --var MOCK_PAYMENT_WEBHOOK_SECRET:smoke-mock-secret --var "WORDLESS_CONVERTER_URL:$CONVERTER_URL" --var "WORDLESS_WOO_URL:$WOO_URL" --var "WORDLESS_SEO_URL:$SEO_URL" > /tmp/wordless-p3-wrangler.log 2>&1 &
DEV_PID=$!
cleanup() { kill "$DEV_PID" 2>/dev/null || true; stop_converter; stop_woo; stop_seo; }
trap cleanup EXIT

# Wait for the worker to answer (wrangler dev builds first — can take a while).
for i in $(seq 1 240); do
  if curl -fsS -o /dev/null "$BASE/api/health" 2>/dev/null; then break; fi
  sleep 2
done

echo "== P3 smoke: checks =="
TMP="$(mktemp -d)"
AUTH=(-H "Content-Type: application/json")

login() { # login <password> — sets $COOKIE on success
  curl -sS -o "$TMP/login.json" -w "%{http_code}" -D "$TMP/login.headers" -X POST "$BASE/api/auth/login" \
    -H 'Content-Type: application/json' -d "{\"login\":\"admin\",\"password\":\"$1\"}" > "$TMP/login.code"
  COOKIE=$(grep -i '^set-cookie:' "$TMP/login.headers" | head -1 | sed 's/^[^:]*: *//;s/;.*//' | tr -d '\r' || true)
}

# --- health ---
curl -sS -o "$TMP/health.json" -w "%{http_code}" "$BASE/api/health" > "$TMP/health.code"
check "GET /api/health status" 200 "$(cat "$TMP/health.code")"
check_contains "health envelope ok=true" "$TMP/health.json" '"ok":true'
check_contains "health version 2.0.0-dev" "$TMP/health.json" '"version":"2.0.0-dev"'

curl -sS -o "$TMP/site.json" -w "%{http_code}" "$BASE/api/site-health" > "$TMP/site.code"
check "GET /api/site-health status" 200 "$(cat "$TMP/site.code")"
check_contains "site-health ok" "$TMP/site.json" '"status":"ok"'
check_contains "site-health database pass" "$TMP/site.json" '"name":"database","status":"pass"'
check_contains "site-health tables pass" "$TMP/site.json" '"name":"tables","status":"pass"'
check_contains "site-health content counts 1 user" "$TMP/site.json" '1 users'

# --- auth: bad password -> 401, generic message ---
login wrong
check "POST /api/auth/login wrong password" 401 "$(cat "$TMP/login.code")"
check_contains "login failure is generic" "$TMP/login.json" 'Invalid login or password.'

# --- auth: good password -> 200 + Set-Cookie ---
login smoke-admin-2026
check "POST /api/auth/login good password" 200 "$(cat "$TMP/login.code")"
check_contains "login returns admin user" "$TMP/login.json" '"login":"admin"'
check_contains "login sets session cookie" "$TMP/login.headers" 'wordless_session='

# --- auth: /me with cookie -> 200; without -> 401 ---
curl -sS -o "$TMP/me.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/auth/me" > "$TMP/me.code"
check "GET /api/auth/me with cookie" 200 "$(cat "$TMP/me.code")"
check_contains "me returns admin" "$TMP/me.json" '"role":"administrator"'
curl -sS -o /dev/null -w "%{http_code}" "$BASE/api/auth/me" > "$TMP/me2.code"
check "GET /api/auth/me without cookie" 401 "$(cat "$TMP/me2.code")"

# --- auth: sessions list, then revoke-all (re-login after) ---
curl -sS -o "$TMP/sess.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/auth/sessions" > "$TMP/sess.code"
check "GET /api/auth/sessions" 200 "$(cat "$TMP/sess.code")"
check_contains "sessions list is an array" "$TMP/sess.json" '"ok":true'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/auth/sessions/revoke-all" > "$TMP/revoke.code"
check "POST /api/auth/sessions/revoke-all" 200 "$(cat "$TMP/revoke.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/auth/me" > "$TMP/me-revoked.code"
check "GET /api/auth/me after revoke-all" 401 "$(cat "$TMP/me-revoked.code")"
login smoke-admin-2026
check "re-login after revoke-all" 200 "$(cat "$TMP/login.code")"

# --- settings ---
curl -sS -o "$TMP/settings.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/settings" > "$TMP/settings.code"
check "GET /api/settings" 200 "$(cat "$TMP/settings.code")"
check_contains "settings has site title" "$TMP/settings.json" '"siteTitle"'
curl -sS -o "$TMP/settings2.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X PUT "$BASE/api/settings" \
  -H 'Content-Type: application/json' -d '{"siteTitle":"Smoke Site"}' > "$TMP/settings2.code"
check "PUT /api/settings" 200 "$(cat "$TMP/settings2.code")"
check_contains "settings update applied" "$TMP/settings2.json" 'Smoke Site'
curl -sS -o "$TMP/opt.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/options/blogname" > "$TMP/opt.code"
check "GET /api/options/blogname" 200 "$(cat "$TMP/opt.code")"
check_contains "option blogname is Smoke Site" "$TMP/opt.json" 'Smoke Site'

# --- permalinks ---
curl -sS -o "$TMP/perma.json" -w "%{http_code}" "$BASE/api/permalinks" > "$TMP/perma.code"
check "GET /api/permalinks" 200 "$(cat "$TMP/perma.code")"
check_contains "permalinks structure present" "$TMP/perma.json" '"structure"'
curl -sS -o "$TMP/perma2.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X PUT "$BASE/api/permalinks" \
  -H 'Content-Type: application/json' -d '{"structure":"postname"}' > "$TMP/perma2.code"
check "PUT /api/permalinks" 200 "$(cat "$TMP/perma2.code")"
check_contains "permalinks set to postname" "$TMP/perma2.json" '"structure":"postname"'

# --- posts: create ---
curl -sS -o "$TMP/post.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/posts" \
  -H 'Content-Type: application/json' \
  -d '{"title":"Smoke Post","content":"<!-- wp:paragraph --><p>Hello smoke world. This is a test post with enough words to analyse for the SEO checks below.</p><!-- /wp:paragraph -->","status":"publish","slug":"smoke-post"}' > "$TMP/post.code"
check "POST /api/posts" 201 "$(cat "$TMP/post.code")"
check_contains "post created with slug" "$TMP/post.json" '"slug":"smoke-post"'
POST_ID="$(jget "$TMP/post.json" 'd.data.id')"
echo "  (post id: $POST_ID)"

# --- posts: list / get by slug / update ---
curl -sS -o "$TMP/posts.json" -w "%{http_code}" "$BASE/api/posts" > "$TMP/posts.code"
check "GET /api/posts" 200 "$(cat "$TMP/posts.code")"
check_contains "post list contains smoke post" "$TMP/posts.json" 'smoke-post'
curl -sS -o "$TMP/post-slug.json" -w "%{http_code}" "$BASE/api/posts/smoke-post" > "$TMP/post-slug.code"
check "GET /api/posts/smoke-post" 200 "$(cat "$TMP/post-slug.code")"
check_contains "post by slug has seo field" "$TMP/post-slug.json" '"seo"'
check_contains "post by slug has elementor flag" "$TMP/post-slug.json" '"hasElementorData":false'
curl -sS -o "$TMP/post-upd.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X PUT "$BASE/api/posts/$POST_ID" \
  -H 'Content-Type: application/json' -d '{"title":"Smoke Post Updated"}' > "$TMP/post-upd.code"
check "PUT /api/posts/:id" 200 "$(cat "$TMP/post-upd.code")"
check_contains "post title updated" "$TMP/post-upd.json" 'Smoke Post Updated'

# --- revisions + autosave ---
curl -sS -o "$TMP/revs.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/posts/$POST_ID/revisions" > "$TMP/revs.code"
check "GET /api/posts/:id/revisions" 200 "$(cat "$TMP/revs.code")"
curl -sS -o "$TMP/autosave.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/posts/$POST_ID/autosave" \
  -H 'Content-Type: application/json' -d '{"content":"<!-- wp:paragraph --><p>Autosaved content.</p><!-- /wp:paragraph -->"}' > "$TMP/autosave.code"
check "POST /api/posts/:id/autosave" 200 "$(cat "$TMP/autosave.code")"

# --- terms ---
curl -sS -o "$TMP/term.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/terms" \
  -H 'Content-Type: application/json' -d '{"name":"Smoke Cat","slug":"smoke-cat","taxonomy":"category"}' > "$TMP/term.code"
check "POST /api/terms" 201 "$(cat "$TMP/term.code")"
TERM_ID="$(jget "$TMP/term.json" 'd.data.termTaxonomyId')"
echo "  (term taxonomy id: $TERM_ID)"
curl -sS -o "$TMP/terms.json" -w "%{http_code}" "$BASE/api/terms?taxonomy=category" > "$TMP/terms.code"
check "GET /api/terms" 200 "$(cat "$TMP/terms.code")"
check_contains "term list contains smoke cat" "$TMP/terms.json" 'smoke-cat'
curl -sS -o "$TMP/post-terms.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/posts/$POST_ID/terms" \
  -H 'Content-Type: application/json' -d "{\"taxonomy\":\"category\",\"termIds\":[$TERM_ID]}" > "$TMP/post-terms.code"
check "POST /api/posts/:id/terms" 200 "$(cat "$TMP/post-terms.code")"
check_contains "term assigned to post" "$TMP/post-terms.json" 'smoke-cat'

# --- comments ---
curl -sS -o "$TMP/comment.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/posts/smoke-post/comments" \
  -H 'Content-Type: application/json' \
  -d '{"author":"Smoke Tester","author_email":"tester@example.com","content":"A smoke-test comment."}' > "$TMP/comment.code"
check "POST /api/posts/:slug/comments" 201 "$(cat "$TMP/comment.code")"
COMMENT_ID="$(jget "$TMP/comment.json" 'd.data.id')"
curl -sS -o "$TMP/comments.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/comments" > "$TMP/comments.code"
check "GET /api/comments" 200 "$(cat "$TMP/comments.code")"
check_contains "comment list contains smoke comment" "$TMP/comments.json" 'Smoke Tester'
curl -sS -o "$TMP/thread.json" -w "%{http_code}" "$BASE/api/posts/smoke-post/comments" > "$TMP/thread.code"
check "GET /api/posts/:slug/comments (public thread)" 200 "$(cat "$TMP/thread.code")"
check_contains "public thread contains comment" "$TMP/thread.json" 'Smoke Tester'

# --- menus ---
curl -sS -o "$TMP/menu.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/menus" \
  -H 'Content-Type: application/json' -d '{"name":"Smoke Menu"}' > "$TMP/menu.code"
check "POST /api/menus" 201 "$(cat "$TMP/menu.code")"
MENU_ID="$(jget "$TMP/menu.json" 'd.data.id')"
curl -sS -o "$TMP/menu-item.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/menus/$MENU_ID/items" \
  -H 'Content-Type: application/json' -d '{"title":"Home","type":"custom","url":"/"}' > "$TMP/menu-item.code"
check "POST /api/menus/:id/items" 201 "$(cat "$TMP/menu-item.code")"
curl -sS -o "$TMP/menu-loc.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/menus/locations" \
  -H 'Content-Type: application/json' -d "{\"location\":\"primary\",\"menuId\":$MENU_ID}" > "$TMP/menu-loc.code"
check "POST /api/menus/locations" 200 "$(cat "$TMP/menu-loc.code")"
curl -sS -o "$TMP/menu-by-loc.json" -w "%{http_code}" "$BASE/api/menus/location/primary" > "$TMP/menu-by-loc.code"
check "GET /api/menus/location/primary" 200 "$(cat "$TMP/menu-by-loc.code")"
check_contains "menu by location resolves" "$TMP/menu-by-loc.json" 'Smoke Menu'

# --- search ---
curl -sS -o "$TMP/search.json" -w "%{http_code}" "$BASE/api/search?q=Smoke" > "$TMP/search.code"
check "GET /api/search?q=Smoke" 200 "$(cat "$TMP/search.code")"
check_contains "search finds smoke post" "$TMP/search.json" 'smoke-post'

# --- activity ---
curl -sS -o "$TMP/activity.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/activity" > "$TMP/activity.code"
check "GET /api/activity" 200 "$(cat "$TMP/activity.code")"
check_contains "activity logged post.create" "$TMP/activity.json" 'post.create'

# --- users ---
curl -sS -o "$TMP/users.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/users" > "$TMP/users.code"
check "GET /api/users" 200 "$(cat "$TMP/users.code")"
check_contains "users list contains admin" "$TMP/users.json" '"login":"admin"'

# --- media: list, upload, serve, derivatives, delete ---
curl -sS -o "$TMP/media.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/media" > "$TMP/media.code"
check "GET /api/media" 200 "$(cat "$TMP/media.code")"
printf 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8Xw8AAwMCAO+ip1sAAAAASUVORK5CYII=' | base64 -d > "$TMP/smoke.png"
curl -sS -o "$TMP/upload.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/media" \
  -F "file=@$TMP/smoke.png;filename=smoke.png;type=image/png" -F "title=Smoke Image" > "$TMP/upload.code"
check "POST /api/media (multipart PNG)" 201 "$(cat "$TMP/upload.code")"
MEDIA_ID="$(jget "$TMP/upload.json" 'd.data.id')"
MEDIA_KEY="$(jget "$TMP/upload.json" 'd.data.objectKey')"
echo "  (media id: $MEDIA_ID key: $MEDIA_KEY)"
curl -sS -o "$TMP/served.png" -w "%{http_code}" -D "$TMP/served.headers" "$BASE/media/$MEDIA_KEY" > "$TMP/served.code"
check "GET /media/:key" 200 "$(cat "$TMP/served.code")"
check_contains "served media is PNG" "$TMP/served.headers" 'image/png'
curl -sS -o "$TMP/derivs.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/media/derivatives" > "$TMP/derivs.code"
check "GET /api/media/derivatives" 200 "$(cat "$TMP/derivs.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $COOKIE" -X DELETE "$BASE/api/media/$MEDIA_ID" > "$TMP/media-del.code"
check "DELETE /api/media/:id" 200 "$(cat "$TMP/media-del.code")"

# --- files ---
curl -sS -o "$TMP/files.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/files" > "$TMP/files.code"
check "GET /api/files" 200 "$(cat "$TMP/files.code")"

# --- themes ---
curl -sS -o "$TMP/themes.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/themes" > "$TMP/themes.code"
check "GET /api/themes" 200 "$(cat "$TMP/themes.code")"
check_contains "themes list contains default" "$TMP/themes.json" 'wordless-default'
curl -sS -o "$TMP/active-theme.json" -w "%{http_code}" "$BASE/api/themes/active" > "$TMP/active-theme.code"
check "GET /api/themes/active" 200 "$(cat "$TMP/active-theme.code")"
curl -sS -o "$TMP/theme.css" -w "%{http_code}" -D "$TMP/theme-css.headers" "$BASE/theme.css" > "$TMP/theme-css.code"
check "GET /theme.css" 200 "$(cat "$TMP/theme-css.code")"
check_contains "theme.css is CSS" "$TMP/theme-css.headers" 'text/css'

# --- seo: per-post seo ---
curl -sS -o "$TMP/seo.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/posts/$POST_ID/seo" > "$TMP/seo.code"
check "GET /api/posts/:id/seo" 200 "$(cat "$TMP/seo.code")"
curl -sS -o "$TMP/seo2.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X PUT "$BASE/api/posts/$POST_ID/seo" \
  -H 'Content-Type: application/json' -d '{"title":"Smoke SEO Title","description":"A smoke-test SEO description."}' > "$TMP/seo2.code"
check "PUT /api/posts/:id/seo" 200 "$(cat "$TMP/seo2.code")"
check_contains "seo title saved" "$TMP/seo2.json" 'Smoke SEO Title'
curl -sS -o "$TMP/analysis.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/posts/$POST_ID/seo/analysis" > "$TMP/analysis.code"
check "GET /api/posts/:id/seo/analysis" 200 "$(cat "$TMP/analysis.code")"
check_contains "seo analysis has score" "$TMP/analysis.json" '"score"'

# --- seo: robots ---
curl -sS -o "$TMP/robots.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/seo/robots" > "$TMP/robots.code"
check "GET /api/seo/robots" 200 "$(cat "$TMP/robots.code")"
check_contains "robots has sitemap line" "$TMP/robots.json" 'Sitemap:'
curl -sS -o "$TMP/robots2.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X PUT "$BASE/api/seo/robots" \
  -H 'Content-Type: application/json' -d '{"robots":"User-agent: *\nDisallow: /private/\n"}' > "$TMP/robots2.code"
check "PUT /api/seo/robots" 200 "$(cat "$TMP/robots2.code")"
check_contains "robots update applied" "$TMP/robots2.json" '/private/'
# NOTE: empty "" is rejected (400) in v1 too — requireString demands a
# non-empty string. Reset by PUT-ing the default text; the served text
# gets the Sitemap line appended automatically.
curl -sS -o "$TMP/robots3.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X PUT "$BASE/api/seo/robots" \
  -H 'Content-Type: application/json' -d '{"robots":"User-agent: *\nAllow: /\n"}' > "$TMP/robots3.code"
check "PUT /api/seo/robots (reset to default)" 200 "$(cat "$TMP/robots3.code")"
check_contains "robots reset has sitemap line" "$TMP/robots3.json" 'Sitemap:'

# --- seo: templates + calendar + import ---
curl -sS -o "$TMP/templates.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/seo/templates" > "$TMP/templates.code"
check "GET /api/seo/templates" 200 "$(cat "$TMP/templates.code")"
check_contains "templates include announcement" "$TMP/templates.json" 'announcement'
curl -sS -o "$TMP/tpl-post.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/seo/templates/announcement/create" \
  -H 'Content-Type: application/json' -d '{"title":"Smoke Announcement"}' > "$TMP/tpl-post.code"
check "POST /api/seo/templates/:slug/create" 201 "$(cat "$TMP/tpl-post.code")"
curl -sS -o "$TMP/cal.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/seo/calendar" > "$TMP/cal.code"
check "GET /api/seo/calendar" 200 "$(cat "$TMP/cal.code")"
curl -sS -o "$TMP/import.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/seo/import" \
  -H 'Content-Type: application/json' -d "{\"posts\":[{\"postId\":$POST_ID,\"meta\":{\"_wl_seo_focus_keyword\":\"smoke\"}}]}" > "$TMP/import.code"
check "POST /api/seo/import" 200 "$(cat "$TMP/import.code")"
check_contains "seo import applied 1" "$TMP/import.json" '"applied":1'

# --- seo: redirects ---
curl -sS -o "$TMP/redir.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/seo/redirects" \
  -H 'Content-Type: application/json' -d '{"oldPath":"/old-smoke","newPath":"/smoke-post"}' > "$TMP/redir.code"
check "POST /api/seo/redirects" 201 "$(cat "$TMP/redir.code")"
REDIR_ID="$(jget "$TMP/redir.json" 'd.data.id')"
curl -sS -o "$TMP/redirs.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/seo/redirects" > "$TMP/redirs.code"
check "GET /api/seo/redirects" 200 "$(cat "$TMP/redirs.code")"
check_contains "redirect list contains old path" "$TMP/redirs.json" '/old-smoke'
curl -sS -o /dev/null -w "%{http_code}" -D "$TMP/redir-follow.headers" "$BASE/old-smoke" > "$TMP/redir-follow.code"
check "GET /old-smoke redirects 301" 301 "$(cat "$TMP/redir-follow.code")"
check_contains "redirect target is smoke post" "$TMP/redir-follow.headers" '/smoke-post'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $COOKIE" -X DELETE "$BASE/api/seo/redirects/$REDIR_ID" > "$TMP/redir-del.code"
check "DELETE /api/seo/redirects/:id" 200 "$(cat "$TMP/redir-del.code")"

# --- seo: 404 log ---
curl -sS -o "$TMP/404s.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/seo/404s" > "$TMP/404s.code"
check "GET /api/seo/404s" 200 "$(cat "$TMP/404s.code")"

# --- public rendering ---
curl -sS -o "$TMP/home.html" -w "%{http_code}" "$BASE/" > "$TMP/home.code"
check "GET / (home)" 200 "$(cat "$TMP/home.code")"
check_contains "home lists smoke post" "$TMP/home.html" 'Smoke Post Updated'
curl -sS -o "$TMP/single.html" -w "%{http_code}" "$BASE/smoke-post" > "$TMP/single.code"
check "GET /smoke-post (public single)" 200 "$(cat "$TMP/single.code")"
check_contains "single page has content" "$TMP/single.html" 'Hello smoke world'
check_contains "single page has seo title" "$TMP/single.html" 'Smoke SEO Title'
curl -sS -o "$TMP/feed.xml" -w "%{http_code}" -D "$TMP/feed.headers" "$BASE/feed" > "$TMP/feed.code"
check "GET /feed" 200 "$(cat "$TMP/feed.code")"
check_contains "feed is rss" "$TMP/feed.headers" 'application/rss+xml'
check_contains "feed contains smoke post" "$TMP/feed.xml" 'smoke-post'
curl -sS -o "$TMP/sitemap.xml" -w "%{http_code}" -D "$TMP/sitemap.headers" "$BASE/sitemap.xml" > "$TMP/sitemap.code"
check "GET /sitemap.xml" 200 "$(cat "$TMP/sitemap.code")"
check_contains "sitemap is xml" "$TMP/sitemap.headers" 'application/xml'
check_contains "sitemap contains smoke post" "$TMP/sitemap.xml" 'smoke-post'
curl -sS -o "$TMP/robots.txt" -w "%{http_code}" "$BASE/robots.txt" > "$TMP/robots-txt.code"
check "GET /robots.txt" 200 "$(cat "$TMP/robots-txt.code")"
check_contains "robots.txt has sitemap" "$TMP/robots.txt" 'Sitemap:'
curl -sS -o "$TMP/nopage.html" -w "%{http_code}" "$BASE/no-such-page-xyz" > "$TMP/nopage.code"
check "GET /no-such-page-xyz (404 page)" 404 "$(cat "$TMP/nopage.code")"
curl -sS -o "$TMP/404s2.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/seo/404s" > "$TMP/404s2.code"
check "GET /api/seo/404s after 404" 200 "$(cat "$TMP/404s2.code")"
check_contains "404 log recorded the path" "$TMP/404s2.json" '/no-such-page-xyz'

# --- public search page ---
curl -sS -o "$TMP/pub-search.html" -w "%{http_code}" "$BASE/search?q=Smoke" > "$TMP/pub-search.code"
check "GET /search?q=Smoke (public)" 200 "$(cat "$TMP/pub-search.code")"
check_contains "public search finds post" "$TMP/pub-search.html" 'Smoke Post'

# --- admin SPA (real v1 build) ---
curl -sS -o "$TMP/admin.html" -w "%{http_code}" "$BASE/admin/" > "$TMP/admin.code"
check "GET /admin/ (SPA)" 200 "$(cat "$TMP/admin.code")"
check_contains "admin serves real SPA" "$TMP/admin.html" 'Wordless Admin'

# --- unknown route + wrong method ---
curl -sS -o /dev/null -w "%{http_code}" "$BASE/api/nope" > "$TMP/n.code"
check "GET /api/nope 404" 404 "$(cat "$TMP/n.code")"
curl -sS -o /dev/null -w "%{http_code}" -X POST "$BASE/api/health" > "$TMP/m.code"
check "POST /api/health 405" 405 "$(cat "$TMP/m.code")"

# --- posts: trash + restore (leave the post published) ---
curl -sS -o "$TMP/trash.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/posts/$POST_ID/trash" > "$TMP/trash.code"
check "POST /api/posts/:id/trash" 200 "$(cat "$TMP/trash.code")"
check_contains "trashed post status" "$TMP/trash.json" '"status":"trash"'
curl -sS -o "$TMP/restore.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/posts/$POST_ID/restore" > "$TMP/restore.code"
check "POST /api/posts/:id/restore" 200 "$(cat "$TMP/restore.code")"
check_contains "restored post status is draft (v1 behaviour)" "$TMP/restore.json" '"status":"draft"'


# ============================================================ P3 checks ==
# (Inserted before logout: everything here needs the admin cookie.
#  Lockout stays LAST in the P2 tail.)

# --- conversion: multipart upload of the real hello.php ---
HELLO_PHP="$HOME/workspace/wordless/reference/wordpress-7.1.5/wp-content/plugins/hello.php"
curl -sS -o "$TMP/conv.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/conversion/jobs" \
  -F "file=@$HELLO_PHP;filename=hello.php;type=text/plain" > "$TMP/conv.code"
check "POST /api/conversion/jobs (upload hello.php)" 201 "$(cat "$TMP/conv.code")"
check_contains "conversion job finished done" "$TMP/conv.json" '"status":"done"'
check_contains "hello dolly converts get_lyric" "$TMP/conv.json" '"hello_dolly_get_lyric"'
check_contains "hello dolly converts the plugin function" "$TMP/conv.json" '"hello_dolly"'
check_contains "hello dolly converts 0 unconverted" "$TMP/conv.json" '"unconverted":[]'
CONV_ID="$(jget "$TMP/conv.json" 'd.data.id')"
curl -sS -o "$TMP/conv-get.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/conversion/jobs/$CONV_ID" > "$TMP/conv-get.code"
check "GET /api/conversion/jobs/:id" 200 "$(cat "$TMP/conv-get.code")"
curl -sS -o "$TMP/conv-list.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/conversion/jobs" > "$TMP/conv-list.code"
check "GET /api/conversion/jobs" 200 "$(cat "$TMP/conv-list.code")"
check_contains "conversion list contains hello job" "$TMP/conv-list.json" 'hello.php'

# --- conversion: fixture path (upstream fetch; may 502 from this VM) ---
curl -sS -o "$TMP/conv-fix.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/conversion/jobs" \
  -H 'Content-Type: application/json' -d '{"fixture":"hello-dolly"}' > "$TMP/conv-fix.code"
if [ "$(cat "$TMP/conv-fix.code")" = "201" ]; then
  check "POST /api/conversion/jobs (fixture hello-dolly)" 201 "201"
  check_contains "fixture job finished done" "$TMP/conv-fix.json" '"status":"done"'
else
  echo "  (note: fixture fetch returned $(cat "$TMP/conv-fix.code") — raw.githubusercontent.com is unreachable from this VM; the upload path above covers the converter)"
fi
curl -sS -o "$TMP/ext.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/extensions" > "$TMP/ext.code"
check "GET /api/extensions" 200 "$(cat "$TMP/ext.code")"

# --- ai: providers / mode / agents ---
curl -sS -o "$TMP/ai-prov.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/ai/providers" > "$TMP/ai-prov.code"
check "GET /api/ai/providers" 200 "$(cat "$TMP/ai-prov.code")"
check_contains "ai providers list mock" "$TMP/ai-prov.json" '"mock"'
check_contains "ai default provider is workers-ai" "$TMP/ai-prov.json" '"selected":"workers-ai"'
curl -sS -o "$TMP/ai-sel.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/ai/providers/select" \
  -H 'Content-Type: application/json' -d '{"provider":"mock"}' > "$TMP/ai-sel.code"
check "POST /api/ai/providers/select mock" 200 "$(cat "$TMP/ai-sel.code")"
check_contains "ai provider now mock" "$TMP/ai-sel.json" '"selected":"mock"'
curl -sS -o "$TMP/ai-agents.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/ai/agents" > "$TMP/ai-agents.code"
check "GET /api/ai/agents" 200 "$(cat "$TMP/ai-agents.code")"
AGENT_COUNT="$(jget "$TMP/ai-agents.json" 'd.data.length')"
check "ai agents count is 15" 15 "$AGENT_COUNT"
curl -sS -o "$TMP/ai-mode.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/ai/mode" > "$TMP/ai-mode.code"
check "GET /api/ai/mode" 200 "$(cat "$TMP/ai-mode.code")"
check_contains "ai mode default supervised" "$TMP/ai-mode.json" '"mode":"supervised"'

# --- ai: task -> approval -> exactly one draft post ---
curl -sS -o "$TMP/drafts-before.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/posts?status=draft" > "$TMP/drafts-before.code"
DRAFTS_BEFORE="$(jget "$TMP/drafts-before.json" 'd.data.total')"
curl -sS -o "$TMP/ai-task.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/ai/tasks" \
  -H 'Content-Type: application/json' \
  -d '{"agent":"content-writer","input":"Write a short post about smoke testing."}' > "$TMP/ai-task.code"
check "POST /api/ai/tasks (mock content-writer)" 201 "$(cat "$TMP/ai-task.code")"
check_contains "ai task finished done" "$TMP/ai-task.json" '"status":"done"'
TASK_ID="$(jget "$TMP/ai-task.json" 'd.data.id')"
curl -sS -o "$TMP/ai-task-get.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/ai/tasks/$TASK_ID" > "$TMP/ai-task-get.code"
check "GET /api/ai/tasks/:id" 200 "$(cat "$TMP/ai-task-get.code")"
curl -sS -o "$TMP/ai-appr.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/ai/approvals?status=pending" > "$TMP/ai-appr.code"
check "GET /api/ai/approvals?status=pending" 200 "$(cat "$TMP/ai-appr.code")"
APPROVAL_ID="$(jget "$TMP/ai-appr.json" 'd.data.approvals[0].id')"
curl -sS -o "$TMP/ai-ok.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/ai/approvals/$APPROVAL_ID/approve" > "$TMP/ai-ok.code"
check "POST /api/ai/approvals/:id/approve" 200 "$(cat "$TMP/ai-ok.code")"
check_contains "approval marked approved" "$TMP/ai-ok.json" '"status":"approved"'
curl -sS -o "$TMP/drafts-after.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/posts?status=draft" > "$TMP/drafts-after.code"
DRAFTS_AFTER="$(jget "$TMP/drafts-after.json" 'd.data.total')"
check "approval created exactly one draft" "$((DRAFTS_BEFORE + 1))" "$DRAFTS_AFTER"

# --- ai: memory notes + rebuild ---
curl -sS -o "$TMP/ai-mem.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/ai/memory" > "$TMP/ai-mem.code"
check "GET /api/ai/memory" 200 "$(cat "$TMP/ai-mem.code")"
curl -sS -o "$TMP/ai-note.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/ai/memory/notes" \
  -H 'Content-Type: application/json' -d '{"text":"Smoke note: the worker is alive."}' > "$TMP/ai-note.code"
check "POST /api/ai/memory/notes" 201 "$(cat "$TMP/ai-note.code")"
NOTE_ID="$(jget "$TMP/ai-note.json" 'd.data.id')"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $COOKIE" -X DELETE "$BASE/api/ai/memory/notes/$NOTE_ID" > "$TMP/ai-note-del.code"
check "DELETE /api/ai/memory/notes/:id" 200 "$(cat "$TMP/ai-note-del.code")"
curl -sS -o "$TMP/ai-rebuild.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/ai/memory/rebuild" > "$TMP/ai-rebuild.code"
check "POST /api/ai/memory/rebuild" 200 "$(cat "$TMP/ai-rebuild.code")"

# --- integrations ---
curl -sS -o "$TMP/integ.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/integrations" > "$TMP/integ.code"
check "GET /api/integrations" 200 "$(cat "$TMP/integ.code")"
check_contains "integrations list has email" "$TMP/integ.json" '"email"'
curl -sS -o "$TMP/mail-log.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/integrations/email/log" > "$TMP/mail-log.code"
check "GET /api/integrations/email/log" 200 "$(cat "$TMP/mail-log.code")"
curl -sS -o "$TMP/analytics.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/integrations/analytics/status" > "$TMP/analytics.code"
check "GET /api/integrations/analytics/status" 200 "$(cat "$TMP/analytics.code")"
curl -sS -o "$TMP/cf.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/integrations/cloudflare" > "$TMP/cf.code"
check "GET /api/integrations/cloudflare" 200 "$(cat "$TMP/cf.code")"
check_contains "cloudflare status lists D1" "$TMP/cf.json" 'DB (D1)'

# --- marketplace: build a hello-dolly ZIP and install it ---
python3 - "$TMP/hello-dolly.zip" "$HELLO_PHP" <<'PYEOF'
import json, sys, zipfile
out, hello = sys.argv[1], sys.argv[2]
manifest = {"slug": "hello-dolly", "name": "Hello Dolly", "version": "1.7.2",
            "description": "A plugin that prints a random lyric."}
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
    z.writestr("hello-dolly/wordless.json", json.dumps(manifest))
    z.writestr("hello-dolly/hello.php", open(hello, encoding="utf-8").read())
PYEOF
curl -sS -o "$TMP/mkt-install.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/marketplace/install" \
  -F "file=@$TMP/hello-dolly.zip;filename=hello-dolly.zip;type=application/zip" > "$TMP/mkt-install.code"
check "POST /api/marketplace/install" 201 "$(cat "$TMP/mkt-install.code")"
check_contains "marketplace installed hello-dolly" "$TMP/mkt-install.json" '"slug":"hello-dolly"'
curl -sS -o "$TMP/mkt.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/marketplace" > "$TMP/mkt.code"
check "GET /api/marketplace" 200 "$(cat "$TMP/mkt.code")"
check_contains "marketplace lists hello-dolly" "$TMP/mkt.json" 'hello-dolly'
curl -sS -o "$TMP/mkt-report.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/marketplace/hello-dolly/report" > "$TMP/mkt-report.code"
check "GET /api/marketplace/:slug/report" 200 "$(cat "$TMP/mkt-report.code")"
check_contains "compat report verdict compatible" "$TMP/mkt-report.json" '"verdict":"compatible"'
check_contains "compat report 1 php file" "$TMP/mkt-report.json" '"phpFiles":1'
check_contains "compat report 1 converted" "$TMP/mkt-report.json" '"converted":1'
check_contains "compat report carries conversion record" "$TMP/mkt-report.json" '"conversion":{'
curl -sS -o "$TMP/mkt-act.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/marketplace/hello-dolly/activate" > "$TMP/mkt-act.code"
check "POST /api/marketplace/:slug/activate" 200 "$(cat "$TMP/mkt-act.code")"
check_contains "extension activated" "$TMP/mkt-act.json" '"status":"active"'
curl -sS -o "$TMP/mkt-deact.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/marketplace/hello-dolly/deactivate" > "$TMP/mkt-deact.code"
check "POST /api/marketplace/:slug/deactivate" 200 "$(cat "$TMP/mkt-deact.code")"
check_contains "extension deactivated" "$TMP/mkt-deact.json" '"status":"inactive"'

# --- commerce: provider select + product + cart + checkout (mock) ---
curl -sS -o "$TMP/pay-sel.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/commerce/payments/select" \
  -H 'Content-Type: application/json' -d '{"provider":"mock","mode":"test"}' > "$TMP/pay-sel.code"
check "POST /api/commerce/payments/select mock" 200 "$(cat "$TMP/pay-sel.code")"
check_contains "payment provider is mock" "$TMP/pay-sel.json" '"provider":"mock"'
curl -sS -o "$TMP/pay-prov.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/commerce/payments/providers" > "$TMP/pay-prov.code"
check "GET /api/commerce/payments/providers" 200 "$(cat "$TMP/pay-prov.code")"
check_contains "providers list has mock" "$TMP/pay-prov.json" '"mock"'
curl -sS -o "$TMP/pay-cur.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/commerce/payments/selected" > "$TMP/pay-cur.code"
check "GET /api/commerce/payments/selected" 200 "$(cat "$TMP/pay-cur.code")"
check_contains "selected provider is mock" "$TMP/pay-cur.json" '"provider":"mock"'
curl -sS -o "$TMP/product.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/commerce/products" \
  -H 'Content-Type: application/json' \
  -d '{"name":"Smoke Widget","priceMinor":2500,"currency":"NGN","stock":5,"status":"published"}' > "$TMP/product.code"
check "POST /api/commerce/products" 201 "$(cat "$TMP/product.code")"
check_contains "product slug smoke-widget" "$TMP/product.json" '"slug":"smoke-widget"'
PRODUCT_ID="$(jget "$TMP/product.json" 'd.data.id')"
curl -sS -o "$TMP/products.json" -w "%{http_code}" "$BASE/api/commerce/products" > "$TMP/products.code"
check "GET /api/commerce/products (public)" 200 "$(cat "$TMP/products.code")"
check_contains "product list contains smoke widget" "$TMP/products.json" 'smoke-widget'
curl -sS -o "$TMP/product-get.json" -w "%{http_code}" "$BASE/api/commerce/products/$PRODUCT_ID" > "$TMP/product-get.code"
check "GET /api/commerce/products/:id" 200 "$(cat "$TMP/product-get.code")"
curl -sS -o "$TMP/cart.json" -w "%{http_code}" -D "$TMP/cart.headers" -X POST "$BASE/api/commerce/cart" \
  -H 'Content-Type: application/json' -d "{\"productId\":$PRODUCT_ID,\"quantity\":2}" > "$TMP/cart.code"
check "POST /api/commerce/cart" 200 "$(cat "$TMP/cart.code")"
check_contains "cart total is 5000 minor" "$TMP/cart.json" '"totalMinor":5000'
CART_COOKIE="$(grep -i '^set-cookie:' "$TMP/cart.headers" | head -1 | sed -e 's/^[^:]*: *//' -e 's/;.*//' | tr -d '\r')"
curl -sS -o "$TMP/cart-get.json" -w "%{http_code}" -H "Cookie: $CART_COOKIE" "$BASE/api/commerce/cart" > "$TMP/cart-get.code"
check "GET /api/commerce/cart" 200 "$(cat "$TMP/cart-get.code")"
check_contains "cart persists across requests" "$TMP/cart-get.json" 'smoke-widget'
curl -sS -o "$TMP/checkout.json" -w "%{http_code}" -H "Cookie: $CART_COOKIE" -X POST "$BASE/api/commerce/checkout" \
  -H 'Content-Type: application/json' -d '{"customerName":"Smoke Buyer","customerEmail":"buyer@example.com"}' > "$TMP/checkout.code"
check "POST /api/commerce/checkout (mock)" 201 "$(cat "$TMP/checkout.code")"  # P5 F2: checkout creates the order, so 201
check_contains "checkout url is the mock test page" "$TMP/checkout.json" '/shop/checkout/test'
ORDER_ID="$(jget "$TMP/checkout.json" 'd.data.order.id')"
ORDER_REF="$(jget "$TMP/checkout.json" 'd.data.reference')"
echo "  (order id: $ORDER_ID ref: $ORDER_REF)"
curl -sS -o "$TMP/order.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/commerce/orders/$ORDER_ID" > "$TMP/order.code"
check "GET /api/commerce/orders/:id" 200 "$(cat "$TMP/order.code")"
check_contains "order starts pending" "$TMP/order.json" '"status":"pending"'

# --- commerce: mock webhook — paid exactly once, replay duplicate, bad sig 401 ---
MOCK_SECRET="smoke-mock-secret"
WEBHOOK_BODY="{\"event\":\"payment.paid\",\"reference\":\"$ORDER_REF\"}"
WEBHOOK_SIG="$(node -e 'const c=require("node:crypto");console.log(c.createHmac("sha256",process.argv[1]).update(process.argv[2]).digest("hex"))' "$MOCK_SECRET" "$WEBHOOK_BODY")"
curl -sS -o "$TMP/hook.json" -w "%{http_code}" -X POST "$BASE/api/commerce/webhooks/mock" \
  -H 'Content-Type: application/json' -H "x-mock-signature: $WEBHOOK_SIG" -d "$WEBHOOK_BODY" > "$TMP/hook.code"
check "POST /api/commerce/webhooks/mock (signed)" 200 "$(cat "$TMP/hook.code")"
check_contains "webhook marks order paid" "$TMP/hook.json" '"order":"paid"'
curl -sS -o "$TMP/order-paid.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/commerce/orders/$ORDER_ID" > "$TMP/order-paid.code"
check_contains "order is paid after webhook" "$TMP/order-paid.json" '"status":"paid"'
curl -sS -o "$TMP/hook-replay.json" -w "%{http_code}" -X POST "$BASE/api/commerce/webhooks/mock" \
  -H 'Content-Type: application/json' -H "x-mock-signature: $WEBHOOK_SIG" -d "$WEBHOOK_BODY" > "$TMP/hook-replay.code"
check "POST /api/commerce/webhooks/mock (replay)" 200 "$(cat "$TMP/hook-replay.code")"
check_contains "webhook replay is duplicate" "$TMP/hook-replay.json" '"order":"duplicate"'
curl -sS -o /dev/null -w "%{http_code}" -X POST "$BASE/api/commerce/webhooks/mock" \
  -H 'Content-Type: application/json' -H "x-mock-signature: deadbeef" -d "$WEBHOOK_BODY" > "$TMP/hook-bad.code"
check "POST /api/commerce/webhooks/mock (bad signature)" 401 "$(cat "$TMP/hook-bad.code")"
curl -sS -o "$TMP/hook-log.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/commerce/webhooks/log" > "$TMP/hook-log.code"
check "GET /api/commerce/webhooks/log" 200 "$(cat "$TMP/hook-log.code")"
check_contains "webhook log has the reference" "$TMP/hook-log.json" "$ORDER_REF"
curl -sS -o "$TMP/orders.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/commerce/orders" > "$TMP/orders.code"
check "GET /api/commerce/orders" 200 "$(cat "$TMP/orders.code")"
check_contains "orders list contains smoke order" "$TMP/orders.json" "$ORDER_ID"

# --- security centre ---
curl -sS -o "$TMP/sec.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/security/overview" > "$TMP/sec.code"
check "GET /api/security/overview" 200 "$(cat "$TMP/sec.code")"
check_contains "security overview has headers" "$TMP/sec.json" '"headers"'
check_contains "security overview has failed-login audit" "$TMP/sec.json" '"failedLogins"'  # P5 F7: v1 shape — no "audit" key, audit log is GET /api/activity
check_contains "security overview has lockout state" "$TMP/sec.json" '"lockout"'

# --- import: WXR + SQL fixtures (same counts as v1) ---
curl -sS -o "$TMP/wxr.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/import/wxr" \
  -F "file=@$HOME/workspace/wordless/wordless-cms/test/fixtures/wordpress/wordpress-export.xml;filename=wordpress-export.xml;type=text/xml" > "$TMP/wxr.code"
check "POST /api/import/wxr" 201 "$(cat "$TMP/wxr.code")"
check_contains "wxr import finished done" "$TMP/wxr.json" '"status":"done"'
check_contains "wxr import users 2" "$TMP/wxr.json" '"user":{"failed":0,"imported":2'
check_contains "wxr import posts 3 (byType)" "$TMP/wxr.json" '"post":{"failed":0,"imported":3'
check_contains "wxr import page 11 imported" "$TMP/wxr.json" '"sourceId":"page:11","status":"imported"' 
WXR_JOB="$(jget "$TMP/wxr.json" 'd.data.id')"
curl -sS -o "$TMP/sql.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/import/sql" \
  -F "file=@$HOME/workspace/wordless/wordless-cms/test/fixtures/wordpress/wordpress-dump.sql;filename=wordpress-dump.sql;type=text/plain" > "$TMP/sql.code"
check "POST /api/import/sql" 201 "$(cat "$TMP/sql.code")"
check_contains "sql import finished done" "$TMP/sql.json" '"status":"done"'
curl -sS -o "$TMP/imp-job.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/import/jobs/$WXR_JOB" > "$TMP/imp-job.code"
check "GET /api/import/jobs/:id" 200 "$(cat "$TMP/imp-job.code")"
check_contains "import job is done" "$TMP/imp-job.json" '"status":"done"'

# --- graphql: same post as REST, same guards ---
curl -sS -o "$TMP/gql-post.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/posts" \
  -H 'Content-Type: application/json' \
  -d '{"title":"GraphQL Post","content":"<p>GraphQL check.</p>","status":"publish","slug":"gql-post"}' > "$TMP/gql-post.code"
check "POST /api/posts (graphql fixture post)" 201 "$(cat "$TMP/gql-post.code")"
curl -sS -o "$TMP/gql.json" -w "%{http_code}" -X POST "$BASE/api/graphql" \
  -H 'Content-Type: application/json' \
  -d '{"query":"{ post(slug: \"gql-post\") { id title status } }"}' > "$TMP/gql.code"
check "POST /api/graphql (post by slug)" 200 "$(cat "$TMP/gql.code")"
check_contains "graphql returns the published post" "$TMP/gql.json" '"title":"GraphQL Post"'
check_contains "graphql sees published status" "$TMP/gql.json" '"status":"publish"'
curl -sS -o "$TMP/gql2.json" -w "%{http_code}" -X POST "$BASE/api/graphql" \
  -H 'Content-Type: application/json' \
  -d '{"query":"{ siteInfo { title } }"}' > "$TMP/gql2.code"
check "POST /api/graphql (siteInfo)" 200 "$(cat "$TMP/gql2.code")"
# NOTE: the SQL import above overwrote blogname with its fixture value
# ("Fixture Site") — correct import behaviour, so siteInfo says that.
check_contains "graphql siteInfo title is the imported one" "$TMP/gql2.json" '"title":"Fixture Site"'
curl -sS -o "$TMP/gql3.json" -w "%{http_code}" -X POST "$BASE/api/graphql" \
  -H 'Content-Type: application/json' \
  -d '{"query":"{ users { total } }"}' > "$TMP/gql3.code"
check "POST /api/graphql (users, anonymous)" 200 "$(cat "$TMP/gql3.code")"
check_contains "graphql guards users from anonymous" "$TMP/gql3.json" '"errors"'
curl -sS -o "$TMP/gql4.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/graphql" \
  -H 'Content-Type: application/json' \
  -d '{"query":"query A { siteInfo { title } } query B { posts { total } }","operationName":"B"}' > "$TMP/gql4.code"
check "POST /api/graphql (operationName)" 200 "$(cat "$TMP/gql4.code")"
check_contains "graphql operationName picks B" "$TMP/gql4.json" '"posts"'

# --- outbound webhooks: subscribe, fire, retry, delete ---
curl -sS -o "$TMP/wh-sub.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/webhooks/subscriptions" \
  -H 'Content-Type: application/json' -d '{"url":"http://127.0.0.1:9/hook","events":["post.created"]}' > "$TMP/wh-sub.code"
check "POST /api/webhooks/subscriptions" 201 "$(cat "$TMP/wh-sub.code")"
check_contains "subscription secret returned once" "$TMP/wh-sub.json" '"secret":"'
SUB_ID="$(jget "$TMP/wh-sub.json" 'd.data.subscription.id')"
curl -sS -o "$TMP/wh-subs.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/webhooks/subscriptions" > "$TMP/wh-subs.code"
check "GET /api/webhooks/subscriptions" 200 "$(cat "$TMP/wh-subs.code")"
check_contains "subscription listed" "$TMP/wh-subs.json" '127.0.0.1:9/hook'
curl -sS -o "$TMP/wh-post.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/posts" \
  -H 'Content-Type: application/json' \
  -d '{"title":"Webhook Post","content":"<p>Webhook check.</p>","status":"publish","slug":"webhook-post"}' > "$TMP/wh-post.code"
check "POST /api/posts (fires webhook)" 201 "$(cat "$TMP/wh-post.code")"
curl -sS -o "$TMP/wh-deliv.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/webhooks/deliveries" > "$TMP/wh-deliv.code"
check "GET /api/webhooks/deliveries" 200 "$(cat "$TMP/wh-deliv.code")"
check_contains "delivery recorded for post.created" "$TMP/wh-deliv.json" '"event":"post.created"'
check_contains "delivery to dead URL is failed" "$TMP/wh-deliv.json" '"status":"failed"'
DELIVERY_ID="$(jget "$TMP/wh-deliv.json" 'd.data.deliveries[0].id')"
curl -sS -o "$TMP/wh-retry.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/webhooks/deliveries/$DELIVERY_ID/retry" > "$TMP/wh-retry.code"
check "POST /api/webhooks/deliveries/:id/retry" 200 "$(cat "$TMP/wh-retry.code")"
curl -sS -o "$TMP/wh-retry-due.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/webhooks/deliveries/retry-due" > "$TMP/wh-retry-due.code"
check "POST /api/webhooks/deliveries/retry-due" 200 "$(cat "$TMP/wh-retry-due.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $COOKIE" -X DELETE "$BASE/api/webhooks/subscriptions/$SUB_ID" > "$TMP/wh-del.code"
check "DELETE /api/webhooks/subscriptions/:id" 200 "$(cat "$TMP/wh-del.code")"

# --- shop: public storefront flow (mock) ---
curl -sS -o "$TMP/shop.html" -w "%{http_code}" "$BASE/shop" > "$TMP/shop.code"
check "GET /shop" 200 "$(cat "$TMP/shop.code")"
check_contains "shop lists smoke widget" "$TMP/shop.html" 'Smoke Widget'
curl -sS -o /dev/null -w "%{http_code}" -D "$TMP/shop-add.headers" -X POST "$BASE/shop/cart/add" \
  -H 'Content-Type: application/x-www-form-urlencoded' -d "productId=$PRODUCT_ID&quantity=1" > "$TMP/shop-add.code"  # P5 F3: camelCase
check "POST /shop/cart/add" 303 "$(cat "$TMP/shop-add.code")"
SHOP_COOKIE="$(grep -i '^set-cookie:' "$TMP/shop-add.headers" | head -1 | sed -e 's/^[^:]*: *//' -e 's/;.*//' | tr -d '\r')"
# (If the cart cookie was already set by the API flow, reuse it.)
if [ -z "$SHOP_COOKIE" ]; then SHOP_COOKIE="$CART_COOKIE"; fi
curl -sS -o "$TMP/shop-cart.html" -w "%{http_code}" -H "Cookie: $SHOP_COOKIE" "$BASE/shop/cart" > "$TMP/shop-cart.code"
check "GET /shop/cart" 200 "$(cat "$TMP/shop-cart.code")"
check_contains "shop cart has smoke widget" "$TMP/shop-cart.html" 'Smoke Widget'
curl -sS -o "$TMP/shop-product.html" -w "%{http_code}" "$BASE/shop/smoke-widget" > "$TMP/shop-product.code"
check "GET /shop/:slug" 200 "$(cat "$TMP/shop-product.code")"
check_contains "shop product page" "$TMP/shop-product.html" 'Smoke Widget'
# P5 F4: mock checkout 303s to the test page (v1 behaviour) — follow it.
curl -sS -o /dev/null -w "%{http_code}" -D "$TMP/shop-checkout.headers" -H "Cookie: $SHOP_COOKIE" -X POST "$BASE/shop/checkout" \
  -H 'Content-Type: application/x-www-form-urlencoded' -d "customerName=Shopper&customerEmail=shopper@example.com" > "$TMP/shop-checkout.code"  # P5 F3: camelCase
check "POST /shop/checkout (mock 303s to test page)" 303 "$(cat "$TMP/shop-checkout.code")"
check_contains "shop checkout redirects to test page" "$TMP/shop-checkout.headers" '/shop/checkout/test?order='
TEST_URL="$(grep -i '^location:' "$TMP/shop-checkout.headers" | head -1 | sed -e 's/^[^:]*: *//' -e 's|^https\?://[^/]*||' | tr -d '\r')"
curl -sS -o "$TMP/shop-checkout.html" -w "%{http_code}" -H "Cookie: $SHOP_COOKIE" "$BASE$TEST_URL" > "$TMP/shop-testpage.code"
check "GET /shop/checkout/test (mock test page)" 200 "$(cat "$TMP/shop-testpage.code")"
check_contains "shop test checkout page" "$TMP/shop-checkout.html" 'Test checkout'
check_contains "shop test page has simulate button" "$TMP/shop-checkout.html" 'Simulate successful payment'
curl -sS -o "$TMP/shop-orders.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/commerce/orders" > "$TMP/shop-orders.code"
SHOP_ORDER_ID="$(jget "$TMP/shop-orders.json" 'd.data.orders[0].id')"
curl -sS -o /dev/null -w "%{http_code}" -D "$TMP/shop-test.headers" -X POST "$BASE/shop/checkout/test" \
  -H 'Content-Type: application/x-www-form-urlencoded' -d "orderId=$SHOP_ORDER_ID" > "$TMP/shop-test.code"  # P5 F3: camelCase
check "POST /shop/checkout/test" 303 "$(cat "$TMP/shop-test.code")"
check_contains "test checkout redirects to success" "$TMP/shop-test.headers" "/shop/checkout/success?order=$SHOP_ORDER_ID"
curl -sS -o "$TMP/shop-success.html" -w "%{http_code}" "$BASE/shop/checkout/success?order=$SHOP_ORDER_ID" > "$TMP/shop-success.code"
check "GET /shop/checkout/success" 200 "$(cat "$TMP/shop-success.code")"
curl -sS -o "$TMP/shop-cancelled.html" -w "%{http_code}" "$BASE/shop/checkout/cancelled?order=$SHOP_ORDER_ID" > "$TMP/shop-cancelled.code"
check "GET /shop/checkout/cancelled" 200 "$(cat "$TMP/shop-cancelled.code")"

# --- CPT archive (P2 gap, closed in P3 wiring) ---
# P2 has no CPT CRUD API routes (service only), so the type is seeded
# directly in D1 here; P4 must add the routes for the admin UI.
CPT_SQL="$(mktemp)"
cat > "$CPT_SQL" <<'EOSQL'
INSERT INTO wp_options (option_name, option_value, autoload) VALUES ('wl_post_types', '{"book":{"slug":"book","label":"Books","singularLabel":"Book","description":"","public":true,"hasArchive":true,"hierarchical":false,"supports":["title","editor"],"rewriteSlug":"books","taxonomies":[],"builtin":false}}', 'yes') ON CONFLICT(option_name) DO UPDATE SET option_value=excluded.option_value;
EOSQL
"$WRANGLER" d1 execute DB --local --file "$CPT_SQL" >/dev/null
rm -f "$CPT_SQL"
curl -sS -o "$TMP/book.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/posts" \
  -H 'Content-Type: application/json' \
  -d '{"title":"Dune","content":"<p>A book about spice.</p>","status":"publish","slug":"dune","type":"book"}' > "$TMP/book.code"
check "POST /api/posts (custom type book)" 201 "$(cat "$TMP/book.code")"
curl -sS -o "$TMP/books.html" -w "%{http_code}" "$BASE/books" > "$TMP/books.code"
check "GET /books (CPT archive renders)" 200 "$(cat "$TMP/books.code")"
check_contains "CPT archive lists the book" "$TMP/books.html" 'Dune'
curl -sS -o "$TMP/book-single.html" -w "%{http_code}" "$BASE/books/dune" > "$TMP/book-single.code"
check "GET /books/dune (CPT single)" 200 "$(cat "$TMP/book-single.code")"

# --- redirect-update purge (P2 gap, closed in P3 wiring) ---
curl -sS -o "$TMP/redir2.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/seo/redirects" \
  -H 'Content-Type: application/json' -d '{"oldPath":"/old-purge","newPath":"/smoke-post"}' > "$TMP/redir2.code"
check "POST /api/seo/redirects (purge test)" 201 "$(cat "$TMP/redir2.code")"
REDIR2_ID="$(jget "$TMP/redir2.json" 'd.data.id')"
curl -sS -o /dev/null -w "%{http_code}" -D "$TMP/purge1.headers" "$BASE/old-purge" > "$TMP/purge1.code"
check "GET /old-purge redirects 301 (first)" 301 "$(cat "$TMP/purge1.code")"
check_contains "purge test target is smoke post" "$TMP/purge1.headers" '/smoke-post'
curl -sS -o "$TMP/redir2-upd.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X PUT "$BASE/api/seo/redirects/$REDIR2_ID" \
  -H 'Content-Type: application/json' -d '{"newPath":"/"}' > "$TMP/redir2-upd.code"
check "PUT /api/seo/redirects/:id (retarget)" 200 "$(cat "$TMP/redir2-upd.code")"
curl -sS -o /dev/null -w "%{http_code}" -D "$TMP/purge2.headers" "$BASE/old-purge" > "$TMP/purge2.code"
check "GET /old-purge redirects 301 (after update)" 301 "$(cat "$TMP/purge2.code")"
if grep -qi $'location: /\r$' "$TMP/purge2.headers"; then
  PASS=$((PASS + 1)); echo "  ok  purge test target is now home (old path purged)"
else
  FAIL=$((FAIL + 1)); echo "  FAIL purge test target is now home: $(grep -i '^location:' "$TMP/purge2.headers" | head -1)"
fi

# --- backup: create, verify, download, restore ---
curl -sS -o "$TMP/backup.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/backup/create" > "$TMP/backup.code"
check "POST /api/backup/create" 201 "$(cat "$TMP/backup.code")"
BACKUP_ID="$(jget "$TMP/backup.json" 'd.data.result.id')"
echo "  (backup id: $BACKUP_ID)"
curl -sS -o "$TMP/backups.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/backup" > "$TMP/backups.code"
check "GET /api/backup" 200 "$(cat "$TMP/backups.code")"
check_contains "backup list contains the backup" "$TMP/backups.json" "$BACKUP_ID"
curl -sS -o "$TMP/backup-verify.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/backup/$BACKUP_ID/verify" > "$TMP/backup-verify.code"
check "POST /api/backup/:id/verify" 200 "$(cat "$TMP/backup-verify.code")"
check_contains "backup verifies ok" "$TMP/backup-verify.json" '"ok":true'
curl -sS -o "$TMP/backup.zip" -w "%{http_code}" -D "$TMP/backup-dl.headers" -H "Cookie: $COOKIE" "$BASE/api/backup/$BACKUP_ID/download" > "$TMP/backup-dl.code"
check "GET /api/backup/:id/download" 200 "$(cat "$TMP/backup-dl.code")"
check_contains "backup download is a zip" "$TMP/backup-dl.headers" 'application/zip'
curl -sS -o "$TMP/backup-restore.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/backup/$BACKUP_ID/restore" \
  -H 'Content-Type: application/json' -d '{"confirmation":"RESTORE"}' > "$TMP/backup-restore.code"
check "POST /api/backup/:id/restore" 200 "$(cat "$TMP/backup-restore.code")"
check_contains "restore reports tables" "$TMP/backup-restore.json" '"restoredTables"'

# ======================================================== end P3 checks ==

# --- logout ---
curl -sS -o "$TMP/logout.json" -w "%{http_code}" -D "$TMP/logout.headers" -X POST \
  -H "Cookie: $COOKIE" "$BASE/api/auth/logout" > "$TMP/logout.code"
check "POST /api/auth/logout" 200 "$(cat "$TMP/logout.code")"
check_contains "logout clears cookie" "$TMP/logout.headers" 'Max-Age=0'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/auth/me" > "$TMP/me3.code"
check "GET /api/auth/me after logout" 401 "$(cat "$TMP/me3.code")"

# --- lockout (LAST: 5 failed logins from this IP lock the name AND the IP) ---
# One failure already happened earlier in this run, so the 5th attempt here
# must come back 429; run 6 and check the last.
LAST_CODE=""
for i in 1 2 3 4 5 6; do
  curl -sS -o /dev/null -w "%{http_code}" -X POST "$BASE/api/auth/login" \
    -H 'Content-Type: application/json' -d '{"login":"admin","password":"wrong"}' > "$TMP/lock.code"
  LAST_CODE="$(cat "$TMP/lock.code")"
done
check "lockout: 6th bad login is 429" 429 "$LAST_CODE"

echo
echo "== P3 smoke: $PASS passed, $FAIL failed =="
rm -rf "$TMP"
if [ "${1:-}" = "--keep" ]; then
  echo "Leaving wrangler dev running (PID $DEV_PID) on :$PORT"
  trap - EXIT
else
  cleanup
fi
[ "$FAIL" -eq 0 ]
