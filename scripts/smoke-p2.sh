#!/usr/bin/env bash
# WORDLESS v2 P2 smoke test — runs the Rust worker under local wrangler dev
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
PORT="${SMOKE_PORT:-8797}"
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
  node -e 'const fs=require("fs");const d=JSON.parse(fs.readFileSync(process.argv[1],"utf8"));const v=eval(process.argv[2]);console.log(v===undefined||v===null?"":v);' "$1" "$2"
}

echo "== P2 smoke: applying migrations (local D1) =="
"$WRANGLER" d1 migrations apply DB --local >/dev/null

echo "== P2 smoke: seeding admin (login admin / smoke-admin-2026) =="
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

echo "== P2 smoke: starting wrangler dev on :$PORT =="
echo "== P2 smoke: starting SEO Worker (port 8812) =="
source "$ROOT/scripts/split-workers-dev.sh"
start_seo

"$WRANGLER" dev --port "$PORT" --local --var "WORDLESS_SEO_URL:$SEO_URL" > /tmp/wordless-p2-wrangler.log 2>&1 &
DEV_PID=$!
cleanup() { kill "$DEV_PID" 2>/dev/null || true; stop_seo; }
trap cleanup EXIT

# Wait for the worker to answer (wrangler dev builds first — can take a while).
for i in $(seq 1 240); do
  if curl -fsS -o /dev/null "$BASE/api/health" 2>/dev/null; then break; fi
  sleep 2
done

echo "== P2 smoke: checks =="
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
echo "== P2 smoke: $PASS passed, $FAIL failed =="
rm -rf "$TMP"
if [ "${1:-}" = "--keep" ]; then
  echo "Leaving wrangler dev running (PID $DEV_PID) on :$PORT"
  trap - EXIT
else
  cleanup
fi
[ "$FAIL" -eq 0 ]
