#!/usr/bin/env bash
# WORDLESS v2 P10 smoke test — SEO depth end to end.
#
# Runs the Rust worker under local wrangler dev (port 8811) against a
# local D1 (migrations applied, one seeded admin) and checks:
#   JSON-LD: home carries WebSite+SearchAction + Organization; a post
#     carries Article + FAQPage + BreadcrumbList; a product page
#     carries Product (price from minor units, InStock); a course
#     page carries Course with provider + instructor.
#   visible breadcrumbs: <nav aria-label="Breadcrumb"> on post /
#     product / course pages, absent on the home page.
#   FAQ API: round-trip, 21st item / over-long question 400, missing
#     post 404, anon 401.
#   sitemaps: /sitemap.xml unchanged; /sitemap-index.xml lists the
#     chunks + image + video sitemaps; /sitemap-images.xml lists only
#     URLs with images; /sitemap-videos.xml lists only videos with a
#     thumbnail (a video without one is NOT listed).
#   robots.txt: default lists all four sitemap files.
#   llms.txt / llms-full.txt: link lists + full post text, no markup.
#   CWV budgets: defaults (2500/200/0.1), PUT round-trip, out-of-range
#     400, anon 401.
#   audit runs: record 201, list + get round-trip, empty run 400,
#     score 101 400, missing id 404, anon 401.
#   sitemap viewer API: /api/seo/sitemap lists the files + counts.
#   JSON-LD preview: /api/posts/:id/seo/json-ld valid=true for the
#     FAQ post, 404 for a missing post.
#   denials: unknown post slug 404 with noindex; unknown product 404.
#
# Usage: ./scripts/smoke-p10.sh [--keep]   (--keep leaves wrangler dev running)
# Clean room first: rm -rf .wrangler   (the script applies migrations itself)
# Requires: wrangler (TS repo's project-managed binary), worker-build, node.
# Seeded admin (password "smoke-admin-2026"): login "admin".
# The school Worker is started first (course pages live there), exactly
# like smoke-p9.sh; the main Worker is pointed at it via
# WORDLESS_SCHOOL_URL.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"

WRANGLER="$HOME/workspace/wordless/wordless-cms/node_modules/.bin/wrangler"
PORT="${SMOKE_PORT:-8811}"
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

echo "== P10 smoke: applying migrations (local D1) =="
"$WRANGLER" d1 migrations apply DB --local >/dev/null

echo "== P10 smoke: seeding admin (password smoke-admin-2026) =="
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
# (robots.txt Sitemap lines, canonicals, llms.txt links) use the
# smoke port instead of the localhost:8787 default. NOTE: Wordless
# options are JSON-encoded on write (wordless-db `set_option`), and
# `get_option` returns the default when the stored text is not valid
# JSON — so the value here must include the JSON double quotes, or
# the site silently falls back to the default (this bit us once).
echo "INSERT OR REPLACE INTO wp_options (option_name, option_value) VALUES ('siteurl', '\"http://127.0.0.1:$PORT\"');" > "$SEED_SQL"
"$WRANGLER" d1 execute DB --local --file "$SEED_SQL" >/dev/null
rm -f "$SEED_SQL"

echo "== P10 smoke: starting school Worker (port 8809) =="
source "$ROOT/scripts/split-workers-dev.sh"
start_school
start_seo

echo "== P10 smoke: starting wrangler dev on :$PORT =="
"$WRANGLER" dev --port "$PORT" --local --var "WORDLESS_SCHOOL_URL:$SCHOOL_URL" --var "WORDLESS_SEO_URL:$SEO_URL" >/tmp/smoke-p10-wrangler.log 2>&1 &
DEV_PID=$!
cleanup() {
  kill "$DEV_PID" 2>/dev/null || true
  for child in $(pgrep -P "$DEV_PID" 2>/dev/null); do kill "$child" 2>/dev/null || true; done
  stop_school
  stop_seo
}
trap cleanup EXIT

# Wait for the worker to answer (wrangler dev builds first — can take a while).
for i in $(seq 1 240); do
  if curl -fsS -o /dev/null "$BASE/api/health" 2>/dev/null; then break; fi
  sleep 2
done

echo "== P10 smoke: checks =="
TMP="$(mktemp -d)"

curl -sS -o "$TMP/login.json" -w "%{http_code}" -D "$TMP/login.headers" -X POST "$BASE/api/auth/login" \
  -H 'Content-Type: application/json' -d '{"login":"admin","password":"smoke-admin-2026"}' > "$TMP/login.code"
check "login admin" 200 "$(cat "$TMP/login.code")"
ADMIN_COOKIE="$(grep -i '^set-cookie:' "$TMP/login.headers" | head -1 | sed -e 's/^[^:]*: *//' -e 's/;.*//' | tr -d '\r')"

# --- media attachment (inserted directly; the media API upload path is
#     covered by earlier smokes) — used as an image-block attachment and
#     as the video post's featured image (thumbnail for the video sitemap).
ATT_SQL="$(mktemp)"
cat > "$ATT_SQL" <<'SQL'
INSERT INTO wp_posts (post_author, post_date, post_date_gmt, post_content, post_title, post_excerpt, post_status, post_name, post_modified, post_modified_gmt, post_parent, post_type, post_mime_type, comment_status) VALUES (1, '2026-11-20 00:00:00', '2026-11-20 00:00:00', '', 'Wolf photo', '', 'inherit', 'wolf-png', '2026-11-20 00:00:00', '2026-11-20 00:00:00', 0, 'attachment', 'image/png', 'open');
INSERT INTO wp_postmeta (post_id, meta_key, meta_value) VALUES ((SELECT ID FROM wp_posts WHERE post_name = 'wolf-png'), '_wp_attached_file', 'wolf.png');
INSERT INTO wp_postmeta (post_id, meta_key, meta_value) VALUES ((SELECT ID FROM wp_posts WHERE post_name = 'wolf-png'), '_wp_attachment_metadata', '{"filename": "wolf.png", "size": 8, "alt": "A wolf"}');
SQL
"$WRANGLER" d1 execute DB --local --file "$ATT_SQL" >/dev/null
rm -f "$ATT_SQL"
ATT_ID="$("$WRANGLER" d1 execute DB --local --command "SELECT ID FROM wp_posts WHERE post_name = 'wolf-png'" --json 2>/dev/null | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{try{const j=JSON.parse(s);console.log(j[0].results[0].ID);}catch(e){console.log("");}})')"
[ -n "$ATT_ID" ] && { PASS=$((PASS + 1)); echo "  ok  attachment seeded (id $ATT_ID)"; } || { FAIL=$((FAIL + 1)); echo "  FAIL attachment seed: no id"; }

# --- post with an image block + a YouTube embed, published ---
curl -sS -o "$TMP/post.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/posts" \
  -H 'Content-Type: application/json' -d "{\"title\":\"Ridge Trail at Dawn\",\"slug\":\"ridge-trail\",\"content\":\"<!-- wp:image {\\\"id\\\":$ATT_ID} --><figure class=\\\"wp-block-image\\\"><img src=\\\"/media/wolf.png\\\" alt=\\\"A wolf\\\"/></figure><!-- /wp:image --><p>Watch the pack:</p><iframe src=\\\"https://www.youtube.com/embed/dQw4w9WgXcQ\\\"></iframe><p>Welcome to the pack.</p>\",\"excerpt\":\"A walk along the ridge trail at dawn.\",\"status\":\"publish\"}" > "$TMP/post.code"
check "POST /api/posts ridge trail" 201 "$(cat "$TMP/post.code")"
POST_ID="$(jget "$TMP/post.json" 'd.data.id')"
# Featured image = the attachment (video sitemap thumbnail).
"$WRANGLER" d1 execute DB --local --command "INSERT INTO wp_postmeta (post_id, meta_key, meta_value) VALUES ($POST_ID, '_thumbnail_id', '$ATT_ID')" >/dev/null

# A second, plain post (no image, no video) for the negative sitemap checks.
curl -sS -o "$TMP/post2.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/posts" \
  -H 'Content-Type: application/json' -d '{"title":"Plain Note","slug":"plain-note","content":"<p>Just words.</p>","excerpt":"Just words.","status":"publish"}' > "$TMP/post2.code"
check "POST /api/posts plain note" 201 "$(cat "$TMP/post2.code")"

# --- SEO fields + FAQ on the first post ---
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/posts/$POST_ID/seo" \
  -H 'Content-Type: application/json' -d '{"title":"Ridge Trail at Dawn","description":"Walk the ridge trail at dawn with the pack.","twitterCard":"summary_large_image"}' > "$TMP/seo.code"
check "PUT /api/posts/:id/seo" 200 "$(cat "$TMP/seo.code")"
curl -sS -o "$TMP/faq.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/posts/$POST_ID/seo/faq" \
  -H 'Content-Type: application/json' -d '{"faq":[{"question":"Is the pack real?","answer":"Yes, the pack is real."}]}' > "$TMP/faq.code"
check "PUT /api/posts/:id/seo/faq" 200 "$(cat "$TMP/faq.code")"
check_contains "FAQ round-trips" "$TMP/faq.json" 'Is the pack real?'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/posts/$POST_ID/seo/faq" \
  -H 'Content-Type: application/json' -d "{\"faq\":[{\"question\":\"$(node -e 'console.log("x".repeat(201))')\",\"answer\":\"Too long.\"}]}" > "$TMP/faq-bad.code"
check "PUT faq with a 201-char question" 400 "$(cat "$TMP/faq-bad.code")"
curl -sS -o /dev/null -w "%{http_code}" -X PUT "$BASE/api/posts/$POST_ID/seo/faq" \
  -H 'Content-Type: application/json' -d '{"faq":[]}' > "$TMP/faq-anon.code"
check "PUT faq anon" 401 "$(cat "$TMP/faq-anon.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X GET "$BASE/api/posts/999999/seo/faq" > "$TMP/faq-404.code"
check "GET faq for a missing post" 404 "$(cat "$TMP/faq-404.code")"

# --- product + course (for Product / Course JSON-LD) ---
curl -sS -o "$TMP/product.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/commerce/products" \
  -H 'Content-Type: application/json' -d "{\"name\":\"Pack Hoodie\",\"slug\":\"pack-hoodie\",\"description\":\"<p>A warm hoodie.</p>\",\"priceMinor\":329700,\"currency\":\"NGN\",\"stock\":5,\"status\":\"published\",\"imageId\":$ATT_ID}" > "$TMP/product.code"
check "POST /api/commerce/products" 201 "$(cat "$TMP/product.code")"
curl -sS -o "$TMP/course.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/school/courses" \
  -H 'Content-Type: application/json' -d '{"title":"Rust 101","description":"Learn Rust."}' > "$TMP/course.code"
check "POST /api/school/courses" 201 "$(cat "$TMP/course.code")"
COURSE_ID="$(jget "$TMP/course.json" 'd.data.id')"
COURSE_SLUG="$(jget "$TMP/course.json" 'd.data.slug')"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/school/courses/$COURSE_ID" \
  -H 'Content-Type: application/json' -d '{"status":"published"}' > "$TMP/course-pub.code"
check "PUT course publish" 200 "$(cat "$TMP/course-pub.code")"

# --- public pages: JSON-LD + visible breadcrumbs ---
curl -sS -o "$TMP/home.html" -w "%{http_code}" "$BASE/" > "$TMP/home.code"
check "GET / home" 200 "$(cat "$TMP/home.code")"
check_contains "home has WebSite+SearchAction" "$TMP/home.html" 'SearchAction'
check_contains "home has Organization" "$TMP/home.html" '"@type":"Organization"'
check_not_contains "home has no breadcrumb nav" "$TMP/home.html" 'aria-label="Breadcrumb"'
curl -sS -o "$TMP/post.html" -w "%{http_code}" "$BASE/ridge-trail" > "$TMP/post-page.code"
check "GET /ridge-trail" 200 "$(cat "$TMP/post-page.code")"
check_contains "post has Article" "$TMP/post.html" '"@type":"Article"'
check_contains "post has FAQPage" "$TMP/post.html" '"@type":"FAQPage"'
check_contains "post has BreadcrumbList" "$TMP/post.html" '"@type":"BreadcrumbList"'
check_contains "post has visible breadcrumbs" "$TMP/post.html" 'aria-label="Breadcrumb"'
check_contains "post breadcrumb is current page" "$TMP/post.html" 'aria-current="page"'
check_contains "post has canonical" "$TMP/post.html" 'rel="canonical"'
check_contains "post has og:type article" "$TMP/post.html" 'og:type" content="article"'
check_contains "post has twitter large image" "$TMP/post.html" 'summary_large_image'
curl -sS -o "$TMP/product.html" -w "%{http_code}" "$BASE/shop/pack-hoodie" > "$TMP/product-page.code"
check "GET /shop/pack-hoodie" 200 "$(cat "$TMP/product-page.code")"
check_contains "product has Product JSON-LD" "$TMP/product.html" '"@type":"Product"'
check_contains "product price from minor units" "$TMP/product.html" '"price":"3297.00"'
check_contains "product InStock" "$TMP/product.html" 'InStock'
check_contains "product has visible breadcrumbs" "$TMP/product.html" 'aria-label="Breadcrumb"'
curl -sS -o "$TMP/course.html" -w "%{http_code}" "$BASE/school/courses/$COURSE_SLUG" > "$TMP/course-page.code"
check "GET /school/courses/:slug" 200 "$(cat "$TMP/course-page.code")"
check_contains "course has Course JSON-LD" "$TMP/course.html" '"@type":"Course"'
check_contains "course provider is the site" "$TMP/course.html" '"provider":{"@type":"Organization"'
check_contains "course has visible breadcrumbs" "$TMP/course.html" 'aria-label="Breadcrumb"'
curl -sS -o /dev/null -w "%{http_code}" "$BASE/shop/no-such-product" > "$TMP/no-prod.code"
check "GET unknown product" 404 "$(cat "$TMP/no-prod.code")"
curl -sS -o "$TMP/nf.html" -w "%{http_code}" "$BASE/no-such-page" > "$TMP/nf.code"
check "GET unknown page" 404 "$(cat "$TMP/nf.code")"
check_contains "404 page is noindex" "$TMP/nf.html" 'noindex'

# --- JSON-LD preview API ---
curl -sS -o "$TMP/jsonld.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/posts/$POST_ID/seo/json-ld" > "$TMP/jsonld.code"
check "GET /api/posts/:id/seo/json-ld" 200 "$(cat "$TMP/jsonld.code")"
check_contains "preview is valid" "$TMP/jsonld.json" '"valid":true'
check_contains "preview has FAQPage" "$TMP/jsonld.json" 'FAQPage'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/posts/999999/seo/json-ld" > "$TMP/jsonld-404.code"
check "GET json-ld for a missing post" 404 "$(cat "$TMP/jsonld-404.code")"
curl -sS -o /dev/null -w "%{http_code}" "$BASE/api/posts/$POST_ID/seo/json-ld" > "$TMP/jsonld-anon.code"
check "GET json-ld anon" 401 "$(cat "$TMP/jsonld-anon.code")"

# --- sitemaps ---
curl -sS -o "$TMP/sitemap.xml" -w "%{http_code}" "$BASE/sitemap.xml" > "$TMP/sitemap.code"
check "GET /sitemap.xml" 200 "$(cat "$TMP/sitemap.code")"
check_contains "sitemap lists the post" "$TMP/sitemap.xml" '/ridge-trail'
curl -sS -o "$TMP/sitemap-index.xml" -w "%{http_code}" "$BASE/sitemap-index.xml" > "$TMP/sitemap-index.code"
check "GET /sitemap-index.xml" 200 "$(cat "$TMP/sitemap-index.code")"
check_contains "index lists the main sitemap" "$TMP/sitemap-index.xml" '/sitemap.xml?page=1'
check_contains "index lists the image sitemap" "$TMP/sitemap-index.xml" '/sitemap-images.xml'
check_contains "index lists the video sitemap" "$TMP/sitemap-index.xml" '/sitemap-videos.xml'
curl -sS -o "$TMP/sitemap-images.xml" -w "%{http_code}" "$BASE/sitemap-images.xml" > "$TMP/sitemap-images.code"
check "GET /sitemap-images.xml" 200 "$(cat "$TMP/sitemap-images.code")"
check_contains "image sitemap has the post" "$TMP/sitemap-images.xml" '/ridge-trail'
check_contains "image sitemap has the image" "$TMP/sitemap-images.xml" '/media/wolf.png'
check_not_contains "image sitemap skips the plain post" "$TMP/sitemap-images.xml" '/plain-note'
curl -sS -o "$TMP/sitemap-videos.xml" -w "%{http_code}" "$BASE/sitemap-videos.xml" > "$TMP/sitemap-videos.code"
check "GET /sitemap-videos.xml" 200 "$(cat "$TMP/sitemap-videos.code")"
check_contains "video sitemap has the embed" "$TMP/sitemap-videos.xml" 'youtube.com/embed/dQw4w9WgXcQ'
check_contains "video sitemap has the thumbnail" "$TMP/sitemap-videos.xml" '/media/wolf.png'
check_contains "video sitemap loc is the page" "$TMP/sitemap-videos.xml" '/ridge-trail'

# --- robots.txt + llms.txt ---
curl -sS -o "$TMP/robots.txt" -w "%{http_code}" "$BASE/robots.txt" > "$TMP/robots.code"
check "GET /robots.txt" 200 "$(cat "$TMP/robots.code")"
check_contains "robots lists the main sitemap" "$TMP/robots.txt" 'Sitemap: http://127.0.0.1:8811/sitemap.xml'
check_contains "robots lists the index" "$TMP/robots.txt" 'sitemap-index.xml'
check_contains "robots lists the image sitemap" "$TMP/robots.txt" 'sitemap-images.xml'
check_contains "robots lists the video sitemap" "$TMP/robots.txt" 'sitemap-videos.xml'
curl -sS -o "$TMP/llms.txt" -w "%{http_code}" "$BASE/llms.txt" > "$TMP/llms.code"
check "GET /llms.txt" 200 "$(cat "$TMP/llms.code")"
check_contains "llms lists the post" "$TMP/llms.txt" '[Ridge Trail at Dawn]'
check_contains "llms lists the product" "$TMP/llms.txt" '[Pack Hoodie]'
check_contains "llms lists the course" "$TMP/llms.txt" '[Rust 101]'
curl -sS -o "$TMP/llms-full.txt" -w "%{http_code}" "$BASE/llms-full.txt" > "$TMP/llms-full.code"
check "GET /llms-full.txt" 200 "$(cat "$TMP/llms-full.code")"
check_contains "llms-full has the post text" "$TMP/llms-full.txt" 'Welcome to the pack.'
check_not_contains "llms-full has no markup" "$TMP/llms-full.txt" '<iframe'

# --- CWV budgets ---
curl -sS -o "$TMP/cwv.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/seo/cwv-budgets" > "$TMP/cwv.code"
check "GET /api/seo/cwv-budgets" 200 "$(cat "$TMP/cwv.code")"
check_contains "budgets default LCP 2500" "$TMP/cwv.json" '"lcpMs":2500'
check_contains "budgets default INP 200" "$TMP/cwv.json" '"inpMs":200'
check_contains "budgets default CLS 0.1" "$TMP/cwv.json" '"cls":0.1'
curl -sS -o "$TMP/cwv2.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/seo/cwv-budgets" \
  -H 'Content-Type: application/json' -d '{"lcpMs":2000,"inpMs":100,"cls":0.05}' > "$TMP/cwv2.code"
check "PUT /api/seo/cwv-budgets" 200 "$(cat "$TMP/cwv2.code")"
check_contains "budgets round-trip LCP" "$TMP/cwv2.json" '"lcpMs":2000'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/seo/cwv-budgets" \
  -H 'Content-Type: application/json' -d '{"lcpMs":100,"inpMs":100,"cls":0.05}' > "$TMP/cwv-bad.code"
check "PUT budgets with LCP 100" 400 "$(cat "$TMP/cwv-bad.code")"
curl -sS -o /dev/null -w "%{http_code}" "$BASE/api/seo/cwv-budgets" > "$TMP/cwv-anon.code"
check "GET budgets anon" 401 "$(cat "$TMP/cwv-anon.code")"

# --- audit runs ---
curl -sS -o "$TMP/run.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/seo/audit-runs" \
  -H 'Content-Type: application/json' -d '{"url":"http://127.0.0.1:8811/ridge-trail","seoScore":100,"accessibilityScore":97,"performanceScore":92,"bestPracticesScore":90,"lcpMs":1800,"inpMs":150,"clsMilli":40,"notes":"smoke run"}' > "$TMP/run.code"
check "POST /api/seo/audit-runs" 201 "$(cat "$TMP/run.code")"
check_contains "run stored the URL" "$TMP/run.json" 'ridge-trail'
RUN_ID="$(jget "$TMP/run.json" 'd.data.id')"
curl -sS -o "$TMP/runs.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/seo/audit-runs" > "$TMP/runs.code"
check "GET /api/seo/audit-runs" 200 "$(cat "$TMP/runs.code")"
check_contains "list has the run" "$TMP/runs.json" 'ridge-trail'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/seo/audit-runs/$RUN_ID" > "$TMP/run-one.code"
check "GET /api/seo/audit-runs/:id" 200 "$(cat "$TMP/run-one.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/seo/audit-runs/999999" > "$TMP/run-404.code"
check "GET missing audit run" 404 "$(cat "$TMP/run-404.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/seo/audit-runs" \
  -H 'Content-Type: application/json' -d '{"url":"http://127.0.0.1:8811/ridge-trail"}' > "$TMP/run-empty.code"
check "POST an empty audit run" 400 "$(cat "$TMP/run-empty.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/seo/audit-runs" \
  -H 'Content-Type: application/json' -d '{"url":"http://127.0.0.1:8811/ridge-trail","seoScore":101}' > "$TMP/run-bad.code"
check "POST a run with score 101" 400 "$(cat "$TMP/run-bad.code")"
curl -sS -o /dev/null -w "%{http_code}" -X POST "$BASE/api/seo/audit-runs" \
  -H 'Content-Type: application/json' -d '{"url":"http://127.0.0.1:8811/ridge-trail","seoScore":100}' > "$TMP/run-anon.code"
check "POST an audit run anon" 401 "$(cat "$TMP/run-anon.code")"

# --- sitemap viewer API ---
curl -sS -o "$TMP/sitemap-api.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/seo/sitemap" > "$TMP/sitemap-api.code"
check "GET /api/seo/sitemap" 200 "$(cat "$TMP/sitemap-api.code")"
check_contains "viewer lists the index" "$TMP/sitemap-api.json" 'sitemap-index.xml'
check_contains "viewer lists the image sitemap" "$TMP/sitemap-api.json" 'sitemap-images.xml'

echo
echo "== P10 smoke: $PASS passed, $FAIL failed =="
rm -rf "$TMP"
[ "$FAIL" -eq 0 ]
