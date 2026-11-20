#!/usr/bin/env bash
# WORDLESS v2 P8 smoke test — WooCommerce subset end to end.
#
# Runs the Rust worker under local wrangler dev (port 8806) against a
# local D1 (migrations applied, one seeded admin) and checks:
#   variable products: create simple + variable, attributes CRUD
#     (delete refused while a variation uses the attribute), variation
#     CRUD + parent price syncs to the minimum variation price, SKU
#     lookups, bad variation 400, variation price/stock on the detail.
#   coupons: create fixed + percent, validate (good code valid, bad
#     code invalid with a reason), usage counted exactly once at paid.
#   tax + shipping: settings on/off, rates CRUD, zones + methods CRUD,
#     quote math (subtotal − discount + tax + shipping, half-up).
#   checkout (mock): no-provider 503 first, then mock selected;
#     Woo checkout 201 with totals; bad coupon 400 (strict) while the
#     quote only reports it; webhook paid exactly once, replay
#     duplicate, bad signature 401; paid order decrements variation
#     stock + coupon usage once; re-pay transition 400.
#   CSV import: products (4 rows imported, re-import updates),
#     orders (imported as fulfilled, stock NOT re-decremented,
#     re-import skips), malformed row fails with a reason.
#   storefront: /shop lists products, product page shows the variation
#     selector + variation list, form add resolves attributes to a
#     variation, cart shows the variation label, form checkout 303s to
#     the mock test page and paying there marks the order paid.
#
# Usage: ./scripts/smoke-p8.sh [--keep]   (--keep leaves wrangler dev running)
# Clean room first: rm -rf .wrangler   (the script applies migrations itself)
# Requires: wrangler (TS repo's project-managed binary), worker-build, node.
# Seeded admin (password "smoke-admin-2026"): login "admin".
#
# SIZE NOTE (2026-11-20): P8 pushed the main Worker to 3,241,322 bytes
# (95,594 over the free-tier limit); the same-day size remedy split
# social (P7) and WooCommerce (P8) into their own Workers, so `npm run
# build` passes the size gate again and this script runs the -Oz build.
# If the gate ever blocks a run again, the local-only escape hatch is
# WORDLESS_NO_WASM_OPT=1 (the served WASM is unoptimized but
# behaviourally identical; never a deploy path).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"

WRANGLER="$HOME/workspace/wordless/wordless-cms/node_modules/.bin/wrangler"
PORT="${SMOKE_PORT:-8806}"
BASE="http://127.0.0.1:$PORT"
MOCK_SECRET="smoke-mock-secret"
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

echo "== P8 smoke: applying migrations (local D1) =="
"$WRANGLER" d1 migrations apply DB --local >/dev/null

echo "== P8 smoke: seeding admin (password smoke-admin-2026) =="
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

# Split (2026-11-20): conversion runs in the converter Worker — start
# it first, and point the main Worker at it. Size remedy (same day):
# WooCommerce runs in the Woo Worker — start it too, with the same
# mock webhook secret (the Woo Worker verifies mock webhooks itself).
source "$ROOT/scripts/converter-dev.sh"
start_converter
source "$ROOT/scripts/split-workers-dev.sh"
start_woo --var MOCK_PAYMENT_WEBHOOK_SECRET:$MOCK_SECRET
echo "== P8 smoke: starting wrangler dev on :$PORT =="
"$WRANGLER" dev --port "$PORT" --local --var MOCK_PAYMENT_WEBHOOK_SECRET:$MOCK_SECRET --var "WORDLESS_CONVERTER_URL:$CONVERTER_URL" --var "WORDLESS_WOO_URL:$WOO_URL" >/tmp/smoke-p8-wrangler.log 2>&1 &
DEV_PID=$!
cleanup() {
  kill "$DEV_PID" 2>/dev/null || true
  for child in $(pgrep -P "$DEV_PID" 2>/dev/null); do kill "$child" 2>/dev/null || true; done
  stop_converter
  stop_woo
}
trap cleanup EXIT

# Wait for the worker to answer (wrangler dev builds first — can take a while).
for i in $(seq 1 240); do
  if curl -fsS -o /dev/null "$BASE/api/health" 2>/dev/null; then break; fi
  sleep 2
done

echo "== P8 smoke: checks =="
TMP="$(mktemp -d)"

# login — logs in and stores the session cookie.
curl -sS -o "$TMP/login.json" -w "%{http_code}" -D "$TMP/login.headers" -X POST "$BASE/api/auth/login" \
  -H 'Content-Type: application/json' -d '{"login":"admin","password":"smoke-admin-2026"}' > "$TMP/login.code"
check "login admin" 200 "$(cat "$TMP/login.code")"
ADMIN_COOKIE="$(grep -i '^set-cookie:' "$TMP/login.headers" | head -1 | sed -e 's/^[^:]*: *//' -e 's/;.*//' | tr -d '\r')"

# --- guards: anonymous cannot use the Woo admin API ---
curl -sS -o /dev/null -w "%{http_code}" "$BASE/api/woo/coupons" > "$TMP/anon.code"
check "GET /api/woo/coupons anon" 401 "$(cat "$TMP/anon.code")"

# --- variable products: simple + variable, attributes, variations ---
curl -sS -o "$TMP/tee.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/woo/products" \
  -H 'Content-Type: application/json' -d '{"name":"Plain Tee","priceMinor":999,"currency":"NGN","stock":10,"status":"published","type":"simple","sku":"TEE-API"}' > "$TMP/tee.code"
check "POST /api/woo/products simple tee" 201 "$(cat "$TMP/tee.code")"
check_contains "tee sku stored" "$TMP/tee.json" '"sku":"TEE-API"'
TEE_ID="$(jget "$TMP/tee.json" 'd.data.id')"
curl -sS -o "$TMP/hoodie.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/woo/products" \
  -H 'Content-Type: application/json' -d '{"name":"Pack Hoodie","priceMinor":0,"currency":"NGN","stock":0,"status":"published","type":"variable","sku":"HOODIE-API"}' > "$TMP/hoodie.code"
check "POST /api/woo/products variable hoodie" 201 "$(cat "$TMP/hoodie.code")"
HOODIE_ID="$(jget "$TMP/hoodie.json" 'd.data.id')"
# A simple product cannot have attributes.
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/woo/products/$TEE_ID/attributes" \
  -H 'Content-Type: application/json' -d '{"name":"Size","options":["S","M"],"isVariation":true}' > "$TMP/attr-simple.code"
check "POST attribute on simple product" 400 "$(cat "$TMP/attr-simple.code")"
curl -sS -o "$TMP/attr-size.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/woo/products/$HOODIE_ID/attributes" \
  -H 'Content-Type: application/json' -d '{"name":"Size","options":["S","M","L"],"isVariation":true,"visible":true}' > "$TMP/attr-size.code"
check "POST attribute Size" 201 "$(cat "$TMP/attr-size.code")"
SIZE_ATTR="$(jget "$TMP/attr-size.json" 'd.data.id')"
curl -sS -o "$TMP/attr-colour.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/woo/products/$HOODIE_ID/attributes" \
  -H 'Content-Type: application/json' -d '{"name":"Colour","options":["Red","Blue"],"isVariation":true,"visible":true}' > "$TMP/attr-colour.code"
check "POST attribute Colour" 201 "$(cat "$TMP/attr-colour.code")"
# Duplicate attribute name is refused.
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/woo/products/$HOODIE_ID/attributes" \
  -H 'Content-Type: application/json' -d '{"name":"Size","options":["XL"],"isVariation":true}' > "$TMP/attr-dup.code"
check "POST duplicate attribute name" 400 "$(cat "$TMP/attr-dup.code")"
# Variations: S/Red 1299 (stock 5), M/Blue 1499 (stock 3).
curl -sS -o "$TMP/var-s.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/woo/products/$HOODIE_ID/variations" \
  -H 'Content-Type: application/json' -d '{"sku":"HOODIE-API-S","priceMinor":1299,"salePriceMinor":0,"stock":5,"status":"published","attributes":{"Size":"S","Colour":"Red"}}' > "$TMP/var-s.code"
check "POST variation S/Red" 201 "$(cat "$TMP/var-s.code")"
check_contains "variation label sorted" "$TMP/var-s.json" '"label":"Colour: Red, Size: S"'
check_contains "variation effective price" "$TMP/var-s.json" '"effectivePriceMinor":1299'
VAR_S="$(jget "$TMP/var-s.json" 'd.data.id')"
curl -sS -o "$TMP/var-m.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/woo/products/$HOODIE_ID/variations" \
  -H 'Content-Type: application/json' -d '{"sku":"HOODIE-API-M","priceMinor":1499,"salePriceMinor":0,"stock":3,"status":"published","attributes":{"Size":"M","Colour":"Blue"}}' > "$TMP/var-m.code"
check "POST variation M/Blue" 201 "$(cat "$TMP/var-m.code")"
VAR_M="$(jget "$TMP/var-m.json" 'd.data.id')"
# A variation with an unknown option value is refused.
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/woo/products/$HOODIE_ID/variations" \
  -H 'Content-Type: application/json' -d '{"sku":"HOODIE-API-X","priceMinor":999,"stock":1,"status":"published","attributes":{"Size":"XXL","Colour":"Red"}}' > "$TMP/var-bad.code"
check "POST variation with unknown option" 400 "$(cat "$TMP/var-bad.code")"
# Parent price syncs to the minimum variation price (1299).
curl -sS -o "$TMP/hoodie-get.json" -w "%{http_code}" "$BASE/api/woo/products/$HOODIE_ID" > "$TMP/hoodie-get.code"
check "GET /api/woo/products/:id (variable detail)" 200 "$(cat "$TMP/hoodie-get.code")"
[ "$(jget "$TMP/hoodie-get.json" 'd.data.priceMinor')" = "1299" ] && { PASS=$((PASS + 1)); echo "  ok  parent price synced to min variation price (1299)"; } || { FAIL=$((FAIL + 1)); echo "  FAIL parent price: got $(jget "$TMP/hoodie-get.json" 'd.data.priceMinor'), expected 1299"; }
[ "$(jget "$TMP/hoodie-get.json" 'd.data.variations.length')" = "2" ] && { PASS=$((PASS + 1)); echo "  ok  detail carries 2 variations"; } || { FAIL=$((FAIL + 1)); echo "  FAIL variation count: got $(jget "$TMP/hoodie-get.json" 'd.data.variations.length')"; }
# SKU lookups.
curl -sS -o "$TMP/sku.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/woo/variations/sku/HOODIE-API-S" > "$TMP/sku.code"
check "GET /api/woo/variations/sku/HOODIE-API-S" 200 "$(cat "$TMP/sku.code")"
check_contains "variation by sku is S/Red" "$TMP/sku.json" '"label":"Colour: Red, Size: S"'
curl -sS -o "$TMP/sku-prod.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/woo/products/sku/TEE-API" > "$TMP/sku-prod.code"
check "GET /api/woo/products/sku/TEE-API" 200 "$(cat "$TMP/sku-prod.code")"
# Deleting an attribute still used by a variation is refused.
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X DELETE "$BASE/api/woo/products/$HOODIE_ID/attributes/$SIZE_ATTR" > "$TMP/attr-del.code"
check "DELETE attribute in use" 400 "$(cat "$TMP/attr-del.code")"
# Sale price becomes the effective price, and the parent re-syncs.
curl -sS -o "$TMP/var-s2.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/woo/products/$HOODIE_ID/variations/$VAR_S" \
  -H 'Content-Type: application/json' -d '{"salePriceMinor":999}' > "$TMP/var-s2.code"
check "PUT variation sale price" 200 "$(cat "$TMP/var-s2.code")"
check_contains "sale price is effective price" "$TMP/var-s2.json" '"effectivePriceMinor":999'
curl -sS -o "$TMP/hoodie-get2.json" -w "%{http_code}" "$BASE/api/woo/products/$HOODIE_ID" > "$TMP/hoodie-get2.code"
[ "$(jget "$TMP/hoodie-get2.json" 'd.data.priceMinor')" = "999" ] && { PASS=$((PASS + 1)); echo "  ok  parent price re-synced to sale price (999)"; } || { FAIL=$((FAIL + 1)); echo "  FAIL parent price after sale: got $(jget "$TMP/hoodie-get2.json" 'd.data.priceMinor'), expected 999"; }
# Restore the regular price so the checkout math below holds (1299).
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/woo/products/$HOODIE_ID/variations/$VAR_S" \
  -H 'Content-Type: application/json' -d '{"salePriceMinor":0}' > "$TMP/var-s3.code"
check "PUT variation sale price back to 0" 200 "$(cat "$TMP/var-s3.code")"

# --- coupons ---
curl -sS -o "$TMP/coupon.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/woo/coupons" \
  -H 'Content-Type: application/json' -d '{"code":"SAVE500","type":"fixed","amountMinor":500,"maxUses":0,"active":true}' > "$TMP/coupon.code"
check "POST /api/woo/coupons SAVE500 fixed" 201 "$(cat "$TMP/coupon.code")"
curl -sS -o "$TMP/coupon2.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/woo/coupons" \
  -H 'Content-Type: application/json' -d '{"code":"TENPCT","type":"percent","percentBp":1000,"maxUses":1,"active":true}' > "$TMP/coupon2.code"
check "POST /api/woo/coupons TENPCT percent" 201 "$(cat "$TMP/coupon2.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/woo/coupons" \
  -H 'Content-Type: application/json' -d '{"code":"SAVE500","type":"fixed","amountMinor":100}' > "$TMP/coupon-dup.code"
check "POST duplicate coupon code" 400 "$(cat "$TMP/coupon-dup.code")"
curl -sS -o "$TMP/validate-ok.json" -w "%{http_code}" -X POST "$BASE/api/woo/coupons/validate" \
  -H 'Content-Type: application/json' -d '{"code":"save500","subtotalMinor":3297}' > "$TMP/validate-ok.code"
check "POST /api/woo/coupons/validate SAVE500 (case-insensitive)" 200 "$(cat "$TMP/validate-ok.code")"
check_contains "validate SAVE500 is valid" "$TMP/validate-ok.json" '"valid":true'
check_contains "validate SAVE500 discount 500" "$TMP/validate-ok.json" '"discountMinor":500'
curl -sS -o "$TMP/validate-bad.json" -w "%{http_code}" -X POST "$BASE/api/woo/coupons/validate" \
  -H 'Content-Type: application/json' -d '{"code":"NOPE","subtotalMinor":3297}' > "$TMP/validate-bad.code"
check "POST /api/woo/coupons/validate unknown code" 200 "$(cat "$TMP/validate-bad.code")"
check_contains "validate unknown code is invalid" "$TMP/validate-bad.json" '"valid":false'
check_contains "validate unknown code has a reason" "$TMP/validate-bad.json" 'does not exist'

# --- tax + shipping ---
curl -sS -o "$TMP/taxset.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/woo/tax-settings" \
  -H 'Content-Type: application/json' -d '{"taxEnabled":true,"pricesIncludeTax":false}' > "$TMP/taxset.code"
check "PUT /api/woo/tax-settings on" 200 "$(cat "$TMP/taxset.code")"
curl -sS -o "$TMP/taxrate.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/woo/tax-rates" \
  -H 'Content-Type: application/json' -d '{"name":"NG VAT","country":"NG","state":"","rateBp":750}' > "$TMP/taxrate.code"
check "POST /api/woo/tax-rates NG 7.5%" 201 "$(cat "$TMP/taxrate.code")"
curl -sS -o "$TMP/zone.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/woo/shipping-zones" \
  -H 'Content-Type: application/json' -d '{"name":"Nigeria","countries":["NG"]}' > "$TMP/zone.code"
check "POST /api/woo/shipping-zones Nigeria" 201 "$(cat "$TMP/zone.code")"
ZONE_ID="$(jget "$TMP/zone.json" 'd.data.id')"
curl -sS -o "$TMP/method.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/woo/shipping-zones/$ZONE_ID/methods" \
  -H 'Content-Type: application/json' -d '{"name":"Flat rate","type":"flat_rate","costMinor":2500}' > "$TMP/method.code"
check "POST flat-rate method 2500" 201 "$(cat "$TMP/method.code")"
METHOD_ID="$(jget "$TMP/method.json" 'd.data.id')"
curl -sS -o "$TMP/method2.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/woo/shipping-zones/$ZONE_ID/methods" \
  -H 'Content-Type: application/json' -d '{"name":"Free over 100","type":"free_shipping","costMinor":0,"minOrderMinor":10000}' > "$TMP/method2.code"
check "POST free-shipping method (threshold 10000)" 201 "$(cat "$TMP/method2.code")"
curl -sS -o "$TMP/zones.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/woo/shipping-zones" > "$TMP/zones.code"
check "GET /api/woo/shipping-zones" 200 "$(cat "$TMP/zones.code")"
check_contains "zone carries its methods" "$TMP/zones.json" '"name":"Flat rate"'

# --- cart + quote ---
# Add tee x2 (API cart, cookie jar) then the S/Red variation x1.
curl -sS -o "$TMP/cart.json" -w "%{http_code}" -c "$TMP/cart.jar" -X POST "$BASE/api/woo/cart" \
  -H 'Content-Type: application/json' -d "{\"productId\":$TEE_ID,\"quantity\":2}" > "$TMP/cart.code"
check "POST /api/woo/cart tee x2" 200 "$(cat "$TMP/cart.code")"
check_contains "cart total 1998 after tee x2" "$TMP/cart.json" '"totalMinor":1998'
curl -sS -o "$TMP/cart2.json" -w "%{http_code}" -b "$TMP/cart.jar" -c "$TMP/cart.jar" -X POST "$BASE/api/woo/cart" \
  -H 'Content-Type: application/json' -d "{\"productId\":$HOODIE_ID,\"variationId\":$VAR_S,\"quantity\":1}" > "$TMP/cart2.code"
check "POST /api/woo/cart hoodie S/Red x1" 200 "$(cat "$TMP/cart2.code")"
check_contains "cart total 3297" "$TMP/cart2.json" '"totalMinor":3297'
check_contains "cart line carries variation label" "$TMP/cart2.json" '"variationLabel":"Colour: Red, Size: S"'
# A variable product without a variation is refused.
curl -sS -o /dev/null -w "%{http_code}" -b "$TMP/cart.jar" -X POST "$BASE/api/woo/cart" \
  -H 'Content-Type: application/json' -d "{\"productId\":$HOODIE_ID,\"quantity\":1}" > "$TMP/cart-bare.code"
check "POST /api/woo/cart bare variable product" 400 "$(cat "$TMP/cart-bare.code")"
# Quote: 3297 − 500 coupon = 2797 taxable; tax 7.5% half-up = 210;
# shipping cheapest applicable = flat 2500 (free threshold 10000 not met).
curl -sS -o "$TMP/quote.json" -w "%{http_code}" -b "$TMP/cart.jar" -X POST "$BASE/api/woo/quote" \
  -H 'Content-Type: application/json' -d '{"couponCode":"SAVE500","country":"NG"}' > "$TMP/quote.code"
check "POST /api/woo/quote SAVE500 + NG" 200 "$(cat "$TMP/quote.code")"
check_contains "quote subtotal 3297" "$TMP/quote.json" '"subtotalMinor":3297'
check_contains "quote discount 500" "$TMP/quote.json" '"discountMinor":500'
check_contains "quote tax 210 (7.5% of 2797, half-up)" "$TMP/quote.json" '"taxMinor":210'
check_contains "quote shipping 2500 (flat rate)" "$TMP/quote.json" '"shippingMinor":2500'
check_contains "quote total 5507" "$TMP/quote.json" '"totalMinor":5507'
# Bad coupon: the quote reports it, the checkout refuses it.
curl -sS -o "$TMP/quote-bad.json" -w "%{http_code}" -b "$TMP/cart.jar" -X POST "$BASE/api/woo/quote" \
  -H 'Content-Type: application/json' -d '{"couponCode":"NOPE","country":"NG"}' > "$TMP/quote-bad.code"
check "POST /api/woo/quote bad coupon" 200 "$(cat "$TMP/quote-bad.code")"
check_contains "quote bad coupon discount 0" "$TMP/quote-bad.json" '"discountMinor":0'
check_contains "quote bad coupon has a note" "$TMP/quote-bad.json" '"couponNote":'
curl -sS -o /dev/null -w "%{http_code}" -b "$TMP/cart.jar" -X POST "$BASE/api/woo/checkout" \
  -H 'Content-Type: application/json' -d '{"customerName":"Smoke Buyer","customerEmail":"buyer@example.com","couponCode":"NOPE","country":"NG"}' > "$TMP/checkout-bad.code"
check "POST /api/woo/checkout bad coupon (strict)" 400 "$(cat "$TMP/checkout-bad.code")"

# --- checkout: no provider is a 503, never a faked success ---
curl -sS -o "$TMP/checkout-503.json" -w "%{http_code}" -b "$TMP/cart.jar" -X POST "$BASE/api/woo/checkout" \
  -H 'Content-Type: application/json' -d '{"customerName":"Smoke Buyer","customerEmail":"buyer@example.com","couponCode":"SAVE500","country":"NG"}' > "$TMP/checkout-503.code"
check "POST /api/woo/checkout with no provider" 503 "$(cat "$TMP/checkout-503.code")"
check_contains "no-provider checkout says so" "$TMP/checkout-503.json" 'payment_not_configured'
# The failed checkout did not consume the cart or the coupon.
curl -sS -o "$TMP/cart-after503.json" -w "%{http_code}" -b "$TMP/cart.jar" "$BASE/api/woo/cart" > "$TMP/cart-after503.code"
check_contains "cart still 3297 after 503" "$TMP/cart-after503.json" '"totalMinor":3297'

# Select the mock provider, then check out for real.
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/commerce/payments/select" \
  -H 'Content-Type: application/json' -d '{"provider":"mock","mode":"test"}' > "$TMP/pay-sel.code"
check "POST /api/commerce/payments/select mock" 200 "$(cat "$TMP/pay-sel.code")"
curl -sS -o "$TMP/checkout.json" -w "%{http_code}" -b "$TMP/cart.jar" -X POST "$BASE/api/woo/checkout" \
  -H 'Content-Type: application/json' -d '{"customerName":"Smoke Buyer","customerEmail":"buyer@example.com","couponCode":"SAVE500","country":"NG"}' > "$TMP/checkout.code"
check "POST /api/woo/checkout (mock)" 201 "$(cat "$TMP/checkout.code")"
check_contains "checkout url is the mock test page" "$TMP/checkout.json" '/shop/checkout/test'
check_contains "checkout totals total 5507" "$TMP/checkout.json" '"totalMinor":5507'
ORDER_ID="$(jget "$TMP/checkout.json" 'd.data.order.id')"
ORDER_REF="$(jget "$TMP/checkout.json" 'd.data.reference')"
echo "  (order id: $ORDER_ID ref: $ORDER_REF)"
curl -sS -o "$TMP/order.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/commerce/orders/$ORDER_ID" > "$TMP/order.code"
check "GET /api/commerce/orders/:id" 200 "$(cat "$TMP/order.code")"
check_contains "order starts pending" "$TMP/order.json" '"status":"pending"'
check_contains "order carries coupon code" "$TMP/order.json" '"couponCode":"SAVE500"'
check_contains "order carries tax 210" "$TMP/order.json" '"taxMinor":210'
check_contains "order carries shipping 2500" "$TMP/order.json" '"shippingMinor":2500'
check_contains "order item carries variation label" "$TMP/order.json" '"variationLabel":"Colour: Red, Size: S"'

# --- mock webhook: paid exactly once, replay duplicate, bad sig 401 ---
WEBHOOK_BODY="{\"event\":\"payment.paid\",\"reference\":\"$ORDER_REF\"}"
WEBHOOK_SIG="$(node -e 'const c=require("node:crypto");console.log(c.createHmac("sha256",process.argv[1]).update(process.argv[2]).digest("hex"))' "$MOCK_SECRET" "$WEBHOOK_BODY")"
curl -sS -o "$TMP/hook.json" -w "%{http_code}" -X POST "$BASE/api/commerce/webhooks/mock" \
  -H 'Content-Type: application/json' -H "x-mock-signature: $WEBHOOK_SIG" -d "$WEBHOOK_BODY" > "$TMP/hook.code"
check "POST /api/commerce/webhooks/mock (signed)" 200 "$(cat "$TMP/hook.code")"
check_contains "webhook marks order paid" "$TMP/hook.json" '"order":"paid"'
curl -sS -o "$TMP/order-paid.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/commerce/orders/$ORDER_ID" > "$TMP/order-paid.code"
check_contains "order is paid after webhook" "$TMP/order-paid.json" '"status":"paid"'
# Coupon usage counted exactly once, variation stock decremented once.
curl -sS -o "$TMP/coupons.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/woo/coupons" > "$TMP/coupons.code"
[ "$(jget "$TMP/coupons.json" 'd.data.find(c=>c.code==="SAVE500").usageCount')" = "1" ] && { PASS=$((PASS + 1)); echo "  ok  SAVE500 usageCount is 1 after one paid order"; } || { FAIL=$((FAIL + 1)); echo "  FAIL SAVE500 usageCount: got $(jget "$TMP/coupons.json" 'd.data.find(c=>c.code==="SAVE500").usageCount')"; }
curl -sS -o "$TMP/var-s-after.json" -w "%{http_code}" "$BASE/api/woo/products/$HOODIE_ID/variations/$VAR_S" > "$TMP/var-s-after.code"
[ "$(jget "$TMP/var-s-after.json" 'd.data.stock')" = "4" ] && { PASS=$((PASS + 1)); echo "  ok  variation stock 5 -> 4 after paid order"; } || { FAIL=$((FAIL + 1)); echo "  FAIL variation stock after paid: got $(jget "$TMP/var-s-after.json" 'd.data.stock')"; }
curl -sS -o "$TMP/tee-after.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/woo/products/sku/TEE-API" > "$TMP/tee-after.code"
[ "$(jget "$TMP/tee-after.json" 'd.data.stock')" = "8" ] && { PASS=$((PASS + 1)); echo "  ok  tee stock 10 -> 8 after paid order"; } || { FAIL=$((FAIL + 1)); echo "  FAIL tee stock after paid: got $(jget "$TMP/tee-after.json" 'd.data.stock')"; }
curl -sS -o "$TMP/hook-replay.json" -w "%{http_code}" -X POST "$BASE/api/commerce/webhooks/mock" \
  -H 'Content-Type: application/json' -H "x-mock-signature: $WEBHOOK_SIG" -d "$WEBHOOK_BODY" > "$TMP/hook-replay.code"
check "POST /api/commerce/webhooks/mock (replay)" 200 "$(cat "$TMP/hook-replay.code")"
check_contains "webhook replay is duplicate" "$TMP/hook-replay.json" '"order":"duplicate"'
curl -sS -o "$TMP/coupons2.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/woo/coupons" > "$TMP/coupons2.code"
[ "$(jget "$TMP/coupons2.json" 'd.data.find(c=>c.code==="SAVE500").usageCount')" = "1" ] && { PASS=$((PASS + 1)); echo "  ok  SAVE500 usageCount still 1 after replay"; } || { FAIL=$((FAIL + 1)); echo "  FAIL SAVE500 usageCount after replay"; }
curl -sS -o /dev/null -w "%{http_code}" -X POST "$BASE/api/commerce/webhooks/mock" \
  -H 'Content-Type: application/json' -H "x-mock-signature: deadbeef" -d "$WEBHOOK_BODY" > "$TMP/hook-bad.code"
check "POST /api/commerce/webhooks/mock (bad signature)" 401 "$(cat "$TMP/hook-bad.code")"
# A paid order cannot be paid again.
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/commerce/orders/$ORDER_ID/transition" \
  -H 'Content-Type: application/json' -d '{"status":"paid"}' > "$TMP/repay.code"
check "POST transition paid -> paid" 400 "$(cat "$TMP/repay.code")"

# --- CSV import: products, then orders, then re-imports ---
cat > "$TMP/products.csv" <<'CSV'
ID,Type,SKU,Name,Published,Is featured?,Visibility in catalog,Short description,Description,Tax status,Tax class,In stock?,Stock,Backorders allowed?,Sold individually?,Weight (kg),Length (cm),Width (cm),Height (cm),Allow customer reviews?,Sale price,Regular price,Categories,Tags,Shipping class,Images,Parent,Position,Attribute 1 name,Attribute 1 value(s),Attribute 1 visible,Attribute 1 global,Attribute 1 default,Attribute 2 name,Attribute 2 value(s),Attribute 2 visible,Attribute 2 global,Attribute 2 default
,simple,TEE-001,Plain Tee,1,0,visible,"A plain tee.","",taxable,,1,10,0,0,,,,,1,9.99,12.99,Clothing,tee,,,,,,,,,,,,,,
,variable,HOODIE,Pack Hoodie,1,0,visible,"A hoodie.","",taxable,,1,,0,0,,,,,1,,,Clothing,hoodie,,,,,Size,"S, M, L",1,0,,Colour,"Red, Blue",1,0,
,variation,HOODIE-S,,1,0,visible,"","",taxable,,1,5,0,0,,,,,1,,12.99,,,,,HOODIE,,Size,S,1,0,,Colour,Red,1,0,
,variation,HOODIE-M,,1,0,visible,"","",taxable,,1,3,0,0,,,,,1,,14.99,,,,,HOODIE,,Size,M,1,0,,Colour,Blue,1,0,
CSV
node -e 'const fs=require("fs");const csv=fs.readFileSync(process.argv[1],"utf8");fs.writeFileSync(process.argv[2],JSON.stringify({kind:"products",csv}));' "$TMP/products.csv" "$TMP/import-body.json"
curl -sS -o "$TMP/import.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/woo/import" \
  -H 'Content-Type: application/json' -d @"$TMP/import-body.json" > "$TMP/import.code"
check "POST /api/woo/import products" 200 "$(cat "$TMP/import.code")"
check_contains "products import: 4 imported" "$TMP/import.json" '"imported":4'
check_contains "products import: 0 failed" "$TMP/import.json" '"failed":0'
curl -sS -o "$TMP/import2.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/woo/import" \
  -H 'Content-Type: application/json' -d @"$TMP/import-body.json" > "$TMP/import2.code"
check "POST /api/woo/import products (re-import)" 200 "$(cat "$TMP/import2.code")"
check_contains "products re-import: 4 updated" "$TMP/import2.json" '"updated":4'
check_contains "products re-import: 0 imported" "$TMP/import2.json" '"imported":0'
cat > "$TMP/orders.csv" <<'CSV'
order_id,order_number,order_date,order_status,customer_email,customer_first_name,customer_last_name,billing_country,billing_state,billing_postcode,payment_method,transaction_id,order_currency,order_total,discount_total,tax_total,shipping_total,shipping_method,line_item_1,line_item_2
1001,1001,2026-01-15 10:30:00,wc-completed,ada@example.test,Ada,Lovelace,NG,LA,100001,paystack,txn_1001,NGN,70.96,0,12.99,25.00,Flat rate,"Pack Hoodie|HOODIE-S|1|12.99","Plain Tee|TEE-001|2|9.99"
1002,1002,2026-01-16 09:00:00,processing,carol@example.test,Carol,,NG,OY,,mock,mock_1002,NGN,14.99,0,0,0,,"Pack Hoodie|HOODIE-M|1|14.99",
CSV
node -e 'const fs=require("fs");const csv=fs.readFileSync(process.argv[1],"utf8");fs.writeFileSync(process.argv[2],JSON.stringify({kind:"orders",csv}));' "$TMP/orders.csv" "$TMP/import-body.json"
curl -sS -o "$TMP/import-orders.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/woo/import" \
  -H 'Content-Type: application/json' -d @"$TMP/import-body.json" > "$TMP/import-orders.code"
check "POST /api/woo/import orders" 200 "$(cat "$TMP/import-orders.code")"
check_contains "orders import: 2 imported" "$TMP/import-orders.json" '"imported":2'
curl -sS -o "$TMP/orders.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/commerce/orders?status=fulfilled" > "$TMP/orders.code"
check_contains "imported order is fulfilled" "$TMP/orders.json" '"sourceId":"1001"'
# Imported orders never re-decrement stock: HOODIE-S stays 5, TEE-001 stays 10.
curl -sS -o "$TMP/hoodie-s-imp.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/woo/variations/sku/HOODIE-S" > "$TMP/hoodie-s-imp.code"
[ "$(jget "$TMP/hoodie-s-imp.json" 'd.data.stock')" = "5" ] && { PASS=$((PASS + 1)); echo "  ok  imported order did not re-decrement variation stock (5)"; } || { FAIL=$((FAIL + 1)); echo "  FAIL imported variation stock: got $(jget "$TMP/hoodie-s-imp.json" 'd.data.stock')"; }
curl -sS -o "$TMP/import-orders2.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/woo/import" \
  -H 'Content-Type: application/json' -d @"$TMP/import-body.json" > "$TMP/import-orders2.code"
check_contains "orders re-import: 2 skipped" "$TMP/import-orders2.json" '"skipped":2'
# A grouped product row fails with a reason instead of vanishing.
printf 'ID,Type,SKU,Name,Published\n,grouped,BUNDLE-1,Bundle,1\n' > "$TMP/bad.csv"
node -e 'const fs=require("fs");const csv=fs.readFileSync(process.argv[1],"utf8");fs.writeFileSync(process.argv[2],JSON.stringify({kind:"products",csv}));' "$TMP/bad.csv" "$TMP/import-body.json"
curl -sS -o "$TMP/import-bad.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/woo/import" \
  -H 'Content-Type: application/json' -d @"$TMP/import-body.json" > "$TMP/import-bad.code"
check "POST /api/woo/import grouped row" 200 "$(cat "$TMP/import-bad.code")"
check_contains "grouped row fails with a reason" "$TMP/import-bad.json" '"status":"failed"'
# Customers import + list.
printf 'Email,First name,Last name,Role,Date created\nbob@example.test,Bob,Builder,customer,2026-01-17 08:00:00\n' > "$TMP/customers.csv"
node -e 'const fs=require("fs");const csv=fs.readFileSync(process.argv[1],"utf8");fs.writeFileSync(process.argv[2],JSON.stringify({kind:"customers",csv}));' "$TMP/customers.csv" "$TMP/import-body.json"
curl -sS -o "$TMP/import-cust.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/woo/import" \
  -H 'Content-Type: application/json' -d @"$TMP/import-body.json" > "$TMP/import-cust.code"
check "POST /api/woo/import customers" 200 "$(cat "$TMP/import-cust.code")"
check_contains "customers import: 1 imported" "$TMP/import-cust.json" '"imported":1'
curl -sS -o "$TMP/customers.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/woo/customers" > "$TMP/customers.code"
check "GET /api/woo/customers" 200 "$(cat "$TMP/customers.code")"
check_contains "customers list has Ada" "$TMP/customers.json" 'ada@example.test'
check_contains "customers list has Bob" "$TMP/customers.json" 'bob@example.test'
check_contains "Ada has 1 order" "$TMP/customers.json" '"totalOrders":1'

# --- storefront: shop, product page, form add, cart, form checkout ---
curl -sS -o "$TMP/shop.html" -w "%{http_code}" "$BASE/shop" > "$TMP/shop.code"
check "GET /shop" 200 "$(cat "$TMP/shop.code")"
check_contains "shop lists Plain Tee" "$TMP/shop.html" 'Plain Tee'
check_contains "shop lists Pack Hoodie" "$TMP/shop.html" 'Pack Hoodie'
HOODIE_SLUG="$(jget "$TMP/hoodie-get.json" 'd.data.slug')"
curl -sS -o "$TMP/product.html" -w "%{http_code}" "$BASE/shop/$HOODIE_SLUG" > "$TMP/product.code"
check "GET /shop/:slug (variable product)" 200 "$(cat "$TMP/product.code")"
check_contains "product page shows From price" "$TMP/product.html" 'From '
check_contains "product page has Size selector" "$TMP/product.html" 'name="attr_size"'
check_contains "product page has Colour selector" "$TMP/product.html" 'name="attr_colour"'
check_contains "product page lists S/Red variation" "$TMP/product.html" 'Colour: Red, Size: S'
# Form add: Size S + Colour Red resolves to the S/Red variation.
curl -sS -o /dev/null -w "%{http_code}" -D "$TMP/shop-add.headers" -c "$TMP/shop.jar" -X POST "$BASE/shop/cart/add" \
  -H 'Content-Type: application/x-www-form-urlencoded' -d "productId=$HOODIE_ID&quantity=1&attr_size=S&attr_colour=Red" > "$TMP/shop-add.code"
check "POST /shop/cart/add (attributes resolve)" 303 "$(cat "$TMP/shop-add.code")"
SHOP_COOKIE="$(grep -i '^set-cookie:' "$TMP/shop-add.headers" | head -1 | sed -e 's/^[^:]*: *//' -e 's/;.*//' | tr -d '\r')"
[ -n "$SHOP_COOKIE" ] || SHOP_COOKIE="$(grep -i 'wordless_cart' "$TMP/shop.jar" | awk '{print $7"="$8}' | tail -1)"
curl -sS -o "$TMP/cart-page.html" -w "%{http_code}" -H "Cookie: $SHOP_COOKIE" "$BASE/shop/cart" > "$TMP/cart-page.code"
check "GET /shop/cart" 200 "$(cat "$TMP/cart-page.code")"
check_contains "cart page shows variation label" "$TMP/cart-page.html" 'Colour: Red, Size: S'
check_contains "cart page has coupon field" "$TMP/cart-page.html" 'name="couponCode"'
check_contains "cart page has country field" "$TMP/cart-page.html" 'name="country"'
check_contains "cart page has shipping method field" "$TMP/cart-page.html" 'name="shippingMethodId"'
# Form checkout with the coupon: 303 to the mock test page.
curl -sS -o /dev/null -w "%{http_code}" -D "$TMP/shop-checkout.headers" -H "Cookie: $SHOP_COOKIE" -X POST "$BASE/shop/checkout" \
  -H 'Content-Type: application/x-www-form-urlencoded' -d "customerName=Shopper&customerEmail=shopper@example.com&couponCode=SAVE500&country=NG" > "$TMP/shop-checkout.code"
check "POST /shop/checkout (mock 303s to test page)" 303 "$(cat "$TMP/shop-checkout.code")"
check_contains "shop checkout redirects to test page" "$TMP/shop-checkout.headers" '/shop/checkout/test?order='
SHOP_ORDER_ID="$(grep -i '^location:' "$TMP/shop-checkout.headers" | head -1 | sed -e 's/^[^:]*: *//' | tr -d '\r' | sed -e 's/.*order=//' -e 's/&.*//')"
# The test page only simulates the webhook when the SUBMITTED reference
# matches the stored payment_reference (P5 F5) — send both fields
# (fixed 2026-11-20: orderId-only left the order pending).
SHOP_REF="$(grep -i '^location:' "$TMP/shop-checkout.headers" | head -1 | sed -e 's/^[^:]*: *//' | tr -d '\r' | sed -e 's/.*reference=//' -e 's/&.*//')"
curl -sS -o /dev/null -w "%{http_code}" -D "$TMP/shop-test.headers" -X POST "$BASE/shop/checkout/test" \
  -H 'Content-Type: application/x-www-form-urlencoded' -d "orderId=$SHOP_ORDER_ID&reference=$SHOP_REF" > "$TMP/shop-test.code"
check "POST /shop/checkout/test (simulate payment)" 303 "$(cat "$TMP/shop-test.code")"
check_contains "test checkout redirects to success" "$TMP/shop-test.headers" "/shop/checkout/success?order=$SHOP_ORDER_ID"
curl -sS -o "$TMP/shop-order.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/commerce/orders/$SHOP_ORDER_ID" > "$TMP/shop-order.code"
check_contains "form order is paid" "$TMP/shop-order.json" '"status":"paid"'
check_contains "form order total 3359 (1299-500+60 tax+2500 ship)" "$TMP/shop-order.json" '"totalMinor":3359'
curl -sS -o "$TMP/coupons3.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/woo/coupons" > "$TMP/coupons3.code"
[ "$(jget "$TMP/coupons3.json" 'd.data.find(c=>c.code==="SAVE500").usageCount')" = "2" ] && { PASS=$((PASS + 1)); echo "  ok  SAVE500 usageCount is 2 after the second paid order"; } || { FAIL=$((FAIL + 1)); echo "  FAIL SAVE500 usageCount after form order"; }

echo
echo "== P8 smoke: $PASS passed, $FAIL failed =="
rm -rf "$TMP"
[ "$FAIL" -eq 0 ]
