# WORDLESS v2.0 — Deployment guide

How to run WORDLESS v2 locally, deploy it to staging, and cut over to
production. The deploy script (`scripts/deploy.mjs`) is the authority:
it refuses to deploy unless every guard passes. This guide explains
what it does and what you must set up first.

> **Status: v2.0 is pre-release.** The current stable release is v1.1.0
> (TypeScript); v2.0 (Rust) has completed all 12 build phases and is
> pending staging validation before production cutover. Production
> cutover requires staging validation plus sign-off from the project
> owner — see `docs/cutover-runbook.md` for the full checklist.
> Deployment runs from the operator's machine, using their own
> Cloudflare login; no credentials are stored in the repository.

## Architecture in one paragraph

Six Cloudflare Workers sharing one D1 database: the **main** Worker
(CMS, auth, admin, public site) plus **converter** (PHP→TypeScript
engine + Elementor renderer), **social** (profiles, friends, groups,
messaging), **woo** (WooCommerce-style store), **school** (courses +
CBT exams), and **seo** (JSON-LD, sitemaps, robots, llms.txt). The main
Worker keeps every route registration and proxies the moved ones to the
sub-Workers over service bindings. One D1 (`wordless-staging` /
`wordless-prod`), one R2 bucket for media. Worker names: `wordless-v2`
(local dev), `wordless-staging`, `wordless-prod` (production shares
the name — and the data — with the live v1 worker; cutover swaps the
Worker binary, not the data).

## Prerequisites

- **Node.js 20+** and npm. Wrangler is a project dependency
  (`npm install`); never assume a global install.
- **Rust toolchain** (stable) + `wasm-pack` + `worker-build 0.9.0`
  (`cargo install worker-build --version 0.9.0`), for the WASM builds.
  The project pins its versions in the docs; `npm run build` fails
  loudly if the toolchain is missing.
- **A Cloudflare account**, logged in via `npx wrangler login`
  (official browser OAuth). Confirm with `npx wrangler whoami` — it
  must print an Account ID line. (`wrangler` exits 0 even when logged
  out, so check the output, not the exit code.)

## First-time setup

```sh
git clone https://github.com/ola1de/Wordless-CMS-v2.git
cd Wordless-CMS-v2
npm install
```

Then fill in the placeholders in `wrangler.toml` (and the matching
blocks in each sub-worker crate's `wrangler.toml`):

| Placeholder | What to put there | How to find it |
|---|---|---|
| `database_id` (`REPLACE-ME`) in `[env.staging]` / `[env.production]` | Your D1 database id | `npx wrangler d1 list` |
| R2 `bucket_name` | Must already exist | `npx wrangler r2 bucket list`; create staging with `npx wrangler r2 bucket create wordless-media-staging` |

Payment provider secrets are **Worker secrets, never files**:

```sh
npx wrangler secret put PAYSTACK_SECRET_KEY --env staging
npx wrangler secret put FLUTTERWAVE_SECRET_KEY --env staging
npx wrangler secret put FLUTTERWAVE_SECRET_HASH --env staging
npx wrangler secret put STRIPE_SECRET_KEY --env staging
npx wrangler secret put STRIPE_WEBHOOK_SECRET --env staging
npx wrangler secret put MOCK_PAYMENT_WEBHOOK_SECRET --env staging
```

(Repeat with `--env production` for production. Without a provider
configured, checkout honestly returns 503 — it never fakes success.)

## Local development

```sh
npm run dev:split   # all six Workers, with service bindings wired up
npm run build:all   # all six WASM builds (wasm-opt -Oz)
npm run size        # WASM size report for all six Workers
npm test            # cargo test --workspace --lib
```

Dev ports: main **:8787**, converter **:8805**, social **:8807**,
Woo **:8808**, school **:8809**, SEO **:8812**. Open
http://127.0.0.1:8787/admin. Local D1 is a scratch database;
migrations in `migrations/` apply automatically.

## Deploy to staging

```sh
npm run deploy:staging
# equivalent: node scripts/deploy.mjs --env staging --confirm
```

Before touching Cloudflare, the script runs: the full cargo test
suite, all six WASM builds, and the size gate for every Worker. Then
it applies the remote D1 migrations for staging, deploys in this
order — **social → Woo → school → SEO → converter → main** — and
verifies every health endpoint, printing the URLs:

- main `/api/health`
- `/__social/health`, `/__woo/health`, `/__school/health`,
  `/__seo/health`, `/__convert/health`

Then create the first staging admin (staging has no admin yet):

```sh
npm run seed-admin -- --remote --env staging
```

Follow the prompts. (Production already has its admin from v1 —
do not seed production.)

Walk through staging as a user: two sites under Multisite, the
builder, a timed exam, a checkout, the SEO audit page. The full
checklist is in `docs/cutover-runbook.md`.

## Cut over to production

Only after staging is walked through and signed off by the project owner:

```sh
npm run deploy:production
# equivalent: node scripts/deploy.mjs --env production --confirm
```

The same guards, checks, migration step, deploy order and health
verification run against production. The production deploy swaps the
`wordless-prod` Worker from the v1 TypeScript build to the v2 Rust
build; the D1 data, sessions and media stay exactly where they are.
Full steps, including the pre-cutover backup
(`npx wrangler d1 export wordless-prod --remote --env production`)
and the rollback rehearsal, are in `docs/cutover-runbook.md`.
Rollback, if ever needed: `npx wrangler rollback --env production`.

## Size limits — what the gate enforces

`scripts/deploy.mjs` enforces **3 MiB per Worker** on the free tier
(10 MiB with `--paid`, Workers Paid $5/mo). That gate is stricter
than Cloudflare now requires: since 4 September 2026 the Worker size
limit is **64 MiB uncompressed on both free and paid plans**, with no
compressed limit. The six-Worker split stays as the architecture —
it keeps every Worker small and fast — but size alone no longer
forces a split. What still stands on the free tier: **100,000
requests/day and 10 ms CPU per request**. The one open performance
risk is the login password check (measured ~31 ms locally against
the 10 ms bar); it stays open until a deployed staging measurement.
See `docs/cutover-runbook.md` step 8.
