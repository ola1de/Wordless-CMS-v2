#!/usr/bin/env bash
# WORDLESS v2 P7 smoke test — social networking end to end.
#
# Runs the Rust worker under local wrangler dev (port 8804) against a
# local D1 (migrations applied, four seeded users) and checks:
#   profiles: own read/update, friends-only visibility (403 for
#     strangers, 200 for friends), directory search + privacy totals.
#   friends: request -> pending -> accept -> list; decline; self
#     request 400; block hides profile + directory, unblock restores.
#   activity: public/friends/group visibility in the feed, like ->
#     notification + likeCount, unlike, like on an invisible item 403,
#     @mention notification, empty post 400.
#   groups: private join request -> approve -> group activity visible
#     to members only (403 for non-members); public direct join;
#     hidden group unlisted + 404 for non-members, invite -> join;
#     ban refuses re-join.
#   messages: thread between friends, send -> unread 1 -> read -> 0;
#     non-participant 404; strangers (no friendship, no shared group)
#     403.
#   moderation: admin hide removes the item from every feed; the
#     admin list still shows it as hidden; a subscriber cannot hide
#     (403).
#   notifications: unread counts, mark-all-read -> 0.
#   public pages: /members, /members/:id, /groups, /activity render
#     for anonymous viewers with the same visibility rules.
#
# Usage: ./scripts/smoke-p7.sh [--keep]   (--keep leaves wrangler dev running)
# Clean room first: rm -rf .wrangler   (the script applies migrations itself)
# Requires: wrangler (TS repo's project-managed binary), worker-build, node.
# Seeded users (all password "smoke-social-2026"):
#   admin (id 1, administrator), ed (id 2, editor),
#   carol (id 3, subscriber), dave (id 4, subscriber, no groups/friends)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"

WRANGLER="$HOME/workspace/wordless/wordless-cms/node_modules/.bin/wrangler"
PORT="${SMOKE_PORT:-8804}"
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

echo "== P7 smoke: applying migrations (local D1) =="
"$WRANGLER" d1 migrations apply DB --local >/dev/null

echo "== P7 smoke: seeding users (admin/ed/carol/dave, password smoke-social-2026) =="
SEED_SQL="$(mktemp)"
node > "$SEED_SQL" <<'NODE'
const { pbkdf2Sync, randomBytes } = require('node:crypto');
const esc = (s) => "'" + s.replace(/'/g, "''") + "'";
const now = new Date().toISOString().replace('T', ' ').replace(/\.\d+Z$/, '');
const users = [
  ['admin', 'admin@example.com', 'admin', '{"administrator":true}'],
  ['ed', 'ed@example.com', 'Ed Editor', '{"editor":true}'],
  ['carol', 'carol@example.com', 'Carol', '{"subscriber":true}'],
  ['dave', 'dave@example.com', 'Dave', '{"subscriber":true}'],
];
for (const [login, email, display, caps] of users) {
  const salt = randomBytes(16);
  const h = pbkdf2Sync('smoke-social-2026', salt, 100000, 32, 'sha256');
  const stored = 'pbkdf2$100000$' + salt.toString('base64url') + '$' + h.toString('base64url');
  console.log('INSERT OR IGNORE INTO wp_users (user_login, user_pass, user_nicename, user_email, user_url, user_registered, display_name) VALUES (' + esc(login) + ', ' + esc(stored) + ', ' + esc(login) + ', ' + esc(email) + ', ' + esc('') + ', ' + esc(now) + ', ' + esc(display) + ');');
  console.log('INSERT OR IGNORE INTO wp_usermeta (user_id, meta_key, meta_value) VALUES ((SELECT ID FROM wp_users WHERE user_login = ' + esc(login) + '), ' + esc('wp_capabilities') + ', ' + esc(caps) + ');');
}
NODE
"$WRANGLER" d1 execute DB --local --file "$SEED_SQL" >/dev/null
rm -f "$SEED_SQL"

# Split (2026-11-20): conversion runs in the converter Worker — start
# it first, and point the main Worker at it. Size remedy (same day):
# social routes run in the social Worker — start it too.
source "$ROOT/scripts/converter-dev.sh"
start_converter
source "$ROOT/scripts/split-workers-dev.sh"
start_social
echo "== P7 smoke: starting wrangler dev on :$PORT =="
"$WRANGLER" dev --port "$PORT" --local --var "WORDLESS_CONVERTER_URL:$CONVERTER_URL" --var "WORDLESS_SOCIAL_URL:$SOCIAL_URL" >/tmp/smoke-p7-wrangler.log 2>&1 &
DEV_PID=$!
cleanup() {
  kill "$DEV_PID" 2>/dev/null || true
  for child in $(pgrep -P "$DEV_PID" 2>/dev/null); do kill "$child" 2>/dev/null || true; done
  stop_converter
  stop_social
}
trap cleanup EXIT

# Wait for the worker to answer (wrangler dev builds first — can take a while).
for i in $(seq 1 240); do
  if curl -fsS -o /dev/null "$BASE/api/health" 2>/dev/null; then break; fi
  sleep 2
done

echo "== P7 smoke: checks =="
TMP="$(mktemp -d)"

# login <login> <var-name> — logs in and stores the session cookie in $var-name.
login() {
  curl -sS -o "$TMP/login-$1.json" -w "%{http_code}" -D "$TMP/login-$1.headers" -X POST "$BASE/api/auth/login" \
    -H 'Content-Type: application/json' -d "{\"login\":\"$1\",\"password\":\"smoke-social-2026\"}" > "$TMP/login-$1.code"
  local cookie
  cookie=$(grep -i '^set-cookie:' "$TMP/login-$1.headers" | head -1 | sed 's/^[^:]*: *//;s/;.*//' | tr -d '\r' || true)
  printf -v "$2" '%s' "$cookie"
}

login admin ADMIN_COOKIE
check "login admin" 200 "$(cat "$TMP/login-admin.code")"
login ed ED_COOKIE
check "login ed" 200 "$(cat "$TMP/login-ed.code")"
login carol CAROL_COOKIE
check "login carol" 200 "$(cat "$TMP/login-carol.code")"
login dave DAVE_COOKIE
check "login dave" 200 "$(cat "$TMP/login-dave.code")"

# --- guards: anonymous cannot use the social API ---
curl -sS -o /dev/null -w "%{http_code}" "$BASE/api/social/members" > "$TMP/anon.code"
check "GET /api/social/members anon" 401 "$(cat "$TMP/anon.code")"

# --- profiles: own read/update; friends-only visibility ---
curl -sS -o "$TMP/prof-admin.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/social/profile" > "$TMP/prof-admin.code"
check "GET /api/social/profile (admin)" 200 "$(cat "$TMP/prof-admin.code")"
check_contains "own profile displayName" "$TMP/prof-admin.json" '"displayName":"admin"'
curl -sS -o "$TMP/prof-put.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/social/profile" \
  -H 'Content-Type: application/json' -d '{"bio":"Site owner","visibility":"friends"}' > "$TMP/prof-put.code"
check "PUT /api/social/profile (bio + friends visibility)" 200 "$(cat "$TMP/prof-put.code")"
check_contains "profile bio saved" "$TMP/prof-put.json" '"bio":"Site owner"'
# Stranger (ed, not yet a friend) cannot see a friends-only profile.
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ED_COOKIE" "$BASE/api/social/profile/1" > "$TMP/prof-ed.code"
check "GET /api/social/profile/1 as stranger (friends-only)" 403 "$(cat "$TMP/prof-ed.code")"

# --- members directory: search + privacy totals ---
curl -sS -o "$TMP/members.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/social/members?q=carol" > "$TMP/members.code"
check "GET /api/social/members?q=carol" 200 "$(cat "$TMP/members.code")"
check_contains "directory search finds carol" "$TMP/members.json" '"login":"carol"'
curl -sS -o "$TMP/members-all.json" -w "%{http_code}" -H "Cookie: $ED_COOKIE" "$BASE/api/social/members" > "$TMP/members-all.code"
check "GET /api/social/members (ed)" 200 "$(cat "$TMP/members-all.code")"
# Total is 3, not 4: admin's profile is friends-only and ed is not
# a friend yet, so admin is hidden from ed's directory.
[ "$(jget "$TMP/members-all.json" 'd.data.total')" = "3" ] && { PASS=$((PASS + 1)); echo "  ok  directory total is 3 (admin friends-only, ed not a friend yet)"; } || { FAIL=$((FAIL + 1)); echo "  FAIL directory total: got $(jget "$TMP/members-all.json" 'd.data.total'), expected 3"; }

# --- friends: request -> pending -> accept ---
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/social/friends/request" \
  -H 'Content-Type: application/json' -d '{"userId":2}' > "$TMP/freq.code"
check "POST /api/social/friends/request admin->ed" 200 "$(cat "$TMP/freq.code")"
curl -sS -o "$TMP/pending.json" -w "%{http_code}" -H "Cookie: $ED_COOKIE" "$BASE/api/social/friends?pending=1" > "$TMP/pending.code"
check "GET /api/social/friends?pending=1 (ed)" 200 "$(cat "$TMP/pending.code")"
[ "$(jget "$TMP/pending.json" 'd.data.length')" = "1" ] && { PASS=$((PASS + 1)); echo "  ok  ed has 1 pending request"; } || { FAIL=$((FAIL + 1)); echo "  FAIL ed pending count: got $(jget "$TMP/pending.json" 'd.data.length')"; }
curl -sS -o "$TMP/notif-ed.json" -w "%{http_code}" -H "Cookie: $ED_COOKIE" "$BASE/api/social/notifications" > "$TMP/notif-ed.code"
check "GET /api/social/notifications (ed)" 200 "$(cat "$TMP/notif-ed.code")"
[ "$(jget "$TMP/notif-ed.json" 'd.data.unreadCount')" -ge 1 ] 2>/dev/null && { PASS=$((PASS + 1)); echo "  ok  ed was notified of the friend request"; } || { FAIL=$((FAIL + 1)); echo "  FAIL ed unreadCount: got $(jget "$TMP/notif-ed.json" 'd.data.unreadCount')"; }
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ED_COOKIE" -X POST "$BASE/api/social/friends/accept" \
  -H 'Content-Type: application/json' -d '{"userId":1}' > "$TMP/faccept.code"
check "POST /api/social/friends/accept ed->admin" 200 "$(cat "$TMP/faccept.code")"
curl -sS -o "$TMP/friends.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/social/friends" > "$TMP/friends.code"
check "GET /api/social/friends (admin)" 200 "$(cat "$TMP/friends.code")"
check_contains "admin friends list has ed" "$TMP/friends.json" '"login":"ed"'
# Friends-only profile now visible to the friend, still hidden from strangers.
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ED_COOKIE" "$BASE/api/social/profile/1" > "$TMP/prof-ed2.code"
check "GET /api/social/profile/1 as friend" 200 "$(cat "$TMP/prof-ed2.code")"
curl -sS -o "$TMP/members-ed3.json" -w "%{http_code}" -H "Cookie: $ED_COOKIE" "$BASE/api/social/members" > "$TMP/members-ed3.code"
[ "$(jget "$TMP/members-ed3.json" 'd.data.total')" = "4" ] && { PASS=$((PASS + 1)); echo "  ok  directory total is 4 once ed is admin's friend"; } || { FAIL=$((FAIL + 1)); echo "  FAIL directory total after friending: got $(jget "$TMP/members-ed3.json" 'd.data.total'), expected 4"; }
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $CAROL_COOKIE" "$BASE/api/social/profile/1" > "$TMP/prof-carol.code"
check "GET /api/social/profile/1 as stranger (still friends-only)" 403 "$(cat "$TMP/prof-carol.code")"
# Decline flow + self-request guard.
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $CAROL_COOKIE" -X POST "$BASE/api/social/friends/request" \
  -H 'Content-Type: application/json' -d '{"userId":1}' > "$TMP/freq2.code"
check "POST /api/social/friends/request carol->admin" 200 "$(cat "$TMP/freq2.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/social/friends/decline" \
  -H 'Content-Type: application/json' -d '{"userId":3}' > "$TMP/fdecline.code"
check "POST /api/social/friends/decline admin->carol" 200 "$(cat "$TMP/fdecline.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/social/friends/request" \
  -H 'Content-Type: application/json' -d '{"userId":1}' > "$TMP/fself.code"
check "POST /api/social/friends/request to self" 400 "$(cat "$TMP/fself.code")"

# --- activity: visibility, like, mention ---
curl -sS -o "$TMP/act1.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/social/activity" \
  -H 'Content-Type: application/json' -d '{"content":"Hello community","visibility":"public"}' > "$TMP/act1.code"
check "POST /api/social/activity (public)" 201 "$(cat "$TMP/act1.code")"
ACT1_ID="$(jget "$TMP/act1.json" 'd.data.id')"
curl -sS -o "$TMP/act2.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/social/activity" \
  -H 'Content-Type: application/json' -d '{"content":"Friends only post","visibility":"friends"}' > "$TMP/act2.code"
check "POST /api/social/activity (friends)" 201 "$(cat "$TMP/act2.code")"
ACT2_ID="$(jget "$TMP/act2.json" 'd.data.id')"
curl -sS -o "$TMP/feed-ed.json" -w "%{http_code}" -H "Cookie: $ED_COOKIE" "$BASE/api/social/activity" > "$TMP/feed-ed.code"
check "GET /api/social/activity (ed, friend)" 200 "$(cat "$TMP/feed-ed.code")"
check_contains "friend sees public post" "$TMP/feed-ed.json" 'Hello community'
check_contains "friend sees friends-only post" "$TMP/feed-ed.json" 'Friends only post'
curl -sS -o "$TMP/feed-carol.json" -w "%{http_code}" -H "Cookie: $CAROL_COOKIE" "$BASE/api/social/activity" > "$TMP/feed-carol.code"
check "GET /api/social/activity (carol, stranger)" 200 "$(cat "$TMP/feed-carol.code")"
check_contains "stranger sees public post" "$TMP/feed-carol.json" 'Hello community'
check_not_contains "stranger does not see friends-only post" "$TMP/feed-carol.json" 'Friends only post'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/social/activity" \
  -H 'Content-Type: application/json' -d '{"content":""}' > "$TMP/act-empty.code"
check "POST /api/social/activity (empty)" 400 "$(cat "$TMP/act-empty.code")"
# Like: ed likes the public post; admin is notified; unlike clears it.
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ED_COOKIE" -X POST "$BASE/api/social/activity/$ACT1_ID/like" > "$TMP/like.code"
check "POST /api/social/activity/:id/like" 200 "$(cat "$TMP/like.code")"
curl -sS -o "$TMP/feed-ed2.json" -w "%{http_code}" -H "Cookie: $ED_COOKIE" "$BASE/api/social/activity" > "$TMP/feed-ed2.code"
[ "$(jget "$TMP/feed-ed2.json" 'd.data.find(i=>i.id==='$ACT1_ID').likeCount')" = "1" ] && { PASS=$((PASS + 1)); echo "  ok  likeCount is 1 after like"; } || { FAIL=$((FAIL + 1)); echo "  FAIL likeCount after like"; }
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $CAROL_COOKIE" -X POST "$BASE/api/social/activity/$ACT2_ID/like" > "$TMP/like-bad.code"
check "POST like on invisible friends-only item (carol)" 403 "$(cat "$TMP/like-bad.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ED_COOKIE" -X DELETE "$BASE/api/social/activity/$ACT1_ID/like" > "$TMP/unlike.code"
check "DELETE /api/social/activity/:id/like" 200 "$(cat "$TMP/unlike.code")"
# Mention: admin mentions ed; ed is notified.
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/social/activity" \
  -H 'Content-Type: application/json' -d '{"content":"Hey @ed, welcome!","visibility":"public"}' > "$TMP/mention.code"
check "POST /api/social/activity (mention @ed)" 201 "$(cat "$TMP/mention.code")"
curl -sS -o "$TMP/notif-ed2.json" -w "%{http_code}" -H "Cookie: $ED_COOKIE" "$BASE/api/social/notifications?unread=1" > "$TMP/notif-ed2.code"
check_contains "mention notification for ed" "$TMP/notif-ed2.json" 'activity_mention'

# --- groups: private request/approve, public join, hidden invite, ban ---
curl -sS -o "$TMP/grp1.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/social/groups" \
  -H 'Content-Type: application/json' -d '{"name":"Book Club","description":"Readers","visibility":"private"}' > "$TMP/grp1.code"
check "POST /api/social/groups (private Book Club)" 201 "$(cat "$TMP/grp1.code")"
GRP1_ID="$(jget "$TMP/grp1.json" 'd.data.id')"
curl -sS -o "$TMP/grps-carol.json" -w "%{http_code}" -H "Cookie: $CAROL_COOKIE" "$BASE/api/social/groups" > "$TMP/grps-carol.code"
check_contains "private group is listed for strangers" "$TMP/grps-carol.json" 'Book Club'
curl -sS -o "$TMP/join1.json" -w "%{http_code}" -H "Cookie: $CAROL_COOKIE" -X POST "$BASE/api/social/groups/$GRP1_ID/join" > "$TMP/join1.code"
check "POST /api/social/groups/:id/join (private -> requested)" 200 "$(cat "$TMP/join1.code")"
check_contains "join request recorded" "$TMP/join1.json" '"myStatus":"requested"'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $CAROL_COOKIE" "$BASE/api/social/groups/$GRP1_ID/activity" > "$TMP/gact-carol.code"
check "GET group activity as non-member" 403 "$(cat "$TMP/gact-carol.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/social/groups/$GRP1_ID/approve" \
  -H 'Content-Type: application/json' -d '{"userId":3}' > "$TMP/approve.code"
check "POST /api/social/groups/:id/approve" 200 "$(cat "$TMP/approve.code")"
curl -sS -o "$TMP/gact1.json" -w "%{http_code}" -H "Cookie: $CAROL_COOKIE" -X POST "$BASE/api/social/activity" \
  -H 'Content-Type: application/json' -d "{\"content\":\"My first book\",\"visibility\":\"group\",\"groupId\":$GRP1_ID}" > "$TMP/gact1.code"
check "POST group activity (carol, member)" 201 "$(cat "$TMP/gact1.code")"
curl -sS -o "$TMP/gact-admin.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/social/groups/$GRP1_ID/activity" > "$TMP/gact-admin.code"
check "GET group activity as member (admin)" 200 "$(cat "$TMP/gact-admin.code")"
check_contains "group activity visible to member" "$TMP/gact-admin.json" 'My first book'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ED_COOKIE" "$BASE/api/social/groups/$GRP1_ID/activity" > "$TMP/gact-ed.code"
check "GET group activity as non-member (ed)" 403 "$(cat "$TMP/gact-ed.code")"
# Public group: direct join.
curl -sS -o "$TMP/grp2.json" -w "%{http_code}" -H "Cookie: $ED_COOKIE" -X POST "$BASE/api/social/groups" \
  -H 'Content-Type: application/json' -d '{"name":"Open Mic","description":"Performers","visibility":"public"}' > "$TMP/grp2.code"
check "POST /api/social/groups (public Open Mic)" 201 "$(cat "$TMP/grp2.code")"
GRP2_ID="$(jget "$TMP/grp2.json" 'd.data.id')"
curl -sS -o "$TMP/join2.json" -w "%{http_code}" -H "Cookie: $CAROL_COOKIE" -X POST "$BASE/api/social/groups/$GRP2_ID/join" > "$TMP/join2.code"
check "POST join public group (direct member)" 200 "$(cat "$TMP/join2.code")"
check_contains "public join is immediate" "$TMP/join2.json" '"myStatus":"member"'
# Hidden group: unlisted + 404 for non-members, invite -> join.
curl -sS -o "$TMP/grp3.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/social/groups" \
  -H 'Content-Type: application/json' -d '{"name":"Hidden Room","description":"Secret","visibility":"hidden"}' > "$TMP/grp3.code"
check "POST /api/social/groups (hidden Hidden Room)" 201 "$(cat "$TMP/grp3.code")"
GRP3_ID="$(jget "$TMP/grp3.json" 'd.data.id')"
curl -sS -o "$TMP/grps-carol2.json" -w "%{http_code}" -H "Cookie: $CAROL_COOKIE" "$BASE/api/social/groups" > "$TMP/grps-carol2.code"
check_not_contains "hidden group not listed for strangers" "$TMP/grps-carol2.json" 'Hidden Room'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $CAROL_COOKIE" "$BASE/api/social/groups/$GRP3_ID" > "$TMP/grp3-carol.code"
check "GET hidden group as non-member" 404 "$(cat "$TMP/grp3-carol.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/social/groups/$GRP3_ID/invite" \
  -H 'Content-Type: application/json' -d '{"userId":3}' > "$TMP/invite.code"
check "POST invite to hidden group" 200 "$(cat "$TMP/invite.code")"
curl -sS -o "$TMP/join3.json" -w "%{http_code}" -H "Cookie: $CAROL_COOKIE" -X POST "$BASE/api/social/groups/$GRP3_ID/join" > "$TMP/join3.code"
check "POST join hidden group after invite" 200 "$(cat "$TMP/join3.code")"
check_contains "invite accepted as member" "$TMP/join3.json" '"myStatus":"member"'
# Ban: admin bans carol from Open Mic; re-join is refused.
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ED_COOKIE" -X POST "$BASE/api/social/groups/$GRP2_ID/ban" \
  -H 'Content-Type: application/json' -d '{"userId":3}' > "$TMP/ban.code"
check "POST ban from group (group admin)" 200 "$(cat "$TMP/ban.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $CAROL_COOKIE" -X POST "$BASE/api/social/groups/$GRP2_ID/join" > "$TMP/rejoin.code"
check "POST re-join after ban" 403 "$(cat "$TMP/rejoin.code")"

# --- messages: friends thread, unread, read; strangers refused ---
curl -sS -o "$TMP/thread.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/social/messages/threads" \
  -H 'Content-Type: application/json' -d '{"userId":2}' > "$TMP/thread.code"
check "POST /api/social/messages/threads (friends)" 200 "$(cat "$TMP/thread.code")"
THREAD_ID="$(jget "$TMP/thread.json" 'd.data.id')"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $DAVE_COOKIE" -X POST "$BASE/api/social/messages/threads" \
  -H 'Content-Type: application/json' -d '{"userId":1}' > "$TMP/thread-dave.code"
check "POST thread as stranger (dave->admin)" 403 "$(cat "$TMP/thread-dave.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/social/messages/threads/$THREAD_ID/messages" \
  -H 'Content-Type: application/json' -d '{"body":"Hi Ed, welcome!"}' > "$TMP/msg.code"
check "POST message in thread" 201 "$(cat "$TMP/msg.code")"
curl -sS -o "$TMP/threads-ed.json" -w "%{http_code}" -H "Cookie: $ED_COOKIE" "$BASE/api/social/messages/threads" > "$TMP/threads-ed.code"
check "GET threads (ed)" 200 "$(cat "$TMP/threads-ed.code")"
[ "$(jget "$TMP/threads-ed.json" 'd.data[0].unreadCount')" = "1" ] && { PASS=$((PASS + 1)); echo "  ok  unreadCount is 1 after message"; } || { FAIL=$((FAIL + 1)); echo "  FAIL unreadCount after message: got $(jget "$TMP/threads-ed.json" 'd.data[0].unreadCount')"; }
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ED_COOKIE" -X POST "$BASE/api/social/messages/threads/$THREAD_ID/read" > "$TMP/read.code"
check "POST mark thread read" 200 "$(cat "$TMP/read.code")"
curl -sS -o "$TMP/threads-ed2.json" -w "%{http_code}" -H "Cookie: $ED_COOKIE" "$BASE/api/social/messages/threads" > "$TMP/threads-ed2.code"
[ "$(jget "$TMP/threads-ed2.json" 'd.data[0].unreadCount')" = "0" ] && { PASS=$((PASS + 1)); echo "  ok  unreadCount is 0 after read"; } || { FAIL=$((FAIL + 1)); echo "  FAIL unreadCount after read: got $(jget "$TMP/threads-ed2.json" 'd.data[0].unreadCount')"; }
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $CAROL_COOKIE" "$BASE/api/social/messages/threads/$THREAD_ID" > "$TMP/thread-carol.code"
check "GET thread as non-participant" 404 "$(cat "$TMP/thread-carol.code")"

# --- block: ed blocks carol; carol vanishes from ed's world ---
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ED_COOKIE" -X POST "$BASE/api/social/friends/block" \
  -H 'Content-Type: application/json' -d '{"userId":3}' > "$TMP/block.code"
check "POST /api/social/friends/block ed->carol" 200 "$(cat "$TMP/block.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ED_COOKIE" "$BASE/api/social/profile/3" > "$TMP/prof-blocked.code"
check "GET blocked user profile" 404 "$(cat "$TMP/prof-blocked.code")"
curl -sS -o "$TMP/members-ed.json" -w "%{http_code}" -H "Cookie: $ED_COOKIE" "$BASE/api/social/members" > "$TMP/members-ed.code"
[ "$(jget "$TMP/members-ed.json" 'd.data.total')" = "3" ] && { PASS=$((PASS + 1)); echo "  ok  directory total is 3 with carol blocked"; } || { FAIL=$((FAIL + 1)); echo "  FAIL directory total after block: got $(jget "$TMP/members-ed.json" 'd.data.total')"; }
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ED_COOKIE" -X POST "$BASE/api/social/friends/unblock" \
  -H 'Content-Type: application/json' -d '{"userId":3}' > "$TMP/unblock.code"
check "POST /api/social/friends/unblock" 200 "$(cat "$TMP/unblock.code")"
curl -sS -o "$TMP/members-ed2.json" -w "%{http_code}" -H "Cookie: $ED_COOKIE" "$BASE/api/social/members" > "$TMP/members-ed2.code"
[ "$(jget "$TMP/members-ed2.json" 'd.data.total')" = "4" ] && { PASS=$((PASS + 1)); echo "  ok  directory total is 4 after unblock"; } || { FAIL=$((FAIL + 1)); echo "  FAIL directory total after unblock: got $(jget "$TMP/members-ed2.json" 'd.data.total')"; }

# --- moderation: hide removes from every feed; subscriber cannot hide ---
curl -sS -o "$TMP/spam.json" -w "%{http_code}" -H "Cookie: $ED_COOKIE" -X POST "$BASE/api/social/activity" \
  -H 'Content-Type: application/json' -d '{"content":"spam spam spam","visibility":"public"}' > "$TMP/spam.code"
check "POST spam activity (ed)" 201 "$(cat "$TMP/spam.code")"
SPAM_ID="$(jget "$TMP/spam.json" 'd.data.id')"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $CAROL_COOKIE" -X POST "$BASE/api/social/activity/$SPAM_ID/hide" > "$TMP/hide-carol.code"
check "POST hide as subscriber" 403 "$(cat "$TMP/hide-carol.code")"
curl -sS -o "$TMP/hide.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/social/activity/$SPAM_ID/hide" > "$TMP/hide.code"
check "POST hide as admin" 200 "$(cat "$TMP/hide.code")"
check_contains "hide marks item hidden" "$TMP/hide.json" '"status":"hidden"'
curl -sS -o "$TMP/feed-carol2.json" -w "%{http_code}" -H "Cookie: $CAROL_COOKIE" "$BASE/api/social/activity" > "$TMP/feed-carol2.code"
check_not_contains "hidden item gone from stranger feed" "$TMP/feed-carol2.json" 'spam spam spam'
curl -sS -o "$TMP/admin-act.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/social/admin/activity" > "$TMP/admin-act.code"
check "GET /api/social/admin/activity" 200 "$(cat "$TMP/admin-act.code")"
check_contains "admin list still shows hidden item" "$TMP/admin-act.json" 'spam spam spam'

# --- notifications: mark all read ---
curl -sS -o "$TMP/notif-admin.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/social/notifications" > "$TMP/notif-admin.code"
check "GET /api/social/notifications (admin)" 200 "$(cat "$TMP/notif-admin.code")"
[ "$(jget "$TMP/notif-admin.json" 'd.data.unreadCount')" -ge 1 ] 2>/dev/null && { PASS=$((PASS + 1)); echo "  ok  admin has unread notifications"; } || { FAIL=$((FAIL + 1)); echo "  FAIL admin unreadCount"; }
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/social/notifications/read" \
  -H 'Content-Type: application/json' -d '{}' > "$TMP/read-all.code"
check "POST mark all notifications read" 200 "$(cat "$TMP/read-all.code")"
curl -sS -o "$TMP/unread.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/social/notifications/unread-count" > "$TMP/unread.code"
check "GET unread-count after mark-all-read" 200 "$(cat "$TMP/unread.code")"
[ "$(jget "$TMP/unread.json" 'd.data.unreadCount')" = "0" ] && { PASS=$((PASS + 1)); echo "  ok  unreadCount is 0 after mark-all-read"; } || { FAIL=$((FAIL + 1)); echo "  FAIL unreadCount after mark-all-read"; }

# --- public pages (anonymous) ---
curl -sS -o "$TMP/pub-members.html" -w "%{http_code}" "$BASE/members" > "$TMP/pub-members.code"
check "GET /members (anon)" 200 "$(cat "$TMP/pub-members.code")"
check_contains "public directory lists Dave" "$TMP/pub-members.html" 'Dave'
curl -sS -o "$TMP/pub-member.html" -w "%{http_code}" "$BASE/members/4" > "$TMP/pub-member.code"
check "GET /members/4 (anon)" 200 "$(cat "$TMP/pub-member.code")"
check_contains "public profile shows Dave" "$TMP/pub-member.html" 'Dave'
curl -sS -o "$TMP/pub-groups.html" -w "%{http_code}" "$BASE/groups" > "$TMP/pub-groups.code"
check "GET /groups (anon)" 200 "$(cat "$TMP/pub-groups.code")"
check_contains "public groups list has Open Mic" "$TMP/pub-groups.html" 'Open Mic'
check_not_contains "public groups list hides Hidden Room" "$TMP/pub-groups.html" 'Hidden Room'
curl -sS -o "$TMP/pub-activity.html" -w "%{http_code}" "$BASE/activity" > "$TMP/pub-activity.code"
check "GET /activity (anon)" 200 "$(cat "$TMP/pub-activity.code")"
check_contains "public activity shows public post" "$TMP/pub-activity.html" 'Hello community'
check_not_contains "public activity hides friends-only post" "$TMP/pub-activity.html" 'Friends only post'

echo
echo "== P7 smoke: $PASS passed, $FAIL failed =="
rm -rf "$TMP"
[ "$FAIL" -eq 0 ]
