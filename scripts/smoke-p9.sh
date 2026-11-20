#!/usr/bin/env bash
# WORDLESS v2 P9 smoke test — School portal + CBT end to end.
#
# Runs the Rust worker under local wrangler dev (port 8810) against a
# local D1 (migrations applied, one seeded admin) and checks:
#   courses: create (draft), draft invisible to students (404),
#     publish, lessons CRUD + progress (1/2, then completed).
#   enrolment: enrol 201, duplicate enrol 409, non-enrolled student
#     cannot start an exam (403).
#   question bank: 4 types created, bad mcq (1 option) 400,
#     true/false options forced to ["True","False"].
#   exams: create + publish, window enforcement (not-open 409,
#     closed 409), empty exam start 400.
#   attempt: start 201 with NO answer keys in the payload, resume
#     returns the same attempt, answers save, submit scores
#     automatically (mcq 5 + tf 2 + multi-select partial 2 = 9/11,
#     81.82%, passed), certificate auto-issued, leaderboard ranks.
#   attempt limit: second start 409 attempt_limit.
#   manual grading: short-answer exam submits to awaiting_grading,
#     result 404 while grading, queue lists the item (student 403),
#     grade full marks -> graded, result + certificate appear.
#   late submit: deadline forced into the past via D1 -> submit 409
#     attempt_expired, but the attempt IS auto-submitted (status
#     expired, result exists with saved answers scored); a late
#     answer save is 409 too.
#   public pages: /school lists the course, /school/courses/:slug
#     renders, /school/certificates/:code renders the certificate.
#   delete guard: course with enrolments 409 course_in_use.
#
# Usage: ./scripts/smoke-p9.sh [--keep]   (--keep leaves wrangler dev running)
# Clean room first: rm -rf .wrangler   (the script applies migrations itself)
# Requires: wrangler (TS repo's project-managed binary), worker-build, node.
# Seeded admin (password "smoke-admin-2026"): login "admin".
# School lives in its own Worker now (wordless-school-worker) —
# start it first and point the main Worker at it, exactly like the
# social/Woo Workers in smoke-p7.sh / smoke-p8.sh. The school Worker
# shares the main Worker's local D1 (--persist-to), so users seeded
# through the main API are visible to it through the proxy.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"

WRANGLER="$HOME/workspace/wordless/wordless-cms/node_modules/.bin/wrangler"
PORT="${SMOKE_PORT:-8810}"
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

echo "== P9 smoke: applying migrations (local D1) =="
"$WRANGLER" d1 migrations apply DB --local >/dev/null

echo "== P9 smoke: seeding admin (password smoke-admin-2026) =="
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

echo "== P9 smoke: starting school Worker (port 8809) =="
source "$ROOT/scripts/split-workers-dev.sh"
start_school

echo "== P9 smoke: starting wrangler dev on :$PORT =="
"$WRANGLER" dev --port "$PORT" --local --var "WORDLESS_SCHOOL_URL:$SCHOOL_URL" >/tmp/smoke-p9-wrangler.log 2>&1 &
DEV_PID=$!
cleanup() {
  kill "$DEV_PID" 2>/dev/null || true
  for child in $(pgrep -P "$DEV_PID" 2>/dev/null); do kill "$child" 2>/dev/null || true; done
  stop_school
}
trap cleanup EXIT

# Wait for the worker to answer (wrangler dev builds first — can take a while).
for i in $(seq 1 240); do
  if curl -fsS -o /dev/null "$BASE/api/health" 2>/dev/null; then break; fi
  sleep 2
done

echo "== P9 smoke: checks =="
TMP="$(mktemp -d)"

# login helper: logs in and prints the session cookie.
login_cookie() { # login_cookie <login> <password> <outfile>
  curl -sS -o "$TMP/login-$1.json" -D "$TMP/login-$1.headers" -X POST "$BASE/api/auth/login" \
    -H 'Content-Type: application/json' -d "{\"login\":\"$1\",\"password\":\"$2\"}" >/dev/null
  grep -i '^set-cookie:' "$TMP/login-$1.headers" | head -1 | sed -e 's/^[^:]*: *//' -e 's/;.*//' | tr -d '\r'
}

curl -sS -o "$TMP/login.json" -w "%{http_code}" -D "$TMP/login.headers" -X POST "$BASE/api/auth/login" \
  -H 'Content-Type: application/json' -d '{"login":"admin","password":"smoke-admin-2026"}' > "$TMP/login.code"
check "login admin" 200 "$(cat "$TMP/login.code")"
ADMIN_COOKIE="$(grep -i '^set-cookie:' "$TMP/login.headers" | head -1 | sed -e 's/^[^:]*: *//' -e 's/;.*//' | tr -d '\r')"

# --- users: a student and an outsider (both subscribers) ---
curl -sS -o "$TMP/student.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/users" \
  -H 'Content-Type: application/json' -d '{"login":"student","email":"student@example.test","password":"smoke-student-2026","displayName":"Student","role":"subscriber"}' > "$TMP/student.code"
check "POST /api/users student" 201 "$(cat "$TMP/student.code")"
curl -sS -o "$TMP/outsider.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/users" \
  -H 'Content-Type: application/json' -d '{"login":"outsider","email":"outsider@example.test","password":"smoke-student-2026","displayName":"Outsider","role":"subscriber"}' > "$TMP/outsider.code"
check "POST /api/users outsider" 201 "$(cat "$TMP/outsider.code")"
STUDENT_COOKIE="$(login_cookie student smoke-student-2026 x)"
OUTSIDER_COOKIE="$(login_cookie outsider smoke-student-2026 x)"
[ -n "$STUDENT_COOKIE" ] && { PASS=$((PASS + 1)); echo "  ok  student login sets a cookie"; } || { FAIL=$((FAIL + 1)); echo "  FAIL student login: no cookie"; }

# --- guards: anonymous cannot create, student cannot manage ---
curl -sS -o /dev/null -w "%{http_code}" -X POST "$BASE/api/school/courses" \
  -H 'Content-Type: application/json' -d '{"title":"Nope"}' > "$TMP/anon.code"
check "POST /api/school/courses anon" 401 "$(cat "$TMP/anon.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/courses" \
  -H 'Content-Type: application/json' -d '{"title":"Nope"}' > "$TMP/stu-create.code"
check "POST /api/school/courses as student" 403 "$(cat "$TMP/stu-create.code")"

# --- course: draft first, invisible to the student, then publish ---
curl -sS -o "$TMP/course.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/school/courses" \
  -H 'Content-Type: application/json' -d '{"title":"Rust 101","description":"Learn Rust."}' > "$TMP/course.code"
check "POST /api/school/courses Rust 101" 201 "$(cat "$TMP/course.code")"
check_contains "course starts as draft" "$TMP/course.json" '"status":"draft"'
COURSE_ID="$(jget "$TMP/course.json" 'd.data.id')"
COURSE_SLUG="$(jget "$TMP/course.json" 'd.data.slug')"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" "$BASE/api/school/courses/$COURSE_ID" > "$TMP/draft.code"
check "GET draft course as student" 404 "$(cat "$TMP/draft.code")"
curl -sS -o "$TMP/course-pub.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/school/courses/$COURSE_ID" \
  -H 'Content-Type: application/json' -d '{"status":"published"}' > "$TMP/course-pub.code"
check "PUT /api/school/courses/:id publish" 200 "$(cat "$TMP/course-pub.code")"
check_contains "course is published" "$TMP/course-pub.json" '"status":"published"'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" "$BASE/api/school/courses/$COURSE_ID" > "$TMP/pub.code"
check "GET published course as student" 200 "$(cat "$TMP/pub.code")"
curl -sS -o "$TMP/courses.json" -w "%{http_code}" "$BASE/api/school/courses" > "$TMP/courses.code"
check "GET /api/school/courses anon" 200 "$(cat "$TMP/courses.code")"
check_contains "public list has Rust 101" "$TMP/courses.json" 'Rust 101'

# --- lessons + enrolment + progress ---
curl -sS -o "$TMP/lesson1.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/school/courses/$COURSE_ID/lessons" \
  -H 'Content-Type: application/json' -d '{"title":"Intro","content":"Welcome.","durationMinutes":5}' > "$TMP/lesson1.code"
check "POST lesson Intro" 201 "$(cat "$TMP/lesson1.code")"
L1="$(jget "$TMP/lesson1.json" 'd.data.id')"
curl -sS -o "$TMP/lesson2.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/school/courses/$COURSE_ID/lessons" \
  -H 'Content-Type: application/json' -d '{"title":"Basics","content":"Content.","durationMinutes":10}' > "$TMP/lesson2.code"
check "POST lesson Basics" 201 "$(cat "$TMP/lesson2.code")"
L2="$(jget "$TMP/lesson2.json" 'd.data.id')"
curl -sS -o "$TMP/enrol.json" -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/courses/$COURSE_ID/enrol" \
  -H 'Content-Type: application/json' -d '{}' > "$TMP/enrol.code"
check "POST /api/school/courses/:id/enrol" 201 "$(cat "$TMP/enrol.code")"
check_contains "enrolment is active" "$TMP/enrol.json" '"status":"active"'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/courses/$COURSE_ID/enrol" \
  -H 'Content-Type: application/json' -d '{}' > "$TMP/enrol2.code"
check "POST enrol again" 409 "$(cat "$TMP/enrol2.code")"
curl -sS -o "$TMP/prog1.json" -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/lessons/$L1/complete" \
  -H 'Content-Type: application/json' -d '{}' > "$TMP/prog1.code"
check "POST /api/school/lessons/:id/complete (1/2)" 200 "$(cat "$TMP/prog1.code")"
check_contains "progress 1 of 2" "$TMP/prog1.json" '"lessonsCompleted":1'
curl -sS -o "$TMP/prog2.json" -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/lessons/$L2/complete" \
  -H 'Content-Type: application/json' -d '{}' > "$TMP/prog2.code"
check "POST /api/school/lessons/:id/complete (2/2)" 200 "$(cat "$TMP/prog2.code")"
check_contains "course completed" "$TMP/prog2.json" '"status":"completed"'

# --- question bank: 4 types, validation, true/false options forced ---
curl -sS -o "$TMP/bank.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/school/courses/$COURSE_ID/banks" \
  -H 'Content-Type: application/json' -d '{"title":"Week 1"}' > "$TMP/bank.code"
check "POST /api/school/courses/:id/banks" 201 "$(cat "$TMP/bank.code")"
BANK_ID="$(jget "$TMP/bank.json" 'd.data.id')"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/school/banks/$BANK_ID/questions" \
  -H 'Content-Type: application/json' -d '{"type":"mcq","questionText":"One option?","options":["only"],"answer":0,"marks":1}' > "$TMP/q-bad.code"
check "POST mcq with 1 option" 400 "$(cat "$TMP/q-bad.code")"
curl -sS -o "$TMP/q1.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/school/banks/$BANK_ID/questions" \
  -H 'Content-Type: application/json' -d '{"type":"mcq","questionText":"What is 2 + 2?","options":["3","4","5"],"answer":1,"marks":5,"difficulty":"easy","topic":"arithmetic"}' > "$TMP/q1.code"
check "POST mcq question" 201 "$(cat "$TMP/q1.code")"
Q1="$(jget "$TMP/q1.json" 'd.data.id')"
curl -sS -o "$TMP/q2.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/school/banks/$BANK_ID/questions" \
  -H 'Content-Type: application/json' -d '{"type":"true_false","questionText":"Rust is memory-safe.","options":["Yes","No","Maybe"],"answer":true,"marks":2}' > "$TMP/q2.code"
check "POST true/false question" 201 "$(cat "$TMP/q2.code")"
check_contains "true/false options forced" "$TMP/q2.json" '"options":["True","False"]'
Q2="$(jget "$TMP/q2.json" 'd.data.id')"
curl -sS -o "$TMP/q3.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/school/banks/$BANK_ID/questions" \
  -H 'Content-Type: application/json' -d '{"type":"multi_select","questionText":"Which are prime?","options":["2","4","5"],"answer":[0,2],"marks":4}' > "$TMP/q3.code"
check "POST multi-select question" 201 "$(cat "$TMP/q3.code")"
Q3="$(jget "$TMP/q3.json" 'd.data.id')"
curl -sS -o "$TMP/q4.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/school/banks/$BANK_ID/questions" \
  -H 'Content-Type: application/json' -d '{"type":"short_answer","questionText":"What is the capital of Nigeria?","modelAnswer":"Abuja","marks":5}' > "$TMP/q4.code"
check "POST short-answer question" 201 "$(cat "$TMP/q4.code")"
Q4="$(jget "$TMP/q4.json" 'd.data.id')"
curl -sS -o "$TMP/qlist.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/school/banks/$BANK_ID/questions" > "$TMP/qlist.code"
check "GET /api/school/banks/:id/questions" 200 "$(cat "$TMP/qlist.code")"
check_contains "instructor read carries the key" "$TMP/qlist.json" '"answer":1'

# --- exam 1: objective only (mcq + tf + multi-select), 1 attempt, pass 50 ---
curl -sS -o "$TMP/exam.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/school/courses/$COURSE_ID/exams" \
  -H 'Content-Type: application/json' -d "{\"bankId\":$BANK_ID,\"title\":\"Week 1 Quiz\",\"durationMinutes\":30,\"maxAttempts\":1,\"passMarkPercent\":50,\"questionIds\":[$Q1,$Q2,$Q3]}" > "$TMP/exam.code"
check "POST /api/school/courses/:id/exams" 201 "$(cat "$TMP/exam.code")"
EXAM_ID="$(jget "$TMP/exam.json" 'd.data.id')"
curl -sS -o "$TMP/exam-pub.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/school/exams/$EXAM_ID" \
  -H 'Content-Type: application/json' -d '{"status":"published"}' > "$TMP/exam-pub.code"
check "PUT /api/school/exams/:id publish" 200 "$(cat "$TMP/exam-pub.code")"
# Non-enrolled student, published exam -> 403 (enrolment denial, not a
# 404: the exam IS visible, the student just may not sit it).
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $OUTSIDER_COOKIE" -X POST "$BASE/api/school/exams/$EXAM_ID/start" \
  -H 'Content-Type: application/json' -d '{}' > "$TMP/outsider-start.code"
check "POST start as non-enrolled outsider" 403 "$(cat "$TMP/outsider-start.code")"
curl -sS -o "$TMP/attempt.json" -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/exams/$EXAM_ID/start" \
  -H 'Content-Type: application/json' -d '{}' > "$TMP/attempt.code"
check "POST /api/school/exams/:id/start" 201 "$(cat "$TMP/attempt.code")"
check_contains "attempt carries 3 questions" "$TMP/attempt.json" '"questionText":"What is 2 + 2?"'
check_not_contains "start payload has no answer keys" "$TMP/attempt.json" '"answer":1'
check_not_contains "start payload has no model answers" "$TMP/attempt.json" 'Abuja'
ATTEMPT_ID="$(jget "$TMP/attempt.json" 'd.data.id')"
# Resume returns the same attempt.
curl -sS -o "$TMP/resume.json" -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/exams/$EXAM_ID/start" \
  -H 'Content-Type: application/json' -d '{}' > "$TMP/resume.code"
check "POST start again (resume)" 201 "$(cat "$TMP/resume.code")"
[ "$(jget "$TMP/resume.json" 'd.data.id')" = "$ATTEMPT_ID" ] && { PASS=$((PASS + 1)); echo "  ok  resume returns the same attempt id"; } || { FAIL=$((FAIL + 1)); echo "  FAIL resume id: got $(jget "$TMP/resume.json" 'd.data.id'), expected $ATTEMPT_ID"; }
# A bad answer shape is a 400, not a crash.
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/attempts/$ATTEMPT_ID/answers" \
  -H 'Content-Type: application/json' -d "{\"questionId\":$Q1,\"answer\":\"one\"}" > "$TMP/ans-bad.code"
check "POST answer with bad shape" 400 "$(cat "$TMP/ans-bad.code")"
# mcq correct (5), tf correct (2), multi-select 1 of 2 correct (2) = 9/11.
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/attempts/$ATTEMPT_ID/answers" \
  -H 'Content-Type: application/json' -d "{\"questionId\":$Q1,\"answer\":1}" > "$TMP/ans1.code"
check "POST answer mcq correct" 200 "$(cat "$TMP/ans1.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/attempts/$ATTEMPT_ID/answers" \
  -H 'Content-Type: application/json' -d "{\"questionId\":$Q2,\"answer\":true}" > "$TMP/ans2.code"
check "POST answer tf correct" 200 "$(cat "$TMP/ans2.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/attempts/$ATTEMPT_ID/answers" \
  -H 'Content-Type: application/json' -d "{\"questionId\":$Q3,\"answer\":[0]}" > "$TMP/ans3.code"
check "POST answer multi-select partial" 200 "$(cat "$TMP/ans3.code")"
curl -sS -o "$TMP/submit.json" -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/attempts/$ATTEMPT_ID/submit" \
  -H 'Content-Type: application/json' -d '{}' > "$TMP/submit.code"
check "POST /api/school/attempts/:id/submit" 200 "$(cat "$TMP/submit.code")"
check_contains "attempt submitted" "$TMP/submit.json" '"status":"submitted"'
check_contains "score 9 of 11" "$TMP/submit.json" '"score":9'
curl -sS -o "$TMP/result.json" -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" "$BASE/api/school/attempts/$ATTEMPT_ID/result" > "$TMP/result.code"
check "GET /api/school/attempts/:id/result" 200 "$(cat "$TMP/result.code")"
check_contains "result 81.82%" "$TMP/result.json" '"percent":81.82'
check_contains "result passed" "$TMP/result.json" '"passed":true'
check_contains "certificate issued" "$TMP/result.json" '"certificateCode":"WL-'
CERT_CODE="$(jget "$TMP/result.json" 'd.data.certificateCode')"
# Per-question review on the attempt detail.
curl -sS -o "$TMP/detail.json" -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" "$BASE/api/school/attempts/$ATTEMPT_ID" > "$TMP/detail.code"
check_contains "review marks mcq correct" "$TMP/detail.json" '"marksAwarded":5'
# Leaderboard ranks by score.
curl -sS -o "$TMP/board.json" -w "%{http_code}" "$BASE/api/school/exams/$EXAM_ID/leaderboard" > "$TMP/board.code"
check "GET /api/school/exams/:id/leaderboard" 200 "$(cat "$TMP/board.code")"
check_contains "leaderboard has the student" "$TMP/board.json" '"login":"student"'
check_contains "leaderboard score 9" "$TMP/board.json" '"score":9'
# Second attempt is blocked (maxAttempts = 1).
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/exams/$EXAM_ID/start" \
  -H 'Content-Type: application/json' -d '{}' > "$TMP/second.code"
check "POST start second attempt" 409 "$(cat "$TMP/second.code")"
# Certificate verifies by code (API + public page).
curl -sS -o "$TMP/cert.json" -w "%{http_code}" "$BASE/api/school/certificates/$CERT_CODE" > "$TMP/cert.code"
check "GET /api/school/certificates/:code" 200 "$(cat "$TMP/cert.code")"
check_contains "certificate names the student" "$TMP/cert.json" '"studentLogin":"student"'
curl -sS -o "$TMP/cert.html" -w "%{http_code}" "$BASE/school/certificates/$CERT_CODE" > "$TMP/cert-html.code"
check "GET /school/certificates/:code" 200 "$(cat "$TMP/cert-html.code")"
check_contains "certificate page renders" "$TMP/cert.html" 'Certificate of Completion'
check_contains "certificate page has print CSS" "$TMP/cert.html" '@media print'
# Public pages.
curl -sS -o "$TMP/school.html" -w "%{http_code}" "$BASE/school" > "$TMP/school.code"
check "GET /school" 200 "$(cat "$TMP/school.code")"
check_contains "catalogue lists Rust 101" "$TMP/school.html" 'Rust 101'
curl -sS -o "$TMP/course-page.html" -w "%{http_code}" "$BASE/school/courses/$COURSE_SLUG" > "$TMP/course-page.code"
check "GET /school/courses/:slug" 200 "$(cat "$TMP/course-page.code")"
check_contains "course page lists lessons" "$TMP/course-page.html" 'Basics'

# --- exam 2: short-answer only, manual grading ---
curl -sS -o "$TMP/exam2.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/school/courses/$COURSE_ID/exams" \
  -H 'Content-Type: application/json' -d "{\"bankId\":$BANK_ID,\"title\":\"Capital Quiz\",\"durationMinutes\":30,\"maxAttempts\":3,\"passMarkPercent\":50,\"questionIds\":[$Q4]}" > "$TMP/exam2.code"
check "POST short-answer exam" 201 "$(cat "$TMP/exam2.code")"
EXAM2_ID="$(jget "$TMP/exam2.json" 'd.data.id')"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/school/exams/$EXAM2_ID" \
  -H 'Content-Type: application/json' -d '{"status":"published"}' > "$TMP/exam2-pub.code"
check "PUT short-answer exam publish" 200 "$(cat "$TMP/exam2-pub.code")"
curl -sS -o "$TMP/attempt2.json" -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/exams/$EXAM2_ID/start" \
  -H 'Content-Type: application/json' -d '{}' > "$TMP/attempt2.code"
check "POST start short-answer exam" 201 "$(cat "$TMP/attempt2.code")"
ATTEMPT2_ID="$(jget "$TMP/attempt2.json" 'd.data.id')"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/attempts/$ATTEMPT2_ID/answers" \
  -H 'Content-Type: application/json' -d "{\"questionId\":$Q4,\"answer\":\"Abuja\"}" > "$TMP/ans4.code"
check "POST short answer Abuja" 200 "$(cat "$TMP/ans4.code")"
curl -sS -o "$TMP/submit2.json" -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/attempts/$ATTEMPT2_ID/submit" \
  -H 'Content-Type: application/json' -d '{}' > "$TMP/submit2.code"
check "POST submit short-answer exam" 200 "$(cat "$TMP/submit2.code")"
check_contains "attempt awaits grading" "$TMP/submit2.json" '"status":"awaiting_grading"'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" "$BASE/api/school/attempts/$ATTEMPT2_ID/result" > "$TMP/result2.code"
check "GET result while grading" 404 "$(cat "$TMP/result2.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" "$BASE/api/school/grading-queue" > "$TMP/queue-stu.code"
check "GET grading queue as student" 403 "$(cat "$TMP/queue-stu.code")"
curl -sS -o "$TMP/queue.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" "$BASE/api/school/grading-queue" > "$TMP/queue.code"
check "GET /api/school/grading-queue" 200 "$(cat "$TMP/queue.code")"
check_contains "queue has the short answer" "$TMP/queue.json" 'capital of Nigeria'
check_contains "queue carries the model answer" "$TMP/queue.json" 'Abuja'
ANSWER_ID="$(jget "$TMP/queue.json" 'd.data.items[0].answerId')"
curl -sS -o "$TMP/grade.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/school/answers/$ANSWER_ID/grade" \
  -H 'Content-Type: application/json' -d '{"marksAwarded":5,"feedback":"Correct."}' > "$TMP/grade.code"
check "POST /api/school/answers/:id/grade full marks" 200 "$(cat "$TMP/grade.code")"
check_contains "graded answer is correct" "$TMP/grade.json" '"isCorrect":true'
curl -sS -o "$TMP/attempt2b.json" -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" "$BASE/api/school/attempts/$ATTEMPT2_ID" > "$TMP/attempt2b.code"
check_contains "attempt graded after manual grading" "$TMP/attempt2b.json" '"status":"graded"'
curl -sS -o "$TMP/result2b.json" -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" "$BASE/api/school/attempts/$ATTEMPT2_ID/result" > "$TMP/result2b.code"
check "GET result after grading" 200 "$(cat "$TMP/result2b.code")"
check_contains "manual result 100%" "$TMP/result2b.json" '"percent":100'
check_contains "manual result certificate issued" "$TMP/result2b.json" '"certificateCode":"WL-'
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/school/answers/$ANSWER_ID/grade" \
  -H 'Content-Type: application/json' -d '{"marksAwarded":5}' > "$TMP/grade2.code"
check "POST grade twice" 409 "$(cat "$TMP/grade2.code")"

# --- exam 3: late submit (deadline forced into the past via D1) ---
curl -sS -o "$TMP/exam3.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/school/courses/$COURSE_ID/exams" \
  -H 'Content-Type: application/json' -d "{\"bankId\":$BANK_ID,\"title\":\"Late Quiz\",\"durationMinutes\":30,\"maxAttempts\":3,\"passMarkPercent\":50,\"questionIds\":[$Q1]}" > "$TMP/exam3.code"
check "POST late-submit exam" 201 "$(cat "$TMP/exam3.code")"
EXAM3_ID="$(jget "$TMP/exam3.json" 'd.data.id')"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/school/exams/$EXAM3_ID" \
  -H 'Content-Type: application/json' -d '{"status":"published"}' > "$TMP/exam3-pub.code"
check "PUT late-submit exam publish" 200 "$(cat "$TMP/exam3-pub.code")"
curl -sS -o "$TMP/attempt3.json" -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/exams/$EXAM3_ID/start" \
  -H 'Content-Type: application/json' -d '{}' > "$TMP/attempt3.code"
check "POST start late-submit exam" 201 "$(cat "$TMP/attempt3.code")"
ATTEMPT3_ID="$(jget "$TMP/attempt3.json" 'd.data.id')"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/attempts/$ATTEMPT3_ID/answers" \
  -H 'Content-Type: application/json' -d "{\"questionId\":$Q1,\"answer\":1}" > "$TMP/ans5.code"
check "POST answer before deadline" 200 "$(cat "$TMP/ans5.code")"
PAST_MS="$(node -e 'console.log(Date.now()-60000)')"
"$WRANGLER" d1 execute DB --local --command "UPDATE wl_exam_attempts SET deadline_ms = $PAST_MS WHERE id = $ATTEMPT3_ID" >/dev/null
curl -sS -o "$TMP/late.json" -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/attempts/$ATTEMPT3_ID/submit" \
  -H 'Content-Type: application/json' -d '{}' > "$TMP/late.code"
check "POST submit after deadline" 409 "$(cat "$TMP/late.code")"
check_contains "late submit says expired" "$TMP/late.json" 'attempt_expired'
curl -sS -o "$TMP/attempt3b.json" -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" "$BASE/api/school/attempts/$ATTEMPT3_ID" > "$TMP/attempt3b.code"
check_contains "late attempt auto-submitted as expired" "$TMP/attempt3b.json" '"status":"expired"'
check_contains "late attempt kept the saved score" "$TMP/attempt3b.json" '"score":5'
curl -sS -o "$TMP/result3.json" -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" "$BASE/api/school/attempts/$ATTEMPT3_ID/result" > "$TMP/result3.code"
check "GET result of expired attempt" 200 "$(cat "$TMP/result3.code")"
check_contains "expired result 100%" "$TMP/result3.json" '"percent":100'
# A late answer save on a fresh attempt is 409 too.
curl -sS -o "$TMP/attempt3c.json" -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/exams/$EXAM3_ID/start" \
  -H 'Content-Type: application/json' -d '{}' > "$TMP/attempt3c.code"
check "POST start second late attempt" 201 "$(cat "$TMP/attempt3c.code")"
ATTEMPT3C_ID="$(jget "$TMP/attempt3c.json" 'd.data.id')"
"$WRANGLER" d1 execute DB --local --command "UPDATE wl_exam_attempts SET deadline_ms = $PAST_MS WHERE id = $ATTEMPT3C_ID" >/dev/null
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/attempts/$ATTEMPT3C_ID/answers" \
  -H 'Content-Type: application/json' -d "{\"questionId\":$Q1,\"answer\":1}" > "$TMP/late-ans.code"
check "POST answer after deadline" 409 "$(cat "$TMP/late-ans.code")"

# --- exam windows: not yet open, already closed, no questions ---
FUTURE_START="$(node -e 'console.log(new Date(Date.now()+86400000).toISOString())')"
FUTURE_END="$(node -e 'console.log(new Date(Date.now()+2*86400000).toISOString())')"
curl -sS -o "$TMP/exam-f.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/school/courses/$COURSE_ID/exams" \
  -H 'Content-Type: application/json' -d "{\"bankId\":$BANK_ID,\"title\":\"Future Quiz\",\"durationMinutes\":30,\"maxAttempts\":1,\"passMarkPercent\":50,\"windowStart\":\"$FUTURE_START\",\"windowEnd\":\"$FUTURE_END\",\"questionIds\":[$Q1]}" > "$TMP/exam-f.code"
check "POST future-window exam" 201 "$(cat "$TMP/exam-f.code")"
EXAMF_ID="$(jget "$TMP/exam-f.json" 'd.data.id')"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/school/exams/$EXAMF_ID" \
  -H 'Content-Type: application/json' -d '{"status":"published"}' > "$TMP/examf-pub.code"
check "PUT future-window exam publish" 200 "$(cat "$TMP/examf-pub.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/exams/$EXAMF_ID/start" \
  -H 'Content-Type: application/json' -d '{}' > "$TMP/future-start.code"
check "POST start before window opens" 409 "$(cat "$TMP/future-start.code")"
PAST_START="$(node -e 'console.log(new Date(Date.now()-2*86400000).toISOString())')"
PAST_END="$(node -e 'console.log(new Date(Date.now()-86400000).toISOString())')"
curl -sS -o "$TMP/exam-p.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/school/courses/$COURSE_ID/exams" \
  -H 'Content-Type: application/json' -d "{\"bankId\":$BANK_ID,\"title\":\"Past Quiz\",\"durationMinutes\":30,\"maxAttempts\":1,\"passMarkPercent\":50,\"windowStart\":\"$PAST_START\",\"windowEnd\":\"$PAST_END\",\"questionIds\":[$Q1]}" > "$TMP/exam-p.code"
check "POST past-window exam" 201 "$(cat "$TMP/exam-p.code")"
EXAMP_ID="$(jget "$TMP/exam-p.json" 'd.data.id')"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/school/exams/$EXAMP_ID" \
  -H 'Content-Type: application/json' -d '{"status":"published"}' > "$TMP/examp-pub.code"
check "PUT past-window exam publish" 200 "$(cat "$TMP/examp-pub.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/exams/$EXAMP_ID/start" \
  -H 'Content-Type: application/json' -d '{}' > "$TMP/past-start.code"
check "POST start after window closed" 409 "$(cat "$TMP/past-start.code")"
# Empty bank -> exam with no questions -> start 400.
curl -sS -o "$TMP/bank2.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/school/courses/$COURSE_ID/banks" \
  -H 'Content-Type: application/json' -d '{"title":"Empty"}' > "$TMP/bank2.code"
check "POST empty bank" 201 "$(cat "$TMP/bank2.code")"
BANK2_ID="$(jget "$TMP/bank2.json" 'd.data.id')"
curl -sS -o "$TMP/exam-e.json" -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X POST "$BASE/api/school/courses/$COURSE_ID/exams" \
  -H 'Content-Type: application/json' -d "{\"bankId\":$BANK2_ID,\"title\":\"Empty Quiz\",\"durationMinutes\":30,\"maxAttempts\":1,\"passMarkPercent\":50}" > "$TMP/exam-e.code"
check "POST empty exam" 201 "$(cat "$TMP/exam-e.code")"
EXAME_ID="$(jget "$TMP/exam-e.json" 'd.data.id')"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X PUT "$BASE/api/school/exams/$EXAME_ID" \
  -H 'Content-Type: application/json' -d '{"status":"published"}' > "$TMP/exame-pub.code"
check "PUT empty exam publish" 200 "$(cat "$TMP/exame-pub.code")"
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $STUDENT_COOKIE" -X POST "$BASE/api/school/exams/$EXAME_ID/start" \
  -H 'Content-Type: application/json' -d '{}' > "$TMP/empty-start.code"
check "POST start exam with no questions" 400 "$(cat "$TMP/empty-start.code")"

# --- delete guard: course with enrolments cannot be deleted ---
curl -sS -o /dev/null -w "%{http_code}" -H "Cookie: $ADMIN_COOKIE" -X DELETE "$BASE/api/school/courses/$COURSE_ID" > "$TMP/del.code"
check "DELETE course with enrolments" 409 "$(cat "$TMP/del.code")"

echo
echo "== P9 smoke: $PASS passed, $FAIL failed =="
rm -rf "$TMP"
[ "$FAIL" -eq 0 ]
