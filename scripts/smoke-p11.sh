#!/usr/bin/env bash
# WORDLESS v2 P11 smoke test — Visual Builder end to end.
#
# Runs the Rust worker under local wrangler dev (port 8814) against a
# local D1 (migrations applied, one seeded admin) and checks:
#   palette: full libraries present (10 core widgets, 26 elements,
#     104 icons, 3 devices 1440/768/360), extension widgets absent
#     until their extension is installed AND loaded, present when
#     loaded, gone again when the instance is disabled (kill switch).
#   preview: layout renders through the P3 renderer (heading text in
#     the HTML), device widths echoed, unknown device 400, unknown
#     widget type listed as unsupported (and not rendered), bad
#     structure 400, anon 401.
#   layout CRUD: PUT /api/posts/:id/meta stores _elementor_data, GET
#     reads it back, the public page renders the saved layout.
#   extension-widget registry: register 201, bad icon 400, unknown
#     extension 404, bad widget type 400, delete 200 then 404,
#     anon 401 on every write.
#   access denials: meta for a missing post 404, anon 401.
#   undo/redo: the admin history module passes its Node tests
#     (3/3) — the state behind the Undo/Redo buttons.
#   admin SPA: the built bundle ships the #/builder route.
#
# The converter Worker is started first (preview + public render go
# through it), exactly like the conversion smokes.
#
# Gating honesty: the "loaded" instance row is seeded directly into
# wl_extension_instances (the loader itself is covered by
# smoke-p6.sh). What this script proves is the palette's gating
# rule — it reads instance status, not marketplace status.
#
# Usage: ./scripts/smoke-p11.sh [--keep]   (--keep leaves wrangler dev running)
# Clean room first: rm -rf .wrangler   (the script applies migrations itself)
# Requires: wrangler (TS repo's project-managed binary), worker-build, node.
# Seeded admin (password "smoke-admin-2026"): login "admin".
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"

WRANGLER="$HOME/workspace/wordless/wordless-cms/node_modules/.bin/wrangler"
PORT="${SMOKE_PORT:-8814}"
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

echo "== P11 smoke: applying migrations (local D1) =="
"$WRANGLER" d1 migrations apply DB --local >/dev/null

echo "== P11 smoke: seeding admin (password smoke-admin-2026) =="
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

# --- the smoke extension: installed + one registered widget, but NO
#     loaded instance yet, so the palette must NOT show its widget.
EXT_SQL="$(mktemp)"
cat > "$EXT_SQL" <<'SQL'
INSERT INTO wl_extensions (id, slug, name, kind, version, status, created_at, updated_at)
VALUES ('smoke-ext', 'smoke-ext', 'Smoke Extension', 'plugin', '0.1.0', 'active', '2026-11-20 00:00:00', '2026-11-20 00:00:00');
INSERT INTO wl_builder_widgets (extension_slug, widget_type, label, icon, description, settings_schema_json, created_at, updated_at)
VALUES ('smoke-ext', 'smoke-hero', 'Smoke Hero', 'star', 'A hero block from the smoke extension.', '[]', '2026-11-20 00:00:00', '2026-11-20 00:00:00');
SQL
"$WRANGLER" d1 execute DB --local --file "$EXT_SQL" >/dev/null
rm -f "$EXT_SQL"

echo "== P11 smoke: starting converter Worker (port 8805) =="
source "$ROOT/scripts/converter-dev.sh"
start_converter

echo "== P11 smoke: starting wrangler dev on :$PORT =="
"$WRANGLER" dev --port "$PORT" --local --var "WORDLESS_CONVERTER_URL:$CONVERTER_URL" >/tmp/smoke-p11-wrangler.log 2>&1 &
DEV_PID=$!
cleanup() {
  kill "$DEV_PID" 2>/dev/null || true
  for child in $(pgrep -P "$DEV_PID" 2>/dev/null); do kill "$child" 2>/dev/null || true; done
  stop_converter
}
trap cleanup EXIT

# Wait for the worker to answer (wrangler dev builds first — can take a while).
for i in $(seq 1 240); do
  if curl -fsS -o /dev/null "$BASE/api/health" 2>/dev/null; then break; fi
  sleep 2
done

echo "== P11 smoke: checks =="
TMP="$(mktemp -d)"

curl -sS -o "$TMP/login.json" -w "%{http_code}" -D "$TMP/login.headers" -X POST "$BASE/api/auth/login" \
  -H 'Content-Type: application/json' -d '{"login":"admin","password":"smoke-admin-2026"}' > "$TMP/login.code"
check "login admin" 200 "$(cat "$TMP/login.code")"
ADMIN_COOKIE="$(grep -i '^set-cookie:' "$TMP/login.headers" | head -1 | sed -e 's/^[^:]*: *//' -e 's/;.*//' | tr -d '\r')"

# --- the test layout: heading + button + wordless-card in one column.
node - "$TMP/layout.json" <<'NODE'
const layout = [
  { id: "s1", elType: "section", elements: [
    { id: "c1", elType: "column", elements: [
      { id: "w1", elType: "widget", widgetType: "heading", settings: { title: "Pack Landing", header_size: "h1", align: "center" } },
      { id: "w2", elType: "widget", widgetType: "button", settings: { text: "Join the pack", link: { url: "https://example.com/join", is_external: true }, align: "center" } },
      { id: "w3", elType: "widget", widgetType: "wordless-card", settings: { title: "Fast by design", text: "Built in Rust, served from the edge." } },
      { id: "w4", elType: "widget", widgetType: "wordless-nope", settings: { title: "Nowhere Widget" } }
    ] }
  ] }
];
require("fs").writeFileSync(process.argv[2], JSON.stringify(layout)); // argv: [node, "-", outPath]
NODE

# --- palette: the full libraries -------------------------------------
curl -sS -o "$TMP/palette.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/builder/palette" > "$TMP/palette.code"
check "GET /api/builder/palette" 200 "$(cat "$TMP/palette.code")"
check "palette has 10 core widgets" 10 "$(jget "$TMP/palette.json" 'd.data.widgets.length')"
check "palette has 26 elements" 26 "$(jget "$TMP/palette.json" 'd.data.elements.length')"
check "palette has 104 icons" 104 "$(jget "$TMP/palette.json" 'd.data.icons.length')"
check "palette has 3 devices" 3 "$(jget "$TMP/palette.json" 'd.data.devices.length')"
check "phone device is 360 wide" 360 "$(jget "$TMP/palette.json" 'd.data.devices.find(d=>d.id==="phone").width')"
check "tablet device is 768 wide" 768 "$(jget "$TMP/palette.json" 'd.data.devices.find(d=>d.id==="tablet").width')"
check "desktop device is 1440 wide" 1440 "$(jget "$TMP/palette.json" 'd.data.devices.find(d=>d.id==="desktop").width')"
check_contains "palette includes the button widget" "$TMP/palette.json" '"type":"button"'
check_contains "palette includes the wordless-card element" "$TMP/palette.json" '"type":"wordless-card"'
check_contains "palette includes the home icon" "$TMP/palette.json" '"name":"home"'
check "extension widgets absent while extension not loaded" 0 "$(jget "$TMP/palette.json" 'd.data.extensionWidgets.length')"
curl -sS -o /dev/null -w "%{http_code}" "$BASE/api/builder/palette" > "$TMP/palette-anon.code"
check "GET /api/builder/palette anon" 401 "$(cat "$TMP/palette-anon.code")"

# --- preview: renders through the P3 renderer ------------------------
node -e 'const fs=require("fs");const layout=JSON.parse(fs.readFileSync(process.argv[1],"utf8"));fs.writeFileSync(process.argv[2],JSON.stringify({data:layout,device:"desktop"}));' "$TMP/layout.json" "$TMP/preview-body.json"
curl -sS -o "$TMP/preview.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/builder/preview" \
  -H 'Content-Type: application/json' --data @"$TMP/preview-body.json" > "$TMP/preview.code"
check "POST /api/builder/preview desktop" 200 "$(cat "$TMP/preview.code")"
check_contains "preview HTML has the heading" "$TMP/preview.json" 'Pack Landing'
check_contains "preview HTML has the button" "$TMP/preview.json" 'Join the pack'
check_contains "preview HTML has the card" "$TMP/preview.json" 'Fast by design'
check "preview reports desktop width" 1440 "$(jget "$TMP/preview.json" 'd.data.deviceWidth')"
check_contains "preview lists the unknown widget as unsupported" "$TMP/preview.json" 'wordless-nope'
check_not_contains "preview does not render the unknown widget" "$TMP/preview.json" 'Nowhere Widget'

node -e 'const fs=require("fs");const layout=JSON.parse(fs.readFileSync(process.argv[1],"utf8"));fs.writeFileSync(process.argv[2],JSON.stringify({data:layout,device:"phone"}));' "$TMP/layout.json" "$TMP/preview-phone.json"
curl -sS -o "$TMP/preview-p.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/builder/preview" \
  -H 'Content-Type: application/json' --data @"$TMP/preview-phone.json" > "$TMP/preview-p.code"
check "POST /api/builder/preview phone" 200 "$(cat "$TMP/preview-p.code")"
check "preview reports phone width" 360 "$(jget "$TMP/preview-p.json" 'd.data.deviceWidth')"

node -e 'const fs=require("fs");const layout=JSON.parse(fs.readFileSync(process.argv[1],"utf8"));fs.writeFileSync(process.argv[2],JSON.stringify({data:layout,device:"watch"}));' "$TMP/layout.json" "$TMP/preview-bad-device.json"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/builder/preview" \
  -H 'Content-Type: application/json' --data @"$TMP/preview-bad-device.json" > "$TMP/preview-bad-device.code"
check "preview with an unknown device" 400 "$(cat "$TMP/preview-bad-device.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/builder/preview" \
  -H 'Content-Type: application/json' -d '{"data":{"not":"a layout"}}' > "$TMP/preview-bad.code"
check "preview with a non-array layout" 400 "$(cat "$TMP/preview-bad.code")"
curl -sS -o /dev/null -w "%{http_code}" -X POST "$BASE/api/builder/preview" \
  -H 'Content-Type: application/json' --data @"$TMP/preview-body.json" > "$TMP/preview-anon.code"
check "preview anon" 401 "$(cat "$TMP/preview-anon.code")"

# --- layout CRUD + public render --------------------------------------
curl -sS -o "$TMP/page.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/posts" \
  -H 'Content-Type: application/json' -d '{"title":"Pack Landing","slug":"pack-landing","content":"","excerpt":"","status":"publish","type":"page"}' > "$TMP/page.code"
check "POST /api/posts pack landing page" 201 "$(cat "$TMP/page.code")"
PAGE_ID="$(jget "$TMP/page.json" 'd.data.id')"

node -e 'const fs=require("fs");const layout=JSON.parse(fs.readFileSync(process.argv[1],"utf8"));fs.writeFileSync(process.argv[2],JSON.stringify({_elementor_data:JSON.stringify(layout)}));' "$TMP/layout.json" "$TMP/meta-body.json"
curl -sS -o "$TMP/meta.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/posts/$PAGE_ID/meta" \
  -H 'Content-Type: application/json' --data @"$TMP/meta-body.json" > "$TMP/meta.code"
check "PUT /api/posts/:id/meta stores the layout" 200 "$(cat "$TMP/meta.code")"
curl -sS -o "$TMP/meta-get.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/posts/$PAGE_ID/meta" > "$TMP/meta-get.code"
check "GET /api/posts/:id/meta" 200 "$(cat "$TMP/meta-get.code")"
check_contains "meta read-back has the layout" "$TMP/meta-get.json" 'Pack Landing'
curl -sS -o /dev/null -w "%{http_code}" "$BASE/api/posts/$PAGE_ID/meta" > "$TMP/meta-anon.code"
check "GET meta anon" 401 "$(cat "$TMP/meta-anon.code")"
curl -sS -o /dev/null -w "%{http_code}" -X PUT "$BASE/api/posts/$PAGE_ID/meta" \
  -H 'Content-Type: application/json' --data @"$TMP/meta-body.json" > "$TMP/meta-put-anon.code"
check "PUT meta anon" 401 "$(cat "$TMP/meta-put-anon.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/posts/999999/meta" > "$TMP/meta-404.code"
check "GET meta for a missing post" 404 "$(cat "$TMP/meta-404.code")"

curl -sS -o "$TMP/public.html" -w "%{http_code}" "$BASE/pack-landing" > "$TMP/public.code"
check "public page renders" 200 "$(cat "$TMP/public.code")"
check_contains "public page has the heading" "$TMP/public.html" 'Pack Landing'
check_contains "public page has the button" "$TMP/public.html" 'Join the pack'
check_not_contains "public page does not render the unknown widget" "$TMP/public.html" 'Nowhere Widget'

# --- extension-widget gating: load the instance, then disable it ------
"$WRANGLER" d1 execute DB --local --command "INSERT INTO wl_extension_instances (slug, version, status, permissions_json, script_key, host_token_hash, token_epoch, dev_url, created_at, updated_at) VALUES ('smoke-ext', '0.1.0', 'loaded', '[]', 'smoke-key', 'smoke-hash', 0, NULL, '2026-11-20 00:00:00', '2026-11-20 00:00:00')" >/dev/null
curl -sS -o "$TMP/palette2.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/builder/palette" > "$TMP/palette2.code"
check "palette after loading the extension" 200 "$(cat "$TMP/palette2.code")"
check "extension widget appears once loaded" 1 "$(jget "$TMP/palette2.json" 'd.data.extensionWidgets.length')"
check_contains "palette shows smoke-hero" "$TMP/palette2.json" 'smoke-hero'
check_contains "extension widget marked not rendering publicly" "$TMP/palette2.json" '"rendersPublicly":false'

"$WRANGLER" d1 execute DB --local --command "UPDATE wl_extension_instances SET status = 'disabled' WHERE slug = 'smoke-ext'" >/dev/null
curl -sS -o "$TMP/palette3.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/builder/palette" > "$TMP/palette3.code"
check "extension widget gone when the instance is disabled" 0 "$(jget "$TMP/palette3.json" 'd.data.extensionWidgets.length')"

# --- extension-widget registry ----------------------------------------
curl -sS -o "$TMP/widget.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/builder/widgets" \
  -H 'Content-Type: application/json' -d '{"extensionSlug":"smoke-ext","widgetType":"smoke-cta","label":"Smoke CTA","icon":"star","description":"A call to action."}' > "$TMP/widget.code"
check "POST /api/builder/widgets smoke-cta" 201 "$(cat "$TMP/widget.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/builder/widgets" \
  -H 'Content-Type: application/json' -d '{"extensionSlug":"smoke-ext","widgetType":"smoke-bad","label":"Bad Icon","icon":"no-such-icon","description":"x"}' > "$TMP/widget-bad-icon.code"
check "register with an unknown icon" 400 "$(cat "$TMP/widget-bad-icon.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/builder/widgets" \
  -H 'Content-Type: application/json' -d '{"extensionSlug":"ghost-ext","widgetType":"ghost","label":"Ghost","icon":"star","description":"x"}' > "$TMP/widget-ghost.code"
check "register for an uninstalled extension" 404 "$(cat "$TMP/widget-ghost.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/builder/widgets" \
  -H 'Content-Type: application/json' -d '{"extensionSlug":"smoke-ext","widgetType":"Bad Type","label":"Bad Type","icon":"star","description":"x"}' > "$TMP/widget-bad-type.code"
check "register with a bad widget type" 400 "$(cat "$TMP/widget-bad-type.code")"
curl -sS -o /dev/null -w "%{http_code}" -X POST "$BASE/api/builder/widgets" \
  -H 'Content-Type: application/json' -d '{"extensionSlug":"smoke-ext","widgetType":"anon","label":"Anon","icon":"star","description":"x"}' > "$TMP/widget-anon.code"
check "register a widget anon" 401 "$(cat "$TMP/widget-anon.code")"
curl -sS -o "$TMP/widgets.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/builder/widgets" > "$TMP/widgets.code"
check "GET /api/builder/widgets" 200 "$(cat "$TMP/widgets.code")"
check_contains "widget list has smoke-hero" "$TMP/widgets.json" 'smoke-hero'
check_contains "widget list has smoke-cta" "$TMP/widgets.json" 'smoke-cta'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X DELETE "$BASE/api/builder/widgets/smoke-ext/smoke-cta" > "$TMP/widget-del.code"
check "DELETE /api/builder/widgets smoke-cta" 200 "$(cat "$TMP/widget-del.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X DELETE "$BASE/api/builder/widgets/smoke-ext/smoke-cta" > "$TMP/widget-del2.code"
check "DELETE the same widget again" 404 "$(cat "$TMP/widget-del2.code")"

# --- undo/redo state (admin history module, Node tests) ---------------
if node --test admin/src/lib/builderHistory.test.ts >/tmp/smoke-p11-history.log 2>&1; then
  PASS=$((PASS + 1)); echo "  ok  builder history Node tests (3/3)"
else
  FAIL=$((FAIL + 1)); echo "  FAIL builder history Node tests"; tail -10 /tmp/smoke-p11-history.log
fi

# --- the admin SPA ships the builder route -----------------------------
# (the built bundle lives under admin/dist/admin/assets/, not
# admin/dist/assets/ — the vite outDir is dist/admin, base /admin/.)
if grep -q "builder" admin/dist/admin/assets/*.js 2>/dev/null; then
  PASS=$((PASS + 1)); echo "  ok  admin bundle contains the builder route"
else
  FAIL=$((FAIL + 1)); echo "  FAIL admin bundle does not contain the builder route (run bash scripts/build-admin.sh)"
fi

echo
echo "== P11 smoke: $PASS passed, $FAIL failed =="
rm -rf "$TMP"
[ "$FAIL" -eq 0 ]
