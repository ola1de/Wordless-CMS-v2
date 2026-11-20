#!/usr/bin/env bash
# WORDLESS v2 P12 smoke test — Multisite end to end.
#
# Runs the Rust worker under local wrangler dev (port 8815) against a
# local D1 (migrations applied, one seeded admin) and checks:
#   sites API: list (1 site to start), create 201, duplicate slug 400,
#     reserved slug 400, bad slug 400, get one 200, missing site 404,
#     current-site endpoint, anon 401 on every sites route.
#   isolation, per query path (the load-bearing property — a post,
#     term, setting, redirect or 404 logged on one site must never
#     show on the other):
#     posts: same slug on both sites is fine; list/get/search are
#       scoped; cross-site get by id is 404.
#     terms: same slug "news" on both sites, each list shows its own.
#     settings: site 2 falls back to the network blogname until it
#       sets its own; changing site 2's title never touches site 1.
#     options API: write of a non-site-scoped key is 400.
#     redirects: the same old path may exist on both sites with
#       different targets; each site follows its own.
#     404 log: each site's log lists only its own paths.
#   switcher behaviour: X-Wordless-Site header selects the site for
#     an admin (list, settings, current); an unknown header site
#     falls back to site 1 rather than failing the request.
#   path-based routing: /blog/ serves the blog home, /blog/blog-post
#     the blog post, /blog-post and /blog/main-post are 404.
#   subdomain routing (WORDLESS_NETWORK_DOMAIN=example.com):
#     Host: blog.example.com serves the blog home; www falls back
#     to the main site; an unknown subdomain falls back too.
#   archived sites: public pages 404, the API still administers them.
#   delete: site 1 is refused (400); deleting site 2 wipes its posts
#     (deletedPosts count) and its paths 404 afterwards.
#   admin SPA: the built bundle ships the #/sites route.
#
# Multisite routing and content isolation live entirely in the main
# Worker, but the redirects and 404-log APIs are proxied to the SEO
# Worker (P10 split), so it is started first — same as smoke-p10.
# Clean room first: rm -rf .wrangler (the script applies migrations
# itself). Orphaned workerd check first (P11 lesson: a stale workerd
# holding the port poisons the run).
#
# Usage: ./scripts/smoke-p12.sh [--keep]   (--keep leaves wrangler dev running)
# Requires: wrangler (TS repo's project-managed binary), worker-build, node.
# Seeded admin (password "smoke-admin-2026"): login "admin".
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"

WRANGLER="$HOME/workspace/wordless/wordless-cms/node_modules/.bin/wrangler"
PORT="${SMOKE_PORT:-8815}"
BASE="http://127.0.0.1:$PORT"
PASS=0
FAIL=0

check_ne() { # check_ne <name> <value-a> <value-b> — passes when they differ
  local name="$1" a="$2" b="$3"
  if [ "$a" != "$b" ]; then
    PASS=$((PASS + 1)); echo "  ok  $name ($a != $b)"
  else
    FAIL=$((FAIL + 1)); echo "  FAIL $name: expected different values, both are $a"
  fi
}

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

check_not_contains() { # check_not_contains <name> <file> <substring>
  if grep -qF "$3" "$2"; then
    FAIL=$((FAIL + 1)); echo "  FAIL $1: '$3' should not be in $2"
  else
    PASS=$((PASS + 1)); echo "  ok  $1"
  fi
}

# jget <file> <js-expression-on-d> — extracts a value from an envelope JSON.
jget() {
  node -e 'const fs=require("fs");const d=JSON.parse(fs.readFileSync(process.argv[1],"utf8"));let v="";try{v=eval(process.argv[2]);}catch(e){v="";}console.log(v===undefined||v===null?"":v);' "$1" "$2"
}

# --- P11 lesson: an orphaned workerd holding the port poisons the run.
for P in "$PORT" 8812; do
  if ss -ltnp 2>/dev/null | grep -q ":$P "; then
    echo "FAIL: port $P is already held (orphaned workerd?). Refusing to run against a stale worker."
    ss -ltnp 2>/dev/null | grep ":$P " || true
    exit 1
  fi
done

KEEP=0
[ "${1:-}" = "--keep" ] && KEEP=1

echo "== P12 smoke: clean room + applying migrations (local D1) =="
rm -rf .wrangler
"$WRANGLER" d1 migrations apply DB --local >/dev/null

echo "== P12 smoke: seeding admin (password smoke-admin-2026) =="
SEED_SQL="$(mktemp)"
node > "$SEED_SQL" <<'NODE'
const { pbkdf2Sync, randomBytes } = require('node:crypto');
const esc = (s) => "'" + s.replace(/'/g, "''") + "'";
const now = new Date().toISOString().replace('T', ' ').replace(/\.\d+Z$/, '');
const salt = randomBytes(16);
const h = pbkdf2Sync('smoke-admin-2026', salt, 100000, 32, 'sha256');
const stored = 'pbkdf2$100000$' + salt.toString('base64url') + '$' + h.toString('base64url');
console.log('INSERT OR IGNORE INTO wp_users (user_login, user_pass, user_nicename, user_email, user_url, user_registered, display_name) VALUES (' + esc('admin') + ', ' + esc(stored) + ', ' + esc('admin') + ', ' + esc('admin@example.com') + ', ' + esc('') + ', ' + esc(now) + ', ' + esc('Admin') + ');');
console.log('INSERT OR IGNORE INTO wp_usermeta (user_id, meta_key, meta_value) VALUES ((SELECT ID FROM wp_users WHERE user_login = ' + esc('admin') + '), ' + esc('wp_capabilities') + ', ' + esc('{"administrator":true}') + ');');
NODE
"$WRANGLER" d1 execute DB --local --file "$SEED_SQL" >/dev/null
rm -f "$SEED_SQL"

# Point the site at this smoke run's own base URL, so absolute URLs
# use the smoke port instead of the localhost:8787 default. NOTE:
# options are JSON-encoded on write, so the value needs its JSON
# double quotes (this bit us once).
echo "INSERT OR REPLACE INTO wp_options (option_name, option_value) VALUES ('siteurl', '\"http://127.0.0.1:$PORT\"');" > "$SEED_SQL"
"$WRANGLER" d1 execute DB --local --file "$SEED_SQL" >/dev/null
rm -f "$SEED_SQL"

echo "== P12 smoke: starting SEO Worker (port 8812, for /api/seo/*) =="
source "$ROOT/scripts/split-workers-dev.sh"
start_seo

echo "== P12 smoke: starting wrangler dev on :$PORT (network domain example.com) =="
"$WRANGLER" dev --port "$PORT" --local --var "WORDLESS_NETWORK_DOMAIN:example.com" --var "WORDLESS_SEO_URL:$SEO_URL" >/tmp/smoke-p12-wrangler.log 2>&1 &
DEV_PID=$!
cleanup() {
  if [ "$KEEP" -eq 1 ]; then echo "  (--keep: wrangler dev left running, pid $DEV_PID)"; return; fi
  kill "$DEV_PID" 2>/dev/null || true
  for child in $(pgrep -P "$DEV_PID" 2>/dev/null); do kill "$child" 2>/dev/null || true; done
  stop_seo
}
trap cleanup EXIT

# Wait for the worker to answer (wrangler dev builds first — can take a while).
for i in $(seq 1 240); do
  if curl -fsS -o /dev/null "$BASE/api/health" 2>/dev/null; then break; fi
  sleep 2
done

echo "== P12 smoke: checks =="
TMP="$(mktemp -d)"

curl -sS -o "$TMP/login.json" -w "%{http_code}" -D "$TMP/login.headers" -X POST "$BASE/api/auth/login" \
  -H 'Content-Type: application/json' -d '{"login":"admin","password":"smoke-admin-2026"}' > "$TMP/login.code"
check "login admin" 200 "$(cat "$TMP/login.code")"
ADMIN_COOKIE="$(grep -i '^set-cookie:' "$TMP/login.headers" | head -1 | sed -e 's/^[^:]*: *//' -e 's/;.*//' | tr -d '\r')"
AUTH=(-H "Cookie: $ADMIN_COOKIE")
SITE2=(-H "X-Wordless-Site: 2")

# --- sites API: list / create / validation ---------------------------
curl -sS -o "$TMP/sites.json" -w "%{http_code}" "${AUTH[@]}" "$BASE/api/sites" > "$TMP/sites.code"
check "GET /api/sites" 200 "$(cat "$TMP/sites.code")"
check "one site to start" 1 "$(jget "$TMP/sites.json" 'd.data.length')"
check "main site slug is empty" "" "$(jget "$TMP/sites.json" 'd.data[0].slug')"
curl -sS -o /dev/null -w "%{http_code}" "$BASE/api/sites" > "$TMP/sites-anon.code"
check "GET /api/sites anon" 401 "$(cat "$TMP/sites-anon.code")"

curl -sS -o "$TMP/site2.json" -w "%{http_code}" "${AUTH[@]}" -X POST "$BASE/api/sites" \
  -H 'Content-Type: application/json' -d '{"slug":"blog","title":"Company Blog"}' > "$TMP/site2.code"
check "POST /api/sites blog" 201 "$(cat "$TMP/site2.code")"
check "blog site id is 2" 2 "$(jget "$TMP/site2.json" 'd.data.id')"
curl -sS -o /dev/null -w "%{http_code}" "${AUTH[@]}" -X POST "$BASE/api/sites" \
  -H 'Content-Type: application/json' -d '{"slug":"blog","title":"Duplicate"}' > "$TMP/site-dup.code"
check "POST duplicate slug" 400 "$(cat "$TMP/site-dup.code")"
curl -sS -o /dev/null -w "%{http_code}" "${AUTH[@]}" -X POST "$BASE/api/sites" \
  -H 'Content-Type: application/json' -d '{"slug":"admin","title":"Reserved"}' > "$TMP/site-res.code"
check "POST reserved slug admin" 400 "$(cat "$TMP/site-res.code")"
curl -sS -o /dev/null -w "%{http_code}" "${AUTH[@]}" -X POST "$BASE/api/sites" \
  -H 'Content-Type: application/json' -d '{"slug":"Bad_Slug!","title":"Bad"}' > "$TMP/site-bad.code"
check "POST bad slug" 400 "$(cat "$TMP/site-bad.code")"
curl -sS -o /dev/null -w "%{http_code}" "$BASE/api/sites" -X POST \
  -H 'Content-Type: application/json' -d '{"slug":"anon","title":"Anon"}' > "$TMP/site-anon.code"
check "POST /api/sites anon" 401 "$(cat "$TMP/site-anon.code")"
curl -sS -o "$TMP/site2-get.json" -w "%{http_code}" "${AUTH[@]}" "$BASE/api/sites/2" > "$TMP/site2-get.code"
check "GET /api/sites/2" 200 "$(cat "$TMP/site2-get.code")"
curl -sS -o /dev/null -w "%{http_code}" "${AUTH[@]}" "$BASE/api/sites/99" > "$TMP/site99.code"
check "GET /api/sites/99 missing" 404 "$(cat "$TMP/site99.code")"

# --- switcher behaviour: current site --------------------------------
curl -sS -o "$TMP/current1.json" -w "%{http_code}" "${AUTH[@]}" "$BASE/api/sites/current" > "$TMP/current1.code"
check "GET /api/sites/current (no header)" 200 "$(cat "$TMP/current1.code")"
check "current site is 1 without a header" 1 "$(jget "$TMP/current1.json" 'd.data.id')"
curl -sS -o "$TMP/current2.json" -w "%{http_code}" "${AUTH[@]}" "${SITE2[@]}" "$BASE/api/sites/current" > "$TMP/current2.code"
check "GET /api/sites/current (header 2)" 200 "$(cat "$TMP/current2.code")"
check "current site is 2 with the header" 2 "$(jget "$TMP/current2.json" 'd.data.id')"
curl -sS -o "$TMP/current999.json" -w "%{http_code}" "${AUTH[@]}" -H "X-Wordless-Site: 999" "$BASE/api/sites/current" > "$TMP/current999.code"
check "GET /api/sites/current (unknown header site)" 200 "$(cat "$TMP/current999.code")"
check "unknown header site falls back to site 1" 1 "$(jget "$TMP/current999.json" 'd.data.id')"

# --- posts: one per site, same category slug on both ------------------
curl -sS -o "$TMP/post1.json" -w "%{http_code}" "${AUTH[@]}" -X POST "$BASE/api/posts" \
  -H 'Content-Type: application/json' \
  -d '{"title":"Main Post","content":"<p>Main site post.</p>","status":"publish","slug":"main-post"}' > "$TMP/post1.code"
check "POST main post on site 1" 201 "$(cat "$TMP/post1.code")"
POST1_ID="$(jget "$TMP/post1.json" 'd.data.id')"
curl -sS -o "$TMP/post2.json" -w "%{http_code}" "${AUTH[@]}" "${SITE2[@]}" -X POST "$BASE/api/posts" \
  -H 'Content-Type: application/json' \
  -d '{"title":"Blog Post","content":"<p>Blog site post.</p>","status":"publish","slug":"blog-post"}' > "$TMP/post2.code"
check "POST blog post on site 2" 201 "$(cat "$TMP/post2.code")"
POST2_ID="$(jget "$TMP/post2.json" 'd.data.id')"
echo "  (post ids: site1=$POST1_ID site2=$POST2_ID)"

curl -sS -o "$TMP/posts1.json" -w "%{http_code}" "${AUTH[@]}" "$BASE/api/posts" > "$TMP/posts1.code"
check "GET /api/posts site 1" 200 "$(cat "$TMP/posts1.code")"
check_contains "site 1 list has the main post" "$TMP/posts1.json" 'main-post'
check_not_contains "site 1 list has no blog post" "$TMP/posts1.json" 'blog-post'
curl -sS -o "$TMP/posts2.json" -w "%{http_code}" "${AUTH[@]}" "${SITE2[@]}" "$BASE/api/posts" > "$TMP/posts2.code"
check "GET /api/posts site 2" 200 "$(cat "$TMP/posts2.code")"
check_contains "site 2 list has the blog post" "$TMP/posts2.json" 'blog-post'
check_not_contains "site 2 list has no main post" "$TMP/posts2.json" 'main-post'
curl -sS -o /dev/null -w "%{http_code}" "${AUTH[@]}" "${SITE2[@]}" "$BASE/api/posts/$POST1_ID" > "$TMP/cross.code"
check "cross-site get by id is 404" 404 "$(cat "$TMP/cross.code")"
curl -sS -o "$TMP/search2.html" -w "%{http_code}" "${AUTH[@]}" "${SITE2[@]}" "$BASE/search?q=Blog" > "$TMP/search2.code"
check "search on site 2" 200 "$(cat "$TMP/search2.code")"
check_contains "site 2 search finds the blog post" "$TMP/search2.html" 'Blog Post'
curl -sS -o "$TMP/search1.html" -w "%{http_code}" "${AUTH[@]}" "$BASE/search?q=Blog" > "$TMP/search1.code"
check "search on site 1" 200 "$(cat "$TMP/search1.code")"
check_not_contains "site 1 search finds no blog post" "$TMP/search1.html" 'Blog Post'

# --- terms: same slug on both sites -----------------------------------
curl -sS -o /dev/null -w "%{http_code}" "${AUTH[@]}" -X POST "$BASE/api/terms" \
  -H 'Content-Type: application/json' -d '{"name":"News","slug":"news","taxonomy":"category"}' > "$TMP/term1.code"
check "POST category news on site 1" 201 "$(cat "$TMP/term1.code")"
curl -sS -o /dev/null -w "%{http_code}" "${AUTH[@]}" "${SITE2[@]}" -X POST "$BASE/api/terms" \
  -H 'Content-Type: application/json' -d '{"name":"News","slug":"news","taxonomy":"category"}' > "$TMP/term2.code"
check "POST category news on site 2 (same slug allowed)" 201 "$(cat "$TMP/term2.code")"
curl -sS -o "$TMP/terms1.json" -w "%{http_code}" "${AUTH[@]}" "$BASE/api/terms?taxonomy=category" > "$TMP/terms1.code"
check "GET terms site 1" 200 "$(cat "$TMP/terms1.code")"
check "site 1 has one category" 1 "$(jget "$TMP/terms1.json" 'd.data.terms.length')"
curl -sS -o "$TMP/terms2.json" -w "%{http_code}" "${AUTH[@]}" "${SITE2[@]}" "$BASE/api/terms?taxonomy=category" > "$TMP/terms2.code"
check "GET terms site 2" 200 "$(cat "$TMP/terms2.code")"
check "site 2 has one category" 1 "$(jget "$TMP/terms2.json" 'd.data.terms.length')"
check_ne "the two news categories are different rows" \
  "$(jget "$TMP/terms1.json" 'd.data.terms[0].termTaxonomyId')" \
  "$(jget "$TMP/terms2.json" 'd.data.terms[0].termTaxonomyId')"

# --- settings isolation ----------------------------------------------
curl -sS -o /dev/null -w "%{http_code}" "${AUTH[@]}" -X PUT "$BASE/api/settings" \
  -H 'Content-Type: application/json' -d '{"siteTitle":"Main Site Title"}' > "$TMP/set1.code"
check "PUT settings on site 1" 200 "$(cat "$TMP/set1.code")"
curl -sS -o "$TMP/opt2.json" -w "%{http_code}" "${AUTH[@]}" "$BASE/api/sites/2/options/blogname" > "$TMP/opt2.code"
check "GET site 2 blogname option" 200 "$(cat "$TMP/opt2.code")"
check_contains "site 2 falls back to the network title" "$TMP/opt2.json" 'Main Site Title'
curl -sS -o "$TMP/opt2put.json" -w "%{http_code}" "${AUTH[@]}" -X PUT "$BASE/api/sites/2/options/blogname" \
  -H 'Content-Type: application/json' -d '{"value":"Blog Title"}' > "$TMP/opt2put.code"
check "PUT site 2 blogname option" 200 "$(cat "$TMP/opt2put.code")"
curl -sS -o "$TMP/set2.json" -w "%{http_code}" "${AUTH[@]}" "${SITE2[@]}" "$BASE/api/settings" > "$TMP/set2.code"
check "GET settings with header 2" 200 "$(cat "$TMP/set2.code")"
check_contains "site 2 settings show its own title" "$TMP/set2.json" 'Blog Title'
curl -sS -o "$TMP/set1.json" -w "%{http_code}" "${AUTH[@]}" "$BASE/api/settings" > "$TMP/set1b.code"
check "GET settings site 1 again" 200 "$(cat "$TMP/set1b.code")"
check_contains "site 1 title untouched by site 2" "$TMP/set1.json" 'Main Site Title'
check_not_contains "site 1 settings have no blog title" "$TMP/set1.json" 'Blog Title'
curl -sS -o /dev/null -w "%{http_code}" "${AUTH[@]}" -X PUT "$BASE/api/sites/2/options/not_a_site_key" \
  -H 'Content-Type: application/json' -d '{"value":"x"}' > "$TMP/optbad.code"
check "PUT non-site-scoped option key" 400 "$(cat "$TMP/optbad.code")"
curl -sS -o /dev/null -w "%{http_code}" "$BASE/api/sites/2/options/blogname" > "$TMP/optanon.code"
check "GET site option anon" 401 "$(cat "$TMP/optanon.code")"
curl -sS -o "$TMP/opt2del.json" -w "%{http_code}" "${AUTH[@]}" -X DELETE "$BASE/api/sites/2/options/blogname" > "$TMP/opt2del.code"
check "DELETE site 2 blogname override" 200 "$(cat "$TMP/opt2del.code")"
check_contains "override deleted" "$TMP/opt2del.json" '"deleted":true'
curl -sS -o "$TMP/opt2b.json" -w "%{http_code}" "${AUTH[@]}" "$BASE/api/sites/2/options/blogname" > "$TMP/opt2b.code"
check_contains "site 2 falls back again after delete" "$TMP/opt2b.json" 'Main Site Title'

# --- redirects: same old path, different targets ----------------------
curl -sS -o /dev/null -w "%{http_code}" "${AUTH[@]}" -X POST "$BASE/api/seo/redirects" \
  -H 'Content-Type: application/json' -d '{"oldPath":"/old-page","newPath":"/main-post"}' > "$TMP/redir1.code"
check "POST redirect on site 1" 201 "$(cat "$TMP/redir1.code")"
curl -sS -o /dev/null -w "%{http_code}" "${AUTH[@]}" "${SITE2[@]}" -X POST "$BASE/api/seo/redirects" \
  -H 'Content-Type: application/json' -d '{"oldPath":"/old-page","newPath":"/blog-post"}' > "$TMP/redir2.code"
check "POST same old path on site 2" 201 "$(cat "$TMP/redir2.code")"
curl -sS -o /dev/null -w "%{http_code}" -D "$TMP/redir1.headers" "$BASE/old-page?x=1" > "$TMP/redir1f.code"
check "GET /old-page redirects on site 1" 301 "$(cat "$TMP/redir1f.code")"
check_contains "site 1 redirect points at the main post" "$TMP/redir1.headers" '/main-post'
curl -sS -o /dev/null -w "%{http_code}" -D "$TMP/redir2.headers" "$BASE/blog/old-page?x=1" > "$TMP/redir2f.code"
check "GET /blog/old-page redirects on site 2" 301 "$(cat "$TMP/redir2f.code")"
check_contains "site 2 redirect points at the blog post" "$TMP/redir2.headers" '/blog-post'

# --- 404 log isolation ------------------------------------------------
curl -sS -o /dev/null -w "%{http_code}" "$BASE/missing-on-main?x=1" > /dev/null
curl -sS -o /dev/null -w "%{http_code}" "$BASE/blog/missing-on-blog?x=1" > /dev/null
curl -sS -o "$TMP/404s1.json" -w "%{http_code}" "${AUTH[@]}" "$BASE/api/seo/404s" > "$TMP/404s1.code"
check "GET 404 log site 1" 200 "$(cat "$TMP/404s1.code")"
check_contains "site 1 log has its own missing path" "$TMP/404s1.json" 'missing-on-main'
check_not_contains "site 1 log has no blog path" "$TMP/404s1.json" 'missing-on-blog'
curl -sS -o "$TMP/404s2.json" -w "%{http_code}" "${AUTH[@]}" "${SITE2[@]}" "$BASE/api/seo/404s" > "$TMP/404s2.code"
check "GET 404 log site 2" 200 "$(cat "$TMP/404s2.code")"
check_contains "site 2 log has its own missing path" "$TMP/404s2.json" 'missing-on-blog'
check_not_contains "site 2 log has no main path" "$TMP/404s2.json" 'missing-on-main'

# --- public rendering: path-based routing -----------------------------
curl -sS -o "$TMP/home1.html" -w "%{http_code}" "$BASE/?x=1" > "$TMP/home1.code"
check "GET / home" 200 "$(cat "$TMP/home1.code")"
check_contains "main home lists the main post" "$TMP/home1.html" 'Main Post'
check_not_contains "main home lists no blog post" "$TMP/home1.html" 'Blog Post'
curl -sS -o "$TMP/home2.html" -w "%{http_code}" "$BASE/blog/?x=1" > "$TMP/home2.code"
check "GET /blog/ home" 200 "$(cat "$TMP/home2.code")"
check_contains "blog home lists the blog post" "$TMP/home2.html" 'Blog Post'
check_not_contains "blog home lists no main post" "$TMP/home2.html" 'Main Post'
curl -sS -o "$TMP/single2.html" -w "%{http_code}" "$BASE/blog/blog-post?x=1" > "$TMP/single2.code"
check "GET /blog/blog-post" 200 "$(cat "$TMP/single2.code")"
check_contains "blog post page renders" "$TMP/single2.html" 'Blog Post'
curl -sS -o /dev/null -w "%{http_code}" "$BASE/blog-post?x=1" > "$TMP/single-cross.code"
check "GET /blog-post without prefix is 404" 404 "$(cat "$TMP/single-cross.code")"
curl -sS -o /dev/null -w "%{http_code}" "$BASE/blog/main-post?x=1" > "$TMP/single-cross2.code"
check "GET /blog/main-post is 404" 404 "$(cat "$TMP/single-cross2.code")"

# --- public rendering: subdomain routing ------------------------------
curl -sS -o "$TMP/sub2.html" -w "%{http_code}" -H "Host: blog.example.com" "$BASE/?x=1" > "$TMP/sub2.code"
check "GET / with Host blog.example.com" 200 "$(cat "$TMP/sub2.code")"
check_contains "subdomain serves the blog home" "$TMP/sub2.html" 'Blog Post'
curl -sS -o "$TMP/subwww.html" -w "%{http_code}" -H "Host: www.example.com" "$BASE/?x=1" > "$TMP/subwww.code"
check "GET / with Host www.example.com" 200 "$(cat "$TMP/subwww.code")"
check_contains "www falls back to the main home" "$TMP/subwww.html" 'Main Post'
curl -sS -o "$TMP/subno.html" -w "%{http_code}" -H "Host: nosuch.example.com" "$BASE/?x=1" > "$TMP/subno.code"
check "GET / with unknown subdomain" 200 "$(cat "$TMP/subno.code")"
check_contains "unknown subdomain falls back to main" "$TMP/subno.html" 'Main Post'

# --- archived sites: public 404, API still administers ----------------
curl -sS -o "$TMP/arch.json" -w "%{http_code}" "${AUTH[@]}" -X PUT "$BASE/api/sites/2" \
  -H 'Content-Type: application/json' -d '{"status":"archived"}' > "$TMP/arch.code"
check "PUT site 2 archived" 200 "$(cat "$TMP/arch.code")"
curl -sS -o /dev/null -w "%{http_code}" "$BASE/blog/?x=2" > "$TMP/arch-home.code"
check "archived site public home is 404" 404 "$(cat "$TMP/arch-home.code")"
curl -sS -o "$TMP/arch-posts.json" -w "%{http_code}" "${AUTH[@]}" "${SITE2[@]}" "$BASE/api/posts" > "$TMP/arch-posts.code"
check "archived site API still lists posts" 200 "$(cat "$TMP/arch-posts.code")"
check_contains "archived site API sees the blog post" "$TMP/arch-posts.json" 'blog-post'
curl -sS -o /dev/null -w "%{http_code}" "${AUTH[@]}" -X PUT "$BASE/api/sites/2" \
  -H 'Content-Type: application/json' -d '{"status":"public"}' > "$TMP/unarch.code"
check "PUT site 2 public again" 200 "$(cat "$TMP/unarch.code")"
curl -sS -o /dev/null -w "%{http_code}" "$BASE/blog/?x=3" > "$TMP/unarch-home.code"
check "unarchived blog home is 200 again" 200 "$(cat "$TMP/unarch-home.code")"

# --- delete: site 1 refused, site 2 wiped ----------------------------
curl -sS -o /dev/null -w "%{http_code}" "${AUTH[@]}" -X DELETE "$BASE/api/sites/1" > "$TMP/del1.code"
check "DELETE site 1 refused" 400 "$(cat "$TMP/del1.code")"
curl -sS -o "$TMP/del2.json" -w "%{http_code}" "${AUTH[@]}" -X DELETE "$BASE/api/sites/2" > "$TMP/del2.code"
check "DELETE site 2" 200 "$(cat "$TMP/del2.code")"
check "deleted post count is 1" 1 "$(jget "$TMP/del2.json" 'd.data.deletedPosts')"
curl -sS -o "$TMP/sites-after.json" -w "%{http_code}" "${AUTH[@]}" "$BASE/api/sites" > "$TMP/sites-after.code"
check "one site left after delete" 1 "$(jget "$TMP/sites-after.json" 'd.data.length')"
curl -sS -o /dev/null -w "%{http_code}" "$BASE/blog/blog-post?x=9" > "$TMP/after-del.code"
check "deleted site's post path is 404" 404 "$(cat "$TMP/after-del.code")"
curl -sS -o /dev/null -w "%{http_code}" "${AUTH[@]}" "$BASE/api/sites/2" > "$TMP/after-del2.code"
check "deleted site is gone from the API" 404 "$(cat "$TMP/after-del2.code")"

# --- the admin SPA ships the sites route -------------------------------
if grep -q '#/sites' admin/dist/admin/assets/*.js 2>/dev/null; then
  PASS=$((PASS + 1)); echo "  ok  admin bundle contains the #/sites route"
else
  FAIL=$((FAIL + 1)); echo "  FAIL admin bundle does not contain the #/sites route (run bash scripts/build-admin.sh)"
fi

echo
echo "== P12 smoke: $PASS passed, $FAIL failed =="
rm -rf "$TMP"
[ "$FAIL" -eq 0 ]
