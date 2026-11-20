# WORDLESS v2.0 — Cutover runbook (Phase P5)

**Status: local evidence complete; staging and production steps are
operator steps.** Nothing in this runbook deploys anything on its own —
deployment needs the operator's Cloudflare login; no credentials are
stored in the repository. Every deploy step runs on the operator's own
Windows machine, in order, with the expected output written next to it.
If a step's output does not match, stop and do not continue to the next
step.

**Companion evidence (all local, measured 2026-11-19):**
- Parity run 5: 320 steps, 265 match, 17 classified known diffs,
  38 v2-only steps, **0 unexplained diffs** (harness exit 0).
  Report: `tests/parity/results/parity-report.json`.
- Latency (local workerd): v1 p50 57 ms / p95 207.5 ms;
  v2 p50 68.2 ms / p95 211.3 ms. Per-group numbers in
  `tests/parity/results/latency.json`. Local workerd timings only —
  they say the two workers are in the same band, nothing about
  Cloudflare CPU time (see step 6, R3).
- Rollback rehearsal: `node tests/parity/rollback.mjs` — **15/15 checks
  passed**. One shared D1 file, v1 → v2 → v1: the v1 session cookie
  kept working on v2 and again on v1 after rollback; posts created on
  either worker read on the other; migration 0011 applied cleanly to
  the v1 file. This is the proof that rollback needs no data work.
- WASM size (measured 2026-11-20, after the SEO split): all six
  Workers fit the free tier after `wasm-opt -Oz` — main **2,854,587
  bytes (90.7%, 145,413 under the 3,000,000 soft target)**, converter
  603,285, social 618,060, woo 883,345, school 649,081, seo 800,785.
  The deploy script size-gates all six and passes. (P5-era figure,
  now superseded: 3,328,519 bytes, 105.8% — resolved by wasm-opt -Oz
  plus the converter, social+Woo, school and SEO splits.)

---

## 1. Prerequisites — decisions first, no deploys until all are done

### 1a. The WASM size decision (Owner: project owner — RESOLVED 2026-11-20)

**Resolved, no longer a blocker.** There is no budget, so Workers
Paid ($5/mo) is permanently out; the project stays on the free tier.
The size was closed by doing BOTH of the free levers, in order:
`wasm-opt -Oz` first (binaryen is a project devDependency — the P5
build went 3,328,519 → 2,906,063 bytes), then the split pattern for
every subsystem that outgrew the limit: conversion out (2026-11-20),
social + WooCommerce out (same day), school out (same day, after P9),
SEO out (same day, after P10). Current state (measured 2026-11-20,
after `-Oz`): main **2,854,587** (90.7%, 145,413 under the 3,000,000
soft target), converter 603,285, social 618,060, woo 883,345, school
649,081, seo 800,785 — all six under the 3,145,728 hard limit, and
`scripts/deploy.mjs` size-gates all six before any deploy. If a future
phase pushes any Worker over the limit again, the deploy stops at the
gate with the exact byte count — do not work around the gate; the next
lever is the same split pattern on another crate. Full record: doc §3
"Converter split", "Social + Woo split", "School split", "SEO split".

### 1b. Project-owner sign-off for the cutover (Owner: project owner)

Cutover needs sign-off from the project owner (doc §7 Q9). P5
prepares the evidence; it does not cut over. Sign-off covers: the size
decision above, the staging soak (step 7) having passed, and the
production cutover window. Production cutover must not happen without it.

### 1c. Where v2 lives (Owner: project owner — doc §7 Q1, decided 2026-11-20)

The v2 code lives in the separate repository `ola1de/Wordless-CMS-v2`
(decided 2026-11-20); clone it on the operator's machine before step 2.
This runbook calls that directory **`C:\Users\USER\Wordless-CMS-v2`**
and the v1 directory `C:\Users\USER\Wordless-CMS`; adjust the path if
your checkout is elsewhere, and keep it consistent through the steps.

### 1d. Toolchain on the operator's machine (Owner: project owner)

- **Rust 1.100.0** (pinned): install rustup, then
  `rustup toolchain install 1.100.0`, `rustup default 1.100.0`,
  `rustup target add wasm32-unknown-unknown`.
- **wasm-pack 0.15.1**: `cargo install wasm-pack --version 0.15.1`.
- **wasm-bindgen CLI 0.2.130**: `cargo install wasm-bindgen-cli --version 0.2.130`.
- **worker-build 0.9.0**: `cargo install worker-build --version 0.9.0`. Every WASM build script calls this binary; without it `wrangler dev` and `npm run build:all` fail with `worker-build: command not found`.
- **Node 24** (already installed: 24.16.0) and **Git for Windows**
  (already installed — `bash` from Git Bash is what runs
  `scripts/build-worker.sh`; run the deploy commands from **Git Bash**,
  or add Git's `usr\bin` to PATH so `bash` resolves in PowerShell).
- **Wrangler 4.165.0** comes from the v2 repo itself: `npm install`
  in the v2 directory installs it as a project dependency (same
  version as the v1 repo). Never assume a global wrangler.

### 1e. What must already be true about production

- The live site is v1.1.0 (or later) on `wordless-prod` at
  https://wordless-prod.ola1de-olawande.workers.dev/ with production
  D1 database id `REPLACE-ME` (your production D1 id — find it with
  `npx wrangler d1 list`).
- The v1.1.0 build is tagged and deployable as the rollback build
  (step 8). If it is not tagged yet, tag it from the v1 directory
  before staging:
  `git tag v1.1.0` (only if that tag does not already exist — check
  with `git tag -l v1.1.0` first) and `git push origin v1.1.0`.

---

## 2. Get v2 onto the operator's machine and verify it locally

From Git Bash (or PowerShell with bash on PATH), in the v2 directory:

```powershell
cd C:\Users\USER\Wordless-CMS-v2
git pull
npm install
```

Expected: `npm install` completes; wrangler 4.165.0 lands in
`node_modules\.bin\wrangler`.

Then prove the machine can build and test v2 before any Cloudflare
contact:

```powershell
cargo test --workspace --lib
```

Expected: **480 passed, 0 failed** (20 crates). Then the parity
harness, exactly as it runs locally:

```powershell
bash tests/parity/run-parity.sh
```

Expected: `=== PARITY SUMMARY ===` with **unexplained 0** and
`== P5 parity: harness exit code 0 ==`. (Takes ~3 minutes plus the
first cargo build.) Then the rollback rehearsal:

```powershell
node tests/parity/rollback.mjs
```

Expected: `== P5 rollback: 15/15 checks passed ==`.

If any of the three fails on the operator's machine, stop — the difference
is environmental (toolchain version, line endings, ports in use) and
must be understood before staging. Do not skip ahead to a deploy.

---

## 3. Configure wrangler.toml — staging first, production second

### 3a. Staging resources (separate from production — the soak must never touch live data)

```powershell
cd C:\Users\USER\Wordless-CMS-v2
npx wrangler login
npx wrangler whoami
```

Expected: `wrangler whoami` prints an Account ID line (it exits 0
even when not logged in — read the output; "You are not authenticated"
means the login did not take). Then create the staging resources:

```powershell
npx wrangler d1 create wordless-staging
npx wrangler r2 bucket create wordless-media-staging
npx wrangler kv namespace create CONFIG
```

Expected: each command prints its id. Paste the three ids into
`wrangler.toml` `[env.staging]` (`database_id`, `bucket_name` is
already `wordless-media-staging`, KV `id`). The deploy script refuses
while any of them is still `REPLACE-ME` (verified).

Secrets — the staging worker needs its own copies of the same secrets
the v1 worker uses (same values; staging is a separate worker, so
secrets do not carry over):

```powershell
npx wrangler secret put PAYSTACK_SECRET_KEY --env staging
npx wrangler secret put FLUTTERWAVE_SECRET_KEY --env staging
npx wrangler secret put FLUTTERWAVE_SECRET_HASH --env staging
npx wrangler secret put STRIPE_SECRET_KEY --env staging
npx wrangler secret put STRIPE_WEBHOOK_SECRET --env staging
npx wrangler secret put MOCK_PAYMENT_WEBHOOK_SECRET --env staging
```

(Only the secrets the live site actually uses; skip any that were
never set. Never paste a secret value into chat or a file.)

### 3b. Production configuration (fill in now, deploy later)

In `wrangler.toml` `[env.production]` — the cutover shares the live
data, so these are the SAME resources as the v1 worker:

- `database_id`: `REPLACE-ME` (your production D1 id)
- `bucket_name`: `wordless-media-prod` (verify with
  `npx wrangler r2 bucket list` — it must already exist)
- KV `id`: find it with `npx wrangler kv namespace list` — the
  namespace the live v1 worker uses — and paste it in.

Do not cut over to production in this step. Filling the values in
now only lets the deploy script's placeholder check pass later.

---

## 4. Deploy to staging

```powershell
cd C:\Users\USER\Wordless-CMS-v2
npm run deploy:staging
```

(Workers Paid is ruled out — no budget — so there is no
`--paid` variant of this command.)

What the script does, in order, and what to expect:

1. `cargo test --workspace --lib` — must pass (619/0).
2. WASM builds (all six Workers) — must pass.
3. **Size gate** — refuses if ANY Worker's binary is over the 3 MiB
   free-tier limit, printing the exact byte count. All six currently
   fit (main 2,854,587; converter 603,285; social 618,060; woo
   883,345; school 649,081; seo 800,785 — measured 2026-11-20), so
   this step passes; it is the tripwire if a future phase outgrows
   the limit.
4. Placeholder check — refuses while any `[env.staging]` id is
   `REPLACE-ME` or the zero UUID.
5. `wrangler whoami` output check — refuses unless an Account ID is
   printed (the exit code alone is not trusted — wrangler exits 0
   even logged out).
6. Remote D1 migrations — on the fresh staging database this applies
   0001–0016. Expected output lists the migrations applied.
7. `wrangler deploy --env staging` in each sub-worker crate first
   (social → woo → school → seo → converter), then the main Worker —
   prints the staging URL
   (`https://wordless-staging.<subdomain>.workers.dev`).
8. Health checks — `GET <url>/api/health` must return 200, and each
   sub-worker health endpoint too: `/__social/health`,
   `/__woo/health`, `/__school/health`, `/__seo/health`,
   `/__convert/health`.

If any step fails, fix that step; the script never half-deploys.

---

## 5. Verify staging (immediately after deploy)

Do these in order; every one must pass before the soak:

1. `GET https://wordless-staging.<subdomain>.workers.dev/api/health` → 200.
2. Open `https://wordless-staging.<subdomain>.workers.dev/` → the public site renders (seeded content is NOT there — staging starts empty; create one post in the admin to confirm rendering).
3. Create the staging admin with the v2 seed script (same rows and
   PBKDF2 format as v1; refuses if an admin already exists):

   ```powershell
   cd C:\Users\USER\Wordless-CMS-v2
   npm run seed-admin -- --remote --env staging
   ```

   Answer the prompts (login, email, password twice). Expected:
   `Administrator created (id 1, login "...", ...)` and
   `Sign in at /admin with login "..."`. The script never prints the
   password. (Local seeding without `--remote` works the same way
   against the local D1.)
4. Sign in at `/admin` with that admin → the redesigned admin loads
   (8 themes, new editor, Plugins page).
5. Create a post with a category, a tag and a featured image; publish it; check it on the public site.
6. Upload one image in Media; check it renders.
7. Run one mock checkout end-to-end (storefront → cart → mock test
   checkout → order paid) and confirm the order shows as paid exactly once.
8. Open the AI command centre → provider reports Workers AI
   configured (the `[env.staging.ai]` binding is declared in
   `wrangler.toml`; v1's staging never had it).

---

## 6. R3 — the deployed PBKDF2 measurement (closes only here)

**Do not claim R3 closed from local timings.** Local wall-clock
login was ~86 ms on workerd; Cloudflare counts CPU time, and
pure-Rust PBKDF2 in WASM measured 78 ms while WebCrypto measured
31 ms (both release builds, Node 24). Login uses WebCrypto first.
The free-tier CPU limit is 10 ms per invocation — the deployed
measurement decides whether that holds.

How to measure (staging, after step 5):

1. Start a tail: `npx wrangler tail --env staging --format pretty`
   in one terminal (leave it running).
2. In a browser, sign out and sign back in at the staging `/admin`
   (one login = one `POST /api/auth/login` invocation).
3. Read the tail line for that invocation: it prints CPU time
   (`CPU Time: Xms` / `duration`). Record the number, and also the
   wall-clock time from the browser's network tab.
4. Repeat the login 5 times; record all five numbers.

**Bar:** login CPU time under 10 ms on the free tier. If it is over:

- Do NOT cut over on the free tier. The options are Workers Paid
  (30 s CPU) or rework of the login path — that is a new decision,
  not a runbook step. Record the numbers and stop.

Also confirm in the Cloudflare dashboard (Workers → wordless-staging
→ Metrics): no invocation errors, p50/p95 latency sane, no CPU-limit
exceptions during the soak.

---

## 7. Staging soak checklist (checks visible in the product — the P4 open items live here)

Soak for at least a day of real use on staging. Every item must be
checked by a human, on the operator's own devices:

- [ ] **§6.4 visual review** — Dashboard, editor, Plugins, community,
  commerce, school, SEO, users, public site at **1440 px, 768 px and
  360 px** wide. No horizontal overflow, no clipped cards.
- [ ] **Per-theme visual re-check** — switch through all 8 dashboard
  themes (Deep Teal, Midnight, Ocean Blue, Forest, Sunset Amber,
  Berry, Slate Mono, High Contrast) as Admin and as Editor; every
  screen stays readable in each theme.
- [ ] **Keyboard-only walkthrough** — Tab through the editor and the
  dashboard; every control reachable, focus visible, no keyboard trap.
- [ ] **Screenshot retake** — retake the admin screenshots on staging
  (the P4 redesign has no verified screenshots yet — none were produced
  during local testing, and none are claimed).
- [ ] **Galaxy A56** (Samsung SM-A566B) — sign in, create and publish
  a post, upload a photo, open the public site. Test on a real phone;
  the Galaxy A56 is the reference device.
- [ ] **Cron sweep** — schedule a post 10 minutes out; confirm it
  flips to `publish` on its own (the 5-minute `*/5 * * * *` trigger;
  the lazy-publish half was verified locally, the real sweep only
  fires on a deployed worker).
- [ ] **Scheduled-post + revision flow** — edit a published post,
  check revisions list and restore.
- [ ] **File manager + marketplace** — upload a theme ZIP, check the
  conversion report; install Hello Dolly from a ZIP, check its
  compatibility report.
- [ ] **AI command centre on real Workers AI** — one mock-free task
  (draft a post from a prompt) completes; approval flow creates
  exactly one draft.
- [ ] **Webhooks** — create a subscription, trigger an event, confirm
  exactly one delivery with a verifiable signature.
- [ ] **No console errors** on any admin page (browser devtools).

If anything on this list fails, fix it in code (P5 follow-up or P6)
and re-soak that item. Do not cut over with open soak items.

---

## 8. Cutover to production

Prerequisites: §1a (size) decided, §1b (project-owner sign-off) given,
§7 (soak) fully checked, §3b (production config) filled in.

### 8a. Back up production first

```powershell
cd C:\Users\USER\Wordless-CMS-v2
npx wrangler d1 export wordless-prod --remote --env production --output wordless-prod-backup-2026-11-19.sql
```

Expected: the command writes the SQL file (check its size is
non-zero). Keep the file — it is the pre-cutover restore point.
(If `d1 export` errors, run `npx wrangler d1 export --help` and use
the syntax it prints; the database name is `wordless-prod`.)

### 8b. Deploy v2 to production

```powershell
cd C:\Users\USER\Wordless-CMS-v2
npm run deploy:production
```

(Workers Paid: `node scripts/deploy.mjs --env production --confirm --paid`.)

What happens: cargo tests → WASM build → size gate → placeholder
check → whoami output check → **remote D1 migrations** (on the live
database only `0011_p3.sql` is new — the webhook subscriptions and
deliveries tables; everything else is already applied) → deploy to
`wordless-prod` → health check on
https://wordless-prod.ola1de-olawande.workers.dev/api/health.

### 8c. Verify production (in order, immediately)

1. `GET https://wordless-prod.ola1de-olawande.workers.dev/api/health` → 200.
2. **Existing session**: in the browser where the operator is already
   signed in to the live `/admin`, refresh — the session must still
   work (no fresh login). This is the live version of rehearsal
   check A7. If it asks for a password, stop and roll back (§9) —
   something changed about sessions.
3. Public site: the home page and two published posts render; one
   category archive and one tag archive render.
4. Admin: create a draft, add a category + tag + featured image,
   publish it, confirm it on the public site.
5. One mock test checkout end-to-end (only if the store has a test
   product; never run a real payment as a test).
6. Dashboard metrics + `wrangler tail --env production`: no errors,
   no CPU-limit exceptions in the first 30 minutes.

---

## 9. Rollback (if anything in §8c fails)

Rollback needs **no data work** — schema and sessions are identical
(§1.3), and the rehearsal proved it locally (15/15). Two equivalent
ways; use the first, keep the second as backup:

**Option A — `wrangler rollback` (fastest):**

```powershell
cd C:\Users\USER\Wordless-CMS-v2
npx wrangler rollback --env production
```

This restores the previous deployment — v1.1.0 — on `wordless-prod`.
Verify §8c items 1–4 against the rolled-back worker (health, the
same browser session still works, public pages, one admin edit).

**Option B — redeploy the v1 commit:**

```powershell
cd C:\Users\USER\Wordless-CMS
git pull
npm install
npm run deploy:production -- --confirm
```

This redeploys v1.1.0 from its own repo. Same verification as A.

After either rollback: confirm the v2-created content (if any) still
reads on v1 — the rehearsal showed v2-created posts read fine on v1
(check B2). If a v2-only feature wrote data v1 cannot read (webhook
subscriptions/deliveries rows in the two new tables), v1 ignores
those tables — they are additive and harmless.

---

## 10. After cutover

- Re-run the local parity suite (`bash tests/parity/run-parity.sh`)
  and the rollback rehearsal (`node tests/parity/rollback.mjs`) from
  the v2 directory — both must stay green; they are the permanent
  regression proof for every later phase.
- Keep the v1.1.0 tag as the rollback build until v2 has soaked on
  production for at least a week with no rollback.
- The P5 exit gate (doc §3 P5) is met only when v2.0.0 is live on
  production, the parity suite is green, and v1.1.0 is retained as
  the tagged rollback build. Until then v2 is "ready, not done".
- P6+ work (extension runtime, social, WooCommerce, school, SEO,
  Visual Builder, multisite) starts only after the cutover is live
  and stable.

---

## Appendix A — Known diffs visible on staging (17, classified)

These are differences the parity harness found and classified, not
bugs. They are expected on staging too:

- **Version strings**: v2 reports `2.0.0-dev`, default theme `0.7.0`;
  v1 reports `1.1.0` / `0.6.0`.
- **Admin SPA redesign**: the whole admin is the P4 redesign (8
  themes, new editor, Plugins page, tweakable login) — it looks
  different from v1 on purpose.
- **CPT archives now serve**: `/books` and `/books/<slug>` return the
  archive/single on v2; v1 404s (P2 gap, closed in P3).
- **Tag archive fixed**: `/tag/<slug>` works on v2; v1 404s on every
  tag archive (v1 bug, fixed in v2 only — v1 is frozen).
- **Single-post `?p=`**: `GET /?p=<id>` renders the post on v2
  (WordPress-correct); v1 ignores `?p=` and renders home.
- **AI binding configured**: v2 declares the `[ai]` binding, so the
  AI command centre reports Workers AI configured; v1's binding is
  commented out, so it reports not-configured.
- **Backup size**: v2 backups are larger (2 extra tables from
  migration 0011 — webhook subscriptions + deliveries). Each worker
  faithfully backs up its own DB.
- **Conversion fixture**: with upstream unreachable, v1 creates the
  job (201, status failed); v2 returns 502 with no half-created job
  (deliberate, documented in `routes_p3.rs`).
- **Extra `mustChangePassword` field** on v2 login/me responses
  (P4 forced-reset flow) — additive, harmless.

## Appendix B — Local evidence summary (2026-11-19)

| Item | Result |
|---|---|
| Parity run 5 (320 steps) | 265 match, 17 known-diff, 38 v2-only, **0 unexplained**, exit 0 |
| Route coverage | All 173 v1 routes present in v2 (guard comparison 0 mismatches); 27 extra v2 routes (P3/P4 additions); 198 distinct corpus paths |
| Latency, local workerd | v1 p50 57 ms / p95 207.5 ms; v2 p50 68.2 ms / p95 211.3 ms |
| Rollback rehearsal | 15/15 checks passed (`tests/parity/rollback.mjs`) |
| Cargo tests | 480 passed, 0 failed (20 crates) |
| WASM size | 3,328,519 bytes — 105.8% of the 3 MiB free-tier limit |
| P4 smoke (regression) | smoke-p3 273/273, smoke-p4-api 150/150 (worker-reported, P4) |

## Appendix C — What is NOT covered by this runbook

- The staging deploy itself has never run yet; steps 4–9 are written
  from the v1 deploy history, the
  wrangler 4.165.0 help text, and the local rehearsals — not from an
  observed v2 deploy. The first staging run may surface environment
  differences (Windows `bash`, Rust toolchain, wrangler version).
- R3 (PBKDF2 CPU) is open until the step-6 measurement.
- The P4 visual items (§6.4) are open until the step-7 soak.
- Live Workers AI calls, live payment-provider checkouts and live
  HTTP email have never been observed (no keys connected) — step 5
  item 8 covers Workers AI on staging; payment providers stay mock
  until the operator configures real keys.
- Converted plugins/themes still do not run (P6). The honest wording
  stays "converts and reports".
