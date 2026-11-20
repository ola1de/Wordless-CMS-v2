#!/usr/bin/env bash
# WORDLESS v2 P6 smoke test — the extension runtime end to end.
#
# Runs the Rust worker under local wrangler dev (port 8800) against a
# local D1 (migrations applied, one seeded admin) and checks:
#   hello-dolly: marketplace ZIP install -> auto-convert -> compat
#     report -> load (devUrl) -> real extension Worker on :8801 ->
#     admin_notices hook fires and renders the lyric HTML.
#   host endpoint: a wrong token -> 401; a right token calling
#     get_option without options.read -> capability_denied (and the
#     denial is logged); granting options.read -> the call succeeds.
#   disable: disabling stops hooks within one request (skipped_disabled);
#     re-enabling restores them.
#   hostile plugin: a plugin whose PHP calls eval() -> the analyser
#     marks it blocked -> load refuses with the file named.
#   TT25 theme: full theme ZIP import -> load with source=theme ->
#     the theme's converted functions.php runs in the sandbox (its
#     callbacks hit the honest unmapped-WP-function boundary and the
#     runtime reports it cleanly). NOTE: the generated entry imports
#     all 99 converted files, but the smoke Worker runs functions.php
#     only — template/pattern files are render fragments whose
#     top-level code is not a runnable module, so running the full
#     99-file entry as a Worker is a known P6 gap (see the P6 report).
#     The script endpoint is still verified to list all 99 files.
#   runaway: an extension Worker that never responds -> the invoke
#     returns promptly (local workerd detects the hung isolate and
#     answers HTTP 500 before the 5,000 ms client-side timeout fires)
#     and the main Worker stays responsive. The 5,000 ms race itself
#     is unit-tested (race_with_timeout in wordless-extensions).
#
# NOT covered here (unit-tested in wordless-extensions): the loader's
# code scan for eval()/Function()/dynamic import()/fetch() inside
# converted code — no PHP construct emits a literal fetch() (the
# emitter maps wp_remote_get to an __unmapped_* helper that throws),
# so that layer is only reachable by hand-crafted converted code.
#
# Usage: ./scripts/smoke-p6.sh [--keep]   (--keep leaves wrangler dev running)
# Clean room first: rm -rf .wrangler   (the script applies migrations itself)
# Requires: wrangler (TS repo's project-managed binary), worker-build, node.
# The admin seed password is fixed for local dev only:
#   login "admin", password "smoke-admin-2026"
# The local D1 lives in this workspace's .wrangler/ (never the TS repo's).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"

WRANGLER="$HOME/workspace/wordless/wordless-cms/node_modules/.bin/wrangler"
PORT="${SMOKE_PORT:-8800}"
BASE="http://127.0.0.1:$PORT"
EXT_PORT=8801
RUNAWAY_PORT=8802
TT25_PORT=8803
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

# derive_token <secret> <slug> <epoch> — the HMAC host token (see
# wordless-extensions/src/host.rs derive_host_token).
derive_token() {
  node -e 'const c=require("node:crypto");console.log(c.createHmac("sha256",process.argv[1]).update(`wordless-ext-host:v1:${process.argv[2]}:${process.argv[3]}`).digest("base64url"));' "$1" "$2" "$3"
}

echo "== P6 smoke: applying migrations (local D1) =="
"$WRANGLER" d1 migrations apply DB --local >/dev/null

echo "== P6 smoke: seeding admin (login admin / smoke-admin-2026) =="
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

# The extension host secret: random per run, passed as a plain var
# (opt_secret reads secret first, var second — same as the P3 mock secret).
EXT_SECRET="$(node -e 'console.log(require("node:crypto").randomBytes(32).toString("base64url"))')"
# Split (2026-11-20): conversion runs in the converter Worker — start
# it first, and point the main Worker at it.
source "$ROOT/scripts/converter-dev.sh"
start_converter
echo "== P6 smoke: starting wrangler dev on :$PORT (host secret set) =="
"$WRANGLER" dev --port "$PORT" --local --var "WORDLESS_EXT_HOST_SECRET:$EXT_SECRET" --var "WORDLESS_CONVERTER_URL:$CONVERTER_URL" >"$ROOT/.wrangler-p6-main.log" 2>&1 &
DEV_PID=$!
EXT_PID=""
RUNAWAY_PID=""
TT25_PID=""
cleanup() {
  # Kill the wrangler wrappers AND their whole trees: the workerd
  # child survives the npx wrapper, so killing the wrapper alone
  # orphans it (that left ports held after the first P6 run).
  for p in $EXT_PID $RUNAWAY_PID $TT25_PID $DEV_PID; do
    [ -n "$p" ] || continue
    kill "$p" 2>/dev/null || true
    for child in $(pgrep -P "$p" 2>/dev/null); do
      for grand in $(pgrep -P "$child" 2>/dev/null); do kill -9 "$grand" 2>/dev/null || true; done
      kill -9 "$child" 2>/dev/null || true
    done
  done
  stop_converter
  # Kill any wrangler dev parent by pattern first: a live parent
  # RESPAWNS workerd when it dies, so killing the listener alone is
  # futile (run 4's orphans proved it). The [r] bracket keeps pkill
  # from matching this script's own command line.
  for port in $PORT $EXT_PORT $RUNAWAY_PORT $TT25_PORT $CONVERTER_PORT; do
    pkill -f "wrangle[r] dev --port $port" 2>/dev/null || true
  done
  sleep 1
  # Backstop: anything still listening on our ports dies.
  for port in $PORT $EXT_PORT $RUNAWAY_PORT $TT25_PORT; do
    for pid in $(ss -tlnp 2>/dev/null | grep ":$port " | grep -o 'pid=[0-9]*' | cut -d= -f2 | sort -u); do
      kill -9 "$pid" 2>/dev/null || true
    done
  done
}
if [ "${1:-}" != "--keep" ]; then trap cleanup EXIT; fi

# Wait for the worker to answer (wrangler dev builds first — can take a while).
for i in $(seq 1 240); do
  if curl -fsS -o /dev/null "$BASE/api/health" 2>/dev/null; then break; fi
  sleep 2
done

echo "== P6 smoke: checks =="
TMP="$(mktemp -d)"

login() { # login <password> — sets $COOKIE on success
  curl -sS -o "$TMP/login.json" -w "%{http_code}" -D "$TMP/login.headers" -X POST "$BASE/api/auth/login" \
    -H 'Content-Type: application/json' -d "{\"login\":\"admin\",\"password\":\"$1\"}" > "$TMP/login.code"
  COOKIE=$(grep -i '^set-cookie:' "$TMP/login.headers" | head -1 | sed 's/^[^:]*: *//;s/;.*//' | tr -d '\r')
}

# --- health + login ---
curl -sS -o "$TMP/health.json" -w "%{http_code}" "$BASE/api/health" > "$TMP/health.code"
check "GET /api/health status" 200 "$(cat "$TMP/health.code")"
login smoke-admin-2026
check "POST /api/auth/login good password" 200 "$(cat "$TMP/login.code")"

# --- assemble_ext <slug> <source> <destdir> [only-file] ---
# Fetches the generated project from GET /api/extensions/:slug/script
# (with ?source=theme when asked), writes entry/shim/dts/wrangler,
# then downloads each converted file from local R2 (the key for a
# project path converted/<p>.ts is <prefix>/converted/<p>.ts).
# With [only-file] set, only that converted file is kept and the entry
# is trimmed to import just it (TT25: functions.php only — see header).
assemble_ext() {
  local slug="$1" source="$2" dest="$3" only="${4:-}"
  local query=""
  [ "$source" = "theme" ] && query="?source=theme"
  mkdir -p "$dest"
  curl -sS -o "$TMP/script-$slug.json" -H "Cookie: $COOKIE" "$BASE/api/extensions/$slug/script$query"
  node -e '
    const fs=require("fs");
    const d=JSON.parse(fs.readFileSync(process.argv[1],"utf8")).data;
    const dest=process.argv[2], only=process.argv[3]||"";
    fs.writeFileSync(dest+"/worker-entry.ts", d.entry);
    fs.writeFileSync(dest+"/shim.ts", d.shim);
    fs.writeFileSync(dest+"/wrangler.toml", d.wranglerToml);
    for (const [p,c] of (Array.isArray(d.wordpressDts) ? d.wordpressDts : Object.entries(d.wordpressDts||{}))) {
      fs.mkdirSync(dest+"/"+p.split("/").slice(0,-1).join("/"), {recursive:true});
      fs.writeFileSync(dest+"/"+p, c);
    }
    let paths=d.convertedPaths||[];
    if (only) {
      const keep="converted/"+only.replace(/\.php$/,".ts");
      paths=paths.filter((p)=>p===keep);
      if (paths.length!==1) { console.error("only-file not in convertedPaths: "+only); process.exit(1); }
      // Trim the entry to the kept file only: drop other imports and
      // their activate_N() calls, and rebuild the rendered-output
      // drain (the `before` array + rendered.push lines) for the kept
      // file alone. The kept file may or may not export renderedOutput.
      const keepIdx=d.convertedPaths.indexOf(keep);
      const keepHasDrain=d.entry.includes("renderedOutput_"+keepIdx);
      let entry=d.entry.split("\n").filter((l)=>{
        const m=l.match(/import \{ activate as activate_(\d+)/);
        if (m && Number(m[1])!==keepIdx) return false;
        if (/^activate_\d+\(host\.wp\);$/.test(l.trim()) && !l.includes("activate_"+keepIdx+"(")) return false;
        if (l.includes("const before = [")) return false;
        if (/rendered\.push\(\.\.\.renderedOutput_\d+\.slice\(before\[\d+\]\)\);/.test(l)) return false;
        return true;
      });
      entry=entry.map((l)=>{
        if (l.includes("const result = await host.runHook")) {
          const before = keepHasDrain ? "      const before = [renderedOutput_"+keepIdx+".length];" : "      const before = [];";
          return before + "\n" + l;
        }
        if (l.includes("const rendered: string[] = [];")) {
          const push = keepHasDrain ? "\n        rendered.push(...renderedOutput_"+keepIdx+".slice(before[0]));" : "";
          return l + push;
        }
        return l;
      });
      fs.writeFileSync(dest+"/worker-entry.ts", entry.join("\n"));
    }
    console.log(JSON.stringify(paths));
  ' "$TMP/script-$slug.json" "$dest" "$only" > "$TMP/paths-$slug.json"
  local prefix="extensions/$slug"
  [ "$source" = "theme" ] && prefix="themes/$slug"
  local rels
  rels="$(node -e '
    const fs=require("fs");
    const paths=JSON.parse(fs.readFileSync(process.argv[1],"utf8"));
    for (const p of paths) console.log(p.replace(/^converted\//,""));
  ' "$TMP/paths-$slug.json")"
  for rel in $rels; do
    mkdir -p "$dest/converted/$(dirname "$rel")"
    if ! "$WRANGLER" r2 object get "wordless-media-v2/$prefix/converted/$rel" --file "$dest/converted/$rel" --local >/dev/null 2>&1; then
      echo "  FAIL assemble $slug: could not download $prefix/converted/$rel from local R2"
      FAIL=$((FAIL+1))
    fi
  done
  echo "  (assembled $slug -> $dest, $(cat "$TMP/paths-$slug.json"))"
}

# --- start_ext <destdir> <port> <varname> ---
start_ext() {
  local dest="$1" port="$2"
  (cd "$dest" && "$WRANGLER" dev --port "$port" --local >"$dest/wrangler.log" 2>&1 & echo $! > "$dest/pid")
  for i in $(seq 1 120); do
    # --max-time: a zombie workerd from a previous run can LISTEN
    # without ever responding; without a timeout this curl hangs the
    # whole script forever (run 5 proved it).
    if curl -fsS --max-time 5 -o /dev/null "http://127.0.0.1:$port/__wordless/health" 2>/dev/null; then return 0; fi
    if grep -q "Address already in use" "$dest/wrangler.log" 2>/dev/null; then
      echo "  FAIL extension worker on :$port: address already in use (a previous run's workerd is still holding it — see $dest/wrangler.log)"
      FAIL=$((FAIL + 1))
      return 1
    fi
    sleep 2
  done
  echo "  FAIL extension worker on :$port never became healthy (see $dest/wrangler.log)"
  FAIL=$((FAIL + 1))
  return 1
}

# ================= hello-dolly end to end =================
echo "-- hello-dolly: install -> convert -> report -> load -> hook --"
HELLO_PHP="$HOME/workspace/wordless/reference/wordpress-7.1.5/wp-content/plugins/hello.php"
python3 - "$TMP/hello-dolly.zip" "$HELLO_PHP" <<'PYEOF'
import json, sys, zipfile
out, hello = sys.argv[1], sys.argv[2]
manifest = {"slug": "hello-dolly", "name": "Hello Dolly", "version": "1.7.2",
            "description": "Hello Dolly — the WordPress sample plugin."}
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
    z.writestr("hello-dolly/wordless.json", json.dumps(manifest))
    z.writestr("hello-dolly/hello.php", open(hello, encoding="utf-8").read())
PYEOF
curl -sS -o "$TMP/mkt-install.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/marketplace/install" \
  -F "file=@$TMP/hello-dolly.zip;filename=hello-dolly.zip;type=application/zip" > "$TMP/mkt-install.code"
check "POST /api/marketplace/install hello-dolly" 201 "$(cat "$TMP/mkt-install.code")"
check_contains "marketplace installed hello-dolly" "$TMP/mkt-install.json" '"slug":"hello-dolly"'
curl -sS -o "$TMP/mkt-report.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/marketplace/hello-dolly/report" > "$TMP/mkt-report.code"
check "GET /api/marketplace/hello-dolly/report" 200 "$(cat "$TMP/mkt-report.code")"
check_contains "hello-dolly report verdict compatible" "$TMP/mkt-report.json" '"verdict":"compatible"'

# Before load there is no instance: GET /:slug is 404 ("Load it
# first"); the list route shows the installed plugin as not loaded.
curl -sS -o "$TMP/ext-before.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/extensions/hello-dolly" > "$TMP/ext-before.code"
check "GET /api/extensions/hello-dolly before load" 404 "$(cat "$TMP/ext-before.code")"
check_contains "hello-dolly not loaded yet" "$TMP/ext-before.json" 'Load it first'
curl -sS -o "$TMP/ext-list.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/extensions/runtime" > "$TMP/ext-list.code"
check "GET /api/extensions/runtime lists hello-dolly" 200 "$(cat "$TMP/ext-list.code")"
check_contains "hello-dolly listed as not loaded" "$TMP/ext-list.json" '"slug":"hello-dolly"'
check_contains "runtime list is empty before load" "$TMP/ext-list.json" '"loaded":[]'
# The P3 route is intact: GET /api/extensions still returns the array
# of ExtensionRecord (the router returns the FIRST matching
# registration — a second GET /api/extensions in P6 would be dead code
# and break the Conversion page + the parity corpus step).
curl -sS -o "$TMP/ext-p3.json" -H "Cookie: $COOKIE" "$BASE/api/extensions"
P3_IS_ARRAY="$(node -e 'const j=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")); console.log(Array.isArray(j.data) ? "array" : "not-array");' "$TMP/ext-p3.json" 2>/dev/null || echo "unreadable")"
if [ "$P3_IS_ARRAY" = "array" ]; then PASS=$((PASS + 1)); echo "  ok  GET /api/extensions still returns the P3 array"; else FAIL=$((FAIL + 1)); echo "  FAIL GET /api/extensions still returns the P3 array (got $P3_IS_ARRAY)"; fi

curl -sS -o "$TMP/load.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/extensions/hello-dolly/load" \
  -H 'Content-Type: application/json' -d "{\"devUrl\":\"http://127.0.0.1:$EXT_PORT\"}" > "$TMP/load.code"
check "POST /api/extensions/hello-dolly/load" 200 "$(cat "$TMP/load.code")"
check_contains "hello-dolly loaded" "$TMP/load.json" '"loaded":true'
check_contains "hello-dolly load needs hooks.register" "$TMP/load.json" 'hooks.register'
curl -sS -o "$TMP/ext-list-after.json" -H "Cookie: $COOKIE" "$BASE/api/extensions/runtime"
check_contains "runtime list shows hello-dolly loaded" "$TMP/ext-list-after.json" '"slug":"hello-dolly"'
check_contains "runtime list loaded entry has status loaded" "$TMP/ext-list-after.json" '"status":"loaded"'
EPOCH="$(jget "$TMP/load.json" 'd.data.tokenEpoch')"
echo "  (hello-dolly token epoch: $EPOCH)"

assemble_ext hello-dolly extension "$TMP/ext-hello-dolly"
start_ext "$TMP/ext-hello-dolly" "$EXT_PORT" && EXT_PID="$(cat "$TMP/ext-hello-dolly/pid")"
curl -sS -o "$TMP/ext-health.json" -w "%{http_code}" "http://127.0.0.1:$EXT_PORT/__wordless/health" > "$TMP/ext-health.code"
check "GET extension /__wordless/health" 200 "$(cat "$TMP/ext-health.code")"
check_contains "extension health names hello-dolly" "$TMP/ext-health.json" '"slug":"hello-dolly"'

curl -sS -o "$TMP/invoke.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/extensions/hello-dolly/invoke" \
  -H 'Content-Type: application/json' -d '{"hook":"admin_notices","kind":"action"}' > "$TMP/invoke.code"
check "POST /api/extensions/hello-dolly/invoke admin_notices" 200 "$(cat "$TMP/invoke.code")"
check_contains "hello-dolly hook ran ok" "$TMP/invoke.json" '"status":"ok"'
check_contains "hello-dolly rendered the lyric" "$TMP/invoke.json" 'dolly'
check_contains "hello-dolly run recorded" "$TMP/invoke.json" '"rendered":['

# ================= host endpoint: token + capabilities =================
echo "-- host endpoint: token auth + capability gate --"
TOKEN="$(derive_token "$EXT_SECRET" hello-dolly "$EPOCH")"
# Wrong token -> 401.
curl -sS -o "$TMP/host-bad.json" -w "%{http_code}" -X POST "$BASE/api/extensions/hello-dolly/host" \
  -H "x-wordless-ext-token: wrong-token" -H 'Content-Type: application/json' \
  -d '{"call":"get_option","args":["blogname"],"actorUserId":1}' > "$TMP/host-bad.code"
check "POST host with wrong token" 401 "$(cat "$TMP/host-bad.code")"
# Right token, ungranted capability (hello-dolly has hooks.register+log
# only) -> 200 with capability_denied inside the HostResult.
curl -sS -o "$TMP/host-denied.json" -w "%{http_code}" -X POST "$BASE/api/extensions/hello-dolly/host" \
  -H "x-wordless-ext-token: $TOKEN" -H 'Content-Type: application/json' \
  -d '{"call":"get_option","args":["blogname"],"actorUserId":1}' > "$TMP/host-denied.code"
check "POST host get_option without options.read" 200 "$(cat "$TMP/host-denied.code")"
check_contains "host call denied by capability" "$TMP/host-denied.json" 'capability_denied'
# The denial is logged.
curl -sS -o "$TMP/logs.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/extensions/hello-dolly/logs" > "$TMP/logs.code"
check "GET /api/extensions/hello-dolly/logs" 200 "$(cat "$TMP/logs.code")"
check_contains "denial logged" "$TMP/logs.json" '"level":"denied"'
# Grant options.read, then the same call succeeds.
curl -sS -o "$TMP/perm.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/extensions/hello-dolly/permissions" \
  -H 'Content-Type: application/json' -d '{"permissions":["hooks.register","log","options.read"]}' > "$TMP/perm.code"
check "POST /api/extensions/hello-dolly/permissions" 200 "$(cat "$TMP/perm.code")"
curl -sS -o "$TMP/opt.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X PUT "$BASE/api/settings" \
  -H 'Content-Type: application/json' -d '{"siteTitle":"P6 Smoke Site"}' > "$TMP/opt.code"
check "PUT /api/settings (set blogname)" 200 "$(cat "$TMP/opt.code")"
curl -sS -o "$TMP/host-ok.json" -w "%{http_code}" -X POST "$BASE/api/extensions/hello-dolly/host" \
  -H "x-wordless-ext-token: $TOKEN" -H 'Content-Type: application/json' \
  -d '{"call":"get_option","args":["blogname"],"actorUserId":1}' > "$TMP/host-ok.code"
check "POST host get_option with options.read" 200 "$(cat "$TMP/host-ok.code")"
check_contains "host call returns the option" "$TMP/host-ok.json" 'P6 Smoke Site'

# ================= disable kills hooks within one request =================
echo "-- disable: hooks stop within one request --"
curl -sS -o "$TMP/disable.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/extensions/hello-dolly/disable" > "$TMP/disable.code"
check "POST /api/extensions/hello-dolly/disable" 200 "$(cat "$TMP/disable.code")"
curl -sS -o "$TMP/invoke-off.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/extensions/hello-dolly/invoke" \
  -H 'Content-Type: application/json' -d '{"hook":"admin_notices","kind":"action"}' > "$TMP/invoke-off.code"
check "POST invoke while disabled" 200 "$(cat "$TMP/invoke-off.code")"
check_contains "disabled extension is skipped" "$TMP/invoke-off.json" 'skipped_disabled'
check_contains "disabled extension renders nothing" "$TMP/invoke-off.json" '"rendered":[]'
curl -sS -o "$TMP/enable.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/extensions/hello-dolly/enable" > "$TMP/enable.code"
check "POST /api/extensions/hello-dolly/enable" 200 "$(cat "$TMP/enable.code")"
curl -sS -o "$TMP/invoke-on.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/extensions/hello-dolly/invoke" \
  -H 'Content-Type: application/json' -d '{"hook":"admin_notices","kind":"action"}' > "$TMP/invoke-on.code"
check "POST invoke after re-enable" 200 "$(cat "$TMP/invoke-on.code")"
check_contains "re-enabled extension runs again" "$TMP/invoke-on.json" '"status":"ok"'

# ================= hostile plugin: eval() is blocked =================
echo "-- hostile plugin: eval() blocked at analysis, refused at load --"
python3 - "$TMP/evil.zip" <<'PYEOF'
import json, sys, zipfile
manifest = {"slug": "evil-plugin", "name": "Evil Plugin", "version": "0.1.0"}
php = """<?php
/*
Plugin Name: Evil Plugin
*/
function evil_run() {
    eval('$x = 1;');
}
add_action('init', 'evil_run');
"""
with zipfile.ZipFile(sys.argv[1], "w", zipfile.ZIP_DEFLATED) as z:
    z.writestr("evil-plugin/wordless.json", json.dumps(manifest))
    z.writestr("evil-plugin/evil.php", php)
PYEOF
curl -sS -o "$TMP/evil-install.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/marketplace/install" \
  -F "file=@$TMP/evil.zip;filename=evil.zip;type=application/zip" > "$TMP/evil-install.code"
check "POST /api/marketplace/install evil-plugin" 201 "$(cat "$TMP/evil-install.code")"
curl -sS -o "$TMP/evil-report.json" -w "%{http_code}" -H "Cookie: $COOKIE" "$BASE/api/marketplace/evil-plugin/report" > "$TMP/evil-report.code"
check "GET /api/marketplace/evil-plugin/report" 200 "$(cat "$TMP/evil-report.code")"
check_contains "evil-plugin report verdict blocked" "$TMP/evil-report.json" '"verdict":"blocked"'
curl -sS -o "$TMP/evil-load.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/extensions/evil-plugin/load" \
  -H 'Content-Type: application/json' -d "{\"devUrl\":\"http://127.0.0.1:$EXT_PORT\"}" > "$TMP/evil-load.code"
check "POST /api/extensions/evil-plugin/load" 200 "$(cat "$TMP/evil-load.code")"
check_contains "evil-plugin load refused" "$TMP/evil-load.json" '"loaded":false'
check_contains "evil-plugin refusal names the file" "$TMP/evil-load.json" 'evil.php'

# ================= TT25 theme: converted functions execute =================
echo "-- TT25 theme: import -> load source=theme -> functions.php runs --"
TT25_DIR="$HOME/workspace/wordless/reference/wordpress-7.1.5/wp-content/themes/twentytwentyfive"
python3 - "$TMP/tt25.zip" "$TT25_DIR" <<'PYEOF'
import os, sys, zipfile
out, root = sys.argv[1], sys.argv[2]
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
    for dirpath, _dirs, files in os.walk(root):
        for f in files:
            p = os.path.join(dirpath, f)
            z.write(p, os.path.relpath(p, root))
PYEOF
curl -sS -o "$TMP/tt25-import.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/themes/import" \
  -F "file=@$TMP/tt25.zip;filename=twentytwentyfive.zip;type=application/zip" > "$TMP/tt25-import.code"
check "POST /api/themes/import twentytwentyfive" 201 "$(cat "$TMP/tt25-import.code")"
check_contains "TT25 imported" "$TMP/tt25-import.json" 'twentytwentyfive'
curl -sS -o "$TMP/tt25-load.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/extensions/twentytwentyfive/load" \
  -H 'Content-Type: application/json' -d "{\"source\":\"theme\",\"devUrl\":\"http://127.0.0.1:$TT25_PORT\"}" > "$TMP/tt25-load.code"
check "POST /api/extensions/twentytwentyfive/load source=theme" 200 "$(cat "$TMP/tt25-load.code")"
check_contains "TT25 theme loaded" "$TMP/tt25-load.json" '"loaded":true'
check_contains "TT25 load warns about partial files" "$TMP/tt25-load.json" 'partial'
check_contains "TT25 load grants theme.enqueue" "$TMP/tt25-load.json" 'theme.enqueue'
# The generated project lists all converted files (M11 record: 99).
# Defensive read: if /script failed (e.g. the load above was refused)
# the count is empty and the check below FAILs cleanly instead of the
# node eval crashing the whole script (which skipped the runaway
# section in the first re-run).
curl -sS -o "$TMP/script-tt25.json" -H "Cookie: $COOKIE" "$BASE/api/extensions/twentytwentyfive/script?source=theme"
TT25_COUNT="$(node -e 'const j=JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")); const d=j.data||{}; console.log((d.convertedPaths||[]).length);' "$TMP/script-tt25.json" 2>/dev/null || echo "")"
echo "  (TT25 convertedPaths: $TT25_COUNT)"
if [ "$TT25_COUNT" -ge 90 ]; then PASS=$((PASS + 1)); echo "  ok  TT25 script lists >=90 converted files ($TT25_COUNT)"; else FAIL=$((FAIL + 1)); echo "  FAIL TT25 script lists >=90 converted files (got $TT25_COUNT)"; fi
assemble_ext twentytwentyfive theme "$TMP/ext-tt25" "functions.php"
start_ext "$TMP/ext-tt25" "$TT25_PORT" && TT25_PID="$(cat "$TMP/ext-tt25/pid")"
curl -sS -o "$TMP/invoke-tt25.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/extensions/twentytwentyfive/invoke" \
  -H 'Content-Type: application/json' -d '{"hook":"after_setup_theme","kind":"action"}' > "$TMP/invoke-tt25.code"
check "POST /api/extensions/twentytwentyfive/invoke after_setup_theme" 200 "$(cat "$TMP/invoke-tt25.code")"
# TT25's first after_setup_theme callback calls add_theme_support(),
# which has no Wordless mapping: the converted function RUNS and the
# runtime reports the honest unmapped boundary as an error. (The
# __unmapped_* helper throws a readable error naming the function, not
# the __unmapped_ identifier itself.)
check_contains "TT25 callback ran to the unmapped boundary" "$TMP/invoke-tt25.json" 'Unmapped function add_theme_support()'
check_contains "TT25 invoke recorded as error" "$TMP/invoke-tt25.json" '"status":"error"'
# The enqueue callback runs too: it throws at the unconverted
# SCRIPT_DEBUG constant (line 18 of the converted file) before it
# reaches the wp_enqueue_style call — the function executes to the
# honest boundary, same as above. (The enqueue host arm itself is
# proven by the routes_p6 unit test, which calls it directly.)
curl -sS -o "$TMP/invoke-tt25-enqueue.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/extensions/twentytwentyfive/invoke" \
  -H 'Content-Type: application/json' -d '{"hook":"wp_enqueue_scripts","kind":"action"}' > "$TMP/invoke-tt25-enqueue.code"
check "POST /api/extensions/twentytwentyfive/invoke wp_enqueue_scripts" 200 "$(cat "$TMP/invoke-tt25-enqueue.code")"
check_contains "TT25 enqueue callback ran to the unmapped boundary" "$TMP/invoke-tt25-enqueue.json" 'constant SCRIPT_DEBUG is not defined'
check_contains "TT25 enqueue invoke recorded as error" "$TMP/invoke-tt25-enqueue.json" '"status":"error"'

# ================= runaway extension: timeout cannot take the Worker down =================
echo "-- runaway extension: 5,000 ms timeout, main Worker stays alive --"
mkdir -p "$TMP/ext-runaway"
cat > "$TMP/ext-runaway/worker-entry.ts" <<'EOF'
export default {
  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    if (url.pathname === "/__wordless/health" && request.method === "GET") {
      return Response.json({ ok: true, slug: "runaway", version: "0.1.0" });
    }
    if (url.pathname === "/__wordless/invoke" && request.method === "POST") {
      // Never responds — the main Worker must time this out.
      return new Promise(() => {});
    }
    return new Response("not found", { status: 404 });
  },
};
EOF
cat > "$TMP/ext-runaway/wrangler.toml" <<'EOF'
name = "runaway"
main = "worker-entry.ts"
compatibility_date = "2026-11-18"
EOF
start_ext "$TMP/ext-runaway" "$RUNAWAY_PORT" && RUNAWAY_PID="$(cat "$TMP/ext-runaway/pid")"
# Register the runaway instance directly in D1 (it has no converted
# code, so it cannot go through the load route — the point is the
# timeout, not the gate). The token hash must be the real derived
# token's SHA-256 hex, as record_instance stores it.
RUNAWAY_TOKEN="$(derive_token "$EXT_SECRET" runaway 0)"
# The `--` is load-bearing: the derived token is base64url and can
# start with `-`, which `node -e` would parse as an option and die
# with exit 9 ("bad option") — a ~1/64-per-run flake that killed this
# suite on the 2026-11-20 school-split verification run.
RUNAWAY_HASH="$(node -e 'console.log(require("node:crypto").createHash("sha256").update(process.argv[1]).digest("hex"));' -- "$RUNAWAY_TOKEN")"
RUNAWAY_SQL="$(mktemp)"
cat > "$RUNAWAY_SQL" <<SQL
INSERT INTO wl_extension_instances (slug, version, status, permissions_json, script_key, host_token_hash, token_epoch, dev_url, created_at, updated_at)
VALUES ('runaway', '0.1.0', 'loaded', '[]', '', '$RUNAWAY_HASH', 0, 'http://127.0.0.1:$RUNAWAY_PORT', '2026-11-19', '2026-11-19');
SQL
"$WRANGLER" d1 execute DB --local --file "$RUNAWAY_SQL" >/dev/null
rm -f "$RUNAWAY_SQL"
T0="$(date +%s)"
curl -sS -o "$TMP/invoke-runaway.json" -w "%{http_code}" -H "Cookie: $COOKIE" -X POST "$BASE/api/extensions/runaway/invoke" \
  -H 'Content-Type: application/json' -d '{"hook":"admin_notices","kind":"action"}' > "$TMP/invoke-runaway.code"
T1="$(date +%s)"
check "POST /api/extensions/runaway/invoke (never-responding)" 200 "$(cat "$TMP/invoke-runaway.code")"
# The runaway never resolves: local workerd detects the hung isolate
# and answers HTTP 500 itself, so the executor reports "error" (with
# the hang message) rather than the 5,000 ms client-side "timeout".
# Either outcome proves the point — the main Worker returns promptly
# and stays alive. The 5,000 ms race itself is unit-tested
# (race_with_timeout in wordless-extensions).
RUNAWAY_STATUS="$(jget "$TMP/invoke-runaway.json" 'd.data.perExtension[0].status')"
echo "  (runaway per-extension status: $RUNAWAY_STATUS)"
if [ "$RUNAWAY_STATUS" = "timeout" ] || [ "$RUNAWAY_STATUS" = "error" ]; then
  PASS=$((PASS + 1)); echo "  ok  runaway invoke did not run ($RUNAWAY_STATUS)"
else
  FAIL=$((FAIL + 1)); echo "  FAIL runaway invoke did not run (status: $RUNAWAY_STATUS)"
fi
ELAPSED=$((T1 - T0))
echo "  (runaway invoke wall time: ${ELAPSED}s, timeout is 5s)"
if [ "$ELAPSED" -lt 12 ]; then PASS=$((PASS + 1)); echo "  ok  runaway invoke returned promptly (${ELAPSED}s < 12s)"; else FAIL=$((FAIL + 1)); echo "  FAIL runaway invoke took ${ELAPSED}s (>= 12s)"; fi
curl -sS -o /dev/null -w "%{http_code}" "$BASE/api/health" > "$TMP/health-after.code"
check "GET /api/health after runaway (main Worker alive)" 200 "$(cat "$TMP/health-after.code")"

echo
echo "== P6 smoke: $PASS passed, $FAIL failed =="
[ "$FAIL" -eq 0 ]
