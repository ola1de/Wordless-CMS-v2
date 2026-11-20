#!/usr/bin/env bash
# WORDLESS v2 — SEO split end-to-end (one-off verification, 2026-11-20).
# Proves the SEO surface works THROUGH the main-Worker proxy, not just
# inside the SEO Worker: public post page renders JSON-LD + breadcrumbs +
# canonical from main's render path; sitemap index + image sitemap +
# llms.txt serve through main; a redirect create returns no leaked
# X-Wordless-Purge header to the client.
set -u
export PATH="$HOME/.cargo/bin:$PATH" # worker-build lives here (same as the smoke scripts)
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WRANGLER="${WRANGLER:-$ROOT/node_modules/.bin/wrangler}"
PORT=8813
BASE="http://127.0.0.1:$PORT"
PASS=0; FAIL=0
check() { # check <label> <expected> <actual>
  if [ "$2" = "$3" ]; then PASS=$((PASS+1)); echo "  ok  $1"; else FAIL=$((FAIL+1)); echo "  FAIL  $1 (expected [$2] got [$3])"; fi
}
check_contains() { # check_contains <label> <file> <needle>
  if grep -qF "$3" "$2"; then PASS=$((PASS+1)); echo "  ok  $1"; else FAIL=$((FAIL+1)); echo "  FAIL  $1 (missing [$3] in $2)"; fi
}
check_absent() { # check_absent <label> <file> <needle>
  if grep -qiF "$3" "$2"; then FAIL=$((FAIL+1)); echo "  FAIL  $1 (found [$3] in $2)"; else PASS=$((PASS+1)); echo "  ok  $1"; fi
}

rm -rf "$ROOT/.wrangler"
(cd "$ROOT" && "$WRANGLER" d1 migrations apply DB --local >/dev/null)

echo "== SEO e2e: seeding admin + published post =="
SEED_SQL="$(mktemp)"
node > "$SEED_SQL" <<'NODE'
const { pbkdf2Sync, randomBytes } = require('node:crypto');
const salt = randomBytes(16);
const h = pbkdf2Sync('smoke-admin-2026', salt, 100000, 32, 'sha256');
const stored = 'pbkdf2$100000$' + salt.toString('base64url') + '$' + h.toString('base64url');
const esc = (s) => "'" + s.replace(/'/g, "''") + "'";
const now = '2026-11-20 10:00:00';
console.log('INSERT OR IGNORE INTO wp_users (user_login, user_pass, user_nicename, user_email, user_url, user_registered, display_name) VALUES (' + esc('admin') + ', ' + esc(stored) + ', ' + esc('admin') + ', ' + esc('admin@example.com') + ', ' + esc('') + ', ' + esc(now) + ', ' + esc('admin') + ');');
console.log('INSERT OR IGNORE INTO wp_usermeta (user_id, meta_key, meta_value) VALUES ((SELECT ID FROM wp_users WHERE user_login = ' + esc('admin') + '), ' + esc('wp_capabilities') + ', ' + esc('{"administrator":true}') + ');');
console.log('INSERT OR REPLACE INTO wp_options (option_name, option_value) VALUES (' + esc('siteurl') + ', ' + esc('"http://127.0.0.1:8813"') + ');');
console.log("INSERT INTO wp_posts (post_author, post_date, post_date_gmt, post_content, post_title, post_excerpt, post_status, comment_status, ping_status, post_name, post_type) VALUES (1, '2026-11-20 10:00:00', '2026-11-20 10:00:00', 'SEO end-to-end test content for the split build.', 'Hello SEO', 'SEO excerpt.', 'publish', 'open', 'open', 'hello-seo', 'post');");
NODE
"$WRANGLER" d1 execute DB --local --file "$SEED_SQL" >/dev/null
rm -f "$SEED_SQL"

echo "== SEO e2e: starting SEO Worker (port 8812) =="
source "$ROOT/scripts/split-workers-dev.sh"
start_seo

echo "== SEO e2e: starting main Worker on :$PORT =="
"$WRANGLER" dev --port "$PORT" --local --var "WORDLESS_SEO_URL:$SEO_URL" >/tmp/seo-e2e-wrangler.log 2>&1 &
DEV_PID=$!
cleanup() { kill "$DEV_PID" 2>/dev/null || true; stop_seo; }
trap cleanup EXIT
for i in $(seq 1 240); do
  if curl -fsS -o /dev/null "$BASE/api/health" 2>/dev/null; then break; fi
  sleep 2
done

TMP="$(mktemp -d)"
# Login as admin (for the redirect create).
curl -sS -o "$TMP/login.json" -w "%{http_code}" -D "$TMP/login.headers" -X POST "$BASE/api/auth/login" \
  -H 'Content-Type: application/json' -d '{"login":"admin","password":"smoke-admin-2026"}' > "$TMP/login.code"
check "login admin" 200 "$(cat "$TMP/login.code")"
COOKIE=$(grep -i '^set-cookie:' "$TMP/login.headers" | head -1 | sed 's/^[^:]*: *//; s/;.*//')

# 1. Public post page through main: JSON-LD + breadcrumbs + canonical.
curl -sS -o "$TMP/post.html" -w "%{http_code}" "$BASE/hello-seo/?e2e=1" > "$TMP/post.code"
check "GET /hello-seo/ status" 200 "$(cat "$TMP/post.code")"
check_contains "post page has JSON-LD script" "$TMP/post.html" 'application/ld+json'
check_contains "post page JSON-LD is an Article" "$TMP/post.html" '"@type":"Article"'
check_contains "post page has breadcrumb nav" "$TMP/post.html" 'aria-label="Breadcrumb"'
check_contains "post page has canonical link" "$TMP/post.html" 'rel="canonical"'
check_contains "post page canonical points at the post" "$TMP/post.html" '/hello-seo/'

# 2. Sitemap index + image sitemap + llms.txt through main.
curl -sS -o "$TMP/sitemap-index.xml" -w "%{http_code}" "$BASE/sitemap-index.xml" > "$TMP/si.code"
check "GET /sitemap-index.xml status" 200 "$(cat "$TMP/si.code")"
check_contains "sitemap index lists the main sitemap" "$TMP/sitemap-index.xml" 'sitemap.xml'
curl -sS -o "$TMP/sitemap-images.xml" -w "%{http_code}" "$BASE/sitemap-images.xml" > "$TMP/sim.code"
check "GET /sitemap-images.xml status" 200 "$(cat "$TMP/sim.code")"
check_contains "image sitemap is a urlset" "$TMP/sitemap-images.xml" 'urlset'
curl -sS -o "$TMP/llms.txt" -w "%{http_code}" "$BASE/llms.txt" > "$TMP/llms.code"
check "GET /llms.txt status" 200 "$(cat "$TMP/llms.code")"
check_contains "llms.txt mentions the post" "$TMP/llms.txt" 'Hello SEO'
curl -sS -o "$TMP/robots.txt" -w "%{http_code}" "$BASE/robots.txt" > "$TMP/robots.code"
check "GET /robots.txt status" 200 "$(cat "$TMP/robots.code")"
check_contains "robots.txt lists sitemaps" "$TMP/robots.txt" 'Sitemap:'

# 3. Redirect create through the proxy: no leaked purge header, redirect works.
curl -sS -o "$TMP/redirect.json" -w "%{http_code}" -D "$TMP/redirect.headers" -X POST "$BASE/api/seo/redirects" \
  -H 'Content-Type: application/json' -H "Cookie: $COOKIE" \
  -d '{"oldPath":"/old-seo-page","newPath":"/hello-seo/"}' > "$TMP/redirect.code"
check "POST /api/seo/redirects status" 201 "$(cat "$TMP/redirect.code")"
check_absent "redirect create leaks no X-Wordless-Purge header" "$TMP/redirect.headers" 'X-Wordless-Purge'
curl -sS -o /dev/null -w "%{http_code}" -D "$TMP/redir2.headers" "$BASE/old-seo-page" > "$TMP/redir2.code"
check "GET /old-seo-page redirects" 301 "$(cat "$TMP/redir2.code")"
grep -qi 'location: .*hello-seo' "$TMP/redir2.headers" && { PASS=$((PASS+1)); echo "  ok  redirect target is hello-seo (stored target is the bare slug — pre-existing normalisation)"; } || { FAIL=$((FAIL+1)); echo "  FAIL  redirect target is hello-seo"; }

echo ""
echo "== SEO e2e: $PASS passed, $FAIL failed =="
[ "$FAIL" = 0 ]
