# WORDLESS v2.0 — Rust backend (Phases P1–P12 + size remedy: skeleton + Core CMS + Conversion/AI/Commerce + admin redesign + parity + extension runtime + social networking + converter split + WooCommerce + social/Woo split + school portal/CBT + school split + SEO depth + Visual Builder + Multisite)

Repo: [ola1de/Wordless-CMS-v2](https://github.com/ola1de/Wordless-CMS-v2).
**Do not deploy to production yet.** The v1.1.0 TypeScript worker stays
live until v2 passes the cutover gate (staging deploy + user walkthrough +
written approval). Nothing here touches the TS repo
(`~/workspace/wordless/wordless-cms/`, read-only reference) or the live site.

Plan: `~/workspace/wordless/docs/04-rust-v2-architecture.md` (§1, §2, §3 P1–P2).
Converter prototype (P0): `~/workspace/wordless/rust-prototype/`.

## Current state (2026-11-20 — after P12: Multisite, the final build phase)

**Six Workers, one shared D1.** P8 pushed the main Worker to 3,241,322 bytes
(103.0% of the free-tier limit), so social networking (P7) and WooCommerce
(P8) were split into their own Workers behind service bindings — the same
pattern as the converter split. Nothing was deleted to fit. The main Worker
keeps all moved route registrations (same paths, methods, auth guards)
and proxies them; all sub-Workers bind the SAME D1 as the main Worker (the
main Worker still runs all migrations). P9 (school portal + CBT) then grew
the main Worker again to 3,111,496 bytes (98.9% — 111,496 over the
3,000,000 soft target), so the school portal + CBT was split the same way
on 2026-11-20. P10 (SEO depth) then grew the main Worker again, by 71,549
bytes post-Oz, to 2,994,900 bytes (95.2% — only 5,100 under the soft
target), so `wordless-seo` itself was split the same way on 2026-11-20:
the main Worker is now **2,935,745 bytes (93.3%)** after P12 (Multisite)
— **64,255 bytes under the soft target** with 209,983 bytes of headroom.
P12 added 48,846 bytes post-Oz (2,886,899 → 2,935,745); the converter
is unchanged at 619,641. Nothing was deleted to fit. P12 is the final
build phase — after it, only the cutover gate remains. The render primitives the
main Worker still needs (JSON-LD/head/meta, 404-log write) live in the
shared `wordless-seo-core` crate — see §3 "SEO split" in the architecture
doc. Measured 2026-11-20, after `wasm-opt -Oz`:

| Worker | Crate | -Oz bytes | % of 3,145,728 |
| --- | --- | --- | --- |
| main | `wordless-worker` | **2,935,745** | 93.3% (209,983 headroom — **64,255 under the 3,000,000 soft target**) |
| converter | `wordless-converter` | 619,641 | 19.7% |
| social | `wordless-social-worker` | 618,060 | 19.6% |
| woo | `wordless-woocommerce-worker` | 881,877 | 28.0% |
| school | `wordless-school-worker` | 650,443 | 20.7% |
| seo | `wordless-seo-worker` | 807,836 | 25.7% |

**Commands** (from this directory; `npm install` first for wrangler + binaryen):

| Command | What it does |
| --- | --- |
| `npm run build:all` | Builds all six Workers with wasm-opt -Oz + the size gate (each must fit 3 MiB) |
| `npm run build` / `build:converter` / `build:social` / `build:woo` / `build:school` / `build:seo` | Build one Worker |
| `npm run size` | Reports all six WASM sizes vs the limit (exit 0 = all under) |
| `npm run dev:split` | Converter :8805, social :8807, woo :8808, school :8809, seo :8812, then main :8787 with all five `WORDLESS_*_URL` vars set |
| `npm run test` | `cargo test --workspace --lib` |
| `npm run deploy:staging` / `deploy:production` | Guarded deploy: cargo tests + all six builds + size gates, then deploys social → woo → school → seo → converter → main, verifying `/api/health` and `/__social/health`, `/__woo/health`, `/__school/health`, `/__seo/health`, `/__convert/health` (user's machine only — no Cloudflare credentials in this VM) |

**Verification on the P12 build (2026-11-20, all measured):** cargo test
**636/0** (baseline 630 + 6 multisite); smoke-p6 62/62, smoke-p7
99/99, smoke-p8 121/121, smoke-p9 107/107, **smoke-p10 89/89**, **smoke-p11
54/54** (port 8814), **smoke-p12 94/94** (port 8815), P4 API
150/150, smoke-p3 275/275, smoke-p2 129/129 — all sequential clean-room
(`rm -rf .wrangler` before each; the smokes share one local D1 and do
not clean it themselves); parity **482 steps, 248 match, 34 known-diff,
0 unexplained, 200 v2-only, exit 0** (v1 p50 45.5/p95 170.2, v2 p50
86.2/p95 207.8 — same band as P5–P11); dedicated SEO
end-to-end through the proxy (`scripts/seo-e2e.sh`) **19/19** (public post
page with JSON-LD + breadcrumbs + canonical, sitemap index + image sitemap
+ `llms.txt` + `robots.txt` through main, redirect create with no leaked
purge header); dedicated school end-to-end (enrol → start → submit →
auto-grade 10/10 → result 100% → certificate verified via API + public
page) 18/18. The smoke and parity scripts start the sub-Workers
themselves (`scripts/split-workers-dev.sh`, `--persist-to` the shared local
D1) — always run them from a clean room (`rm -rf .wrangler` first). One
split lesson is recorded in the code: the proxy must forward the
sub-Worker's `Set-Cookie` (the Woo cart token) or every later cart
request starts a new empty cart. One script flake was found and fixed
during this verification: smoke-p6 died once (exit 9) because the
randomly-derived runaway host token can start with `-`, which `node -e`
parses as an option — the token is now passed after `--`. One options-layer
finding from P10: Wordless options are JSON-encoded on write, and
`get_option` returns the default when the stored text is not valid JSON —
so any option hand-seeded by raw SQL must be JSON-encoded (a bare URL is
silently ignored). One harness lesson from P11: killing the wrangler
parent does NOT kill its `workerd` children — an orphaned workerd held
port 8812 through two p2 runs and every proxied SEO call 401'd against
its stale D1; kill the orphaned `workerd` PIDs (`ss -ltnp` shows the
listener) before re-running, and never `pkill -f 'wrangler dev'` from a
shell whose own command line contains that string (it SIGTERMs itself —
use the `wrangle[r]` bracket trick).

The P3–P12 sections below are the chronological phase log; their size
statements are historical (P3 over, P4 over, P7 over-then-resolved, P8
over-then-resolved, P9 over-then-resolved by the school split, P10 thin —
5,100 under the soft target, P11 fits with 113,101 to spare, P12 fits
with 64,255 to spare). The table above is the current state. Full remedy
record: doc §3 "Social + Woo split" and "School split".

## What P1 built

Cargo workspace, 4 crates, ~3,050 lines of Rust:

| Crate | Contents |
| --- | --- |
| `wordless-kernel` | JSON envelope `{ok, data, error}`, generic router (`:param`, 401/403/404/405, `read_json_body`), capabilities (5 roles), validation (v1 messages), security headers (exact v1 values), base64url, hand-rolled ISO-8601 time, crypto (WebCrypto PBKDF2 + SHA-256 on wasm32, pure-Rust fallback elsewhere), `ObjectStore` trait |
| `wordless-db` | `Db` trait (`prepare`/`bind`/`all`/`first`/`run`/`batch`, mirroring v1 §2.3), `Row` helpers, `get_option`/`set_option`, D1 implementation over `worker::d1` |
| `wordless-auth` | PBKDF2 passwords (`pbkdf2$100000$<salt>$<hash>`, byte-identical to v1), sessions (opaque token, SHA-256 hash in `wl_sessions`, 24 h idle timeout, `wordless_session` cookie), users (login matches `user_login` only), login flow |
| `wordless-worker` | `#[event(fetch)]` entry, route table, site-health (6 checks, 27 tables), `/admin` from ASSETS, security headers on every response, R2-backed `ObjectStore` |

Routes (all returning the v1 envelope): `GET /api/health`,
`GET /api/site-health`, `POST /api/auth/login`, `POST /api/auth/logout`,
`GET /api/auth/me`. Lockout/TOTP/session-hardening were P2 scope and are
implemented there (see below) — the P1 login was the plain password flow.
Non-API, non-admin paths return the 404 envelope in P1 (public rendering
is P2).

Migrations 0001, 0002, 0005–0010 are copied read-only from the TS repo
into `migrations/`; the originals are never edited.

## What P2 built (2026-11-19)

Cargo workspace now 9 crates, ~32,500 lines of Rust (incl. tests). All
**104 P2 routes** from `P2-NOTES.md` are wired in
`crates/wordless-worker/src/routes.rs` (method/path/guard verified
programmatically: 104 registered vs 104 listed, 0 missing, 0 extra,
0 guard mismatches, no P3 routes). Route list: `P2-NOTES.md`.

| Crate | Contents (new in P2) |
| --- | --- |
| `wordless-auth` | TOTP 2FA (RFC 6238, ±1 step, 8 hashed recovery codes; all six RFC vectors verified), login lockout (5 failed in 15 min → 429, per-name AND per-IP, doubling 15→30→60 min cap), guarded login (`login_guarded`: lockout → password → forced-reset → TOTP), sessions list/revoke-all, users CRUD + admin reset. **v1 has no user-facing forgot-password/token reset** — none was invented. |
| `wordless-cms` | Posts (CRUD, revisions/autosaves, slug-change 301s, trash/restore/delete — restore sets `draft`, as in v1), terms (hierarchy, counts), comments (moderated, honeypot, link limit), menus + locations, LIKE search, permalinks (`plain`/`postname`/`day-name`/`month-name`), options/settings, activity log (`wl_audit_log`, as in v1), postmeta. **NEW vs v1:** persistent custom post types + custom taxonomies (JSON in `wp_options` under `wl_post_types` / `wl_taxonomies`, no new migration; rewrite rules `/{rewriteSlug}/{slug}` + archive; per-type slug uniqueness). No dedicated CPT/taxonomy REST endpoints exist in v1, so none were added — exposed via the posts `type` filter / terms `taxonomy` param (P3 decision). |
| `wordless-media` | R2 uploads (50 MB cap, MIME allowlist, magic-byte sniffing, hand-rolled binary-safe multipart), listing, file manager (`MANAGED_PREFIXES`), image derivatives when the Images binding is present — **never faked when unbound** (serve original + `X-Wordless-Derivatives: unavailable`; Images binding not declared in `wrangler.toml`). Video/document optimisation is a decision, not transcoding: no in-Worker video transcoding (v1 admits no video MIME type; delivery stored-as-is with reason documented). R2 keys are v1's flat `<8-hex>-<filename>` shape. No Range support (v1 parity — whole-object 200). |
| `wordless-themes` | theme.json v1–v3 → CSS (byte-identical to v1's generator, cyrb53-verified), registry + built-in refresh rule, ZIP import (10,000 files / 50 MB), customizer (variations + preview tokens), patterns (allowlisted literal PHP evaluator, no PHP execution), classic-theme scan. Import reports carry `"conversion": null` — M11 auto-convert is P3. |
| `wordless-render` | Block-comment HTML rendering, template hierarchy, public pages, RSS feed, Cache API helpers (`public, max-age=60, stale-while-revalidate=60`; purge on API post create/update only, not imports — v1 limit). Elementor: `render_post_content` returns block content only when `_elementor_data` exists (P3 renders Elementor); shop pages are not P2. |
| `wordless-seo` | Per-post SEO fields (limits/messages exactly as v1), head rendering (title/meta/canonical/robots/OG/Twitter/JSON-LD with v1 key order), deterministic analysis (empty post scores exactly 50; Flesch clamped 0–100 as in v1), sitemap (1,000/page, 50,000 max) + robots.txt, redirect manager + 404 log (500 cap), content templates + calendar, Yoast/Rank Math/Wordless metadata import. |
| `wordless-worker` | Full 104-route table; guarded login; public fallback (`/?p=<id>`, `resolve_path` → 301/home/single/page/term/date-archive/404 + 404 logging); Cache API in the entry (anonymous public GETs only, `Cache::default()`); purge on post create/update + redirect create; `/admin` now serves the **real v1 SPA build** (copied from `wordless-cms/admin/dist/`, never modified — P4 redesigns it). |

**P2 bugs found & fixed by the smoke run (all verified):**
1. `SeoImportRow` deserialized `post_id` from JSON while v1's wire
   format is `postId` — rows were silently skipped ("Row has neither
   postId nor slug."). Fixed with `#[serde(rename_all = "camelCase")]`
   on the struct (`crates/wordless-seo/src/import.rs`).
2. Smoke-script expectation fixes (not product bugs): permalink
   structures are the v1 names (`postname`, not `/%postname%/`);
   `Term`/`MediaRecord` serialize camelCase (`termTaxonomyId`,
   `objectKey`); empty `robots` is rejected 400 in v1 too
   (`requireString` non-empty); SEO import takes raw meta keys
   (`_wl_seo_focus_keyword`, not `focusKeyword`); restore sets
   `draft`, as in v1.
3. The `syn/full` build-dep workaround from P1 was **removed** from
   `wordless-db` in P2 — worker-macros 0.9.0 does not need it
   (workspace check + tests green without it).

**WASM size warning (resolved in P3 — see the P3 section):**
`index_bg.wasm` was 2,662,192 bytes (~2.54 MiB) at P2 and the P3
wiring first pushed it to **5,017,385 bytes** — 59% OVER the 3 MiB
limit. Size-tuning the release profile (`strip = true`,
`opt-level = "z"`) brought it to **3,179,092 bytes — still 33,364
bytes (1.1%) over the free-tier limit**. wasm-opt `-Oz` (binaryen
cannot be downloaded in this VM) is the remaining lever and is
expected to close the gap; until it is measured on a machine that
can download it, the binary is OVER the limit — do not deploy it
to the free tier as-is. See Measurements below.

**Open blockers / notes for P3 (both P2 gaps CLOSED in P3 wiring):**
- ~~CPT archive rendering~~ — DONE: `render_post_type_archive` exists
  in `wordless-render` and `handle_public_fallback` wires the
  `PostTypeArchive` branch (smoke: `GET /books` renders, `GET
  /books/dune` renders). Caveat: P2 shipped **no CPT/taxonomy REST
  routes** (service only) — the smoke seeds the type by SQL; P4 must
  add the routes for the admin UI.
- ~~Redirect-update purge~~ — DONE: `PUT /api/seo/redirects/:id` now
  purges the old path too (smoke verifies the retarget takes effect
  immediately).
- Mail, partially closed: order-confirmation mail is now sent
  best-effort when a webhook marks an order paid (mock provider logs
  to `wl_mail_log`, never presented as delivered). Comment admin
  notification and reset-password email are STILL not wired —
  `POST /api/users/:id/reset-password` returns `emailed: false` and
  the temporary password in the response is the admin's only copy.
- Theme CSS preview: `?v=<hash>` is accepted and ignored (hash is
  recomputed from served CSS); `?wl_preview=` works via
  `resolve_variation`.
- PBKDF2 R3 still open until a deployed measurement at P5 (see below).
- `P2-NOTES.md` is coordinator working notes — remove before any push.

## What P3 built (2026-11-19)

Cargo workspace now **20 crates, ~66,900 lines of Rust** (incl.
tests). All **69 P3 routes** from `P2-NOTES.md` are wired in
`crates/wordless-worker/src/routes_p3.rs`, plus the two new v1.3
groups: `POST /api/graphql` and the outbound-webhook routes
(`GET/POST/DELETE /api/webhooks/subscriptions`,
`GET /api/webhooks/deliveries`,
`POST /api/webhooks/deliveries/:id/retry`,
`POST /api/webhooks/deliveries/retry-due`) — **173 routes total**
(104 P2 + 69 P3), plus the 11 public shop routes and the GraphQL
endpoint. Route list: `P2-NOTES.md` (P3 list) + `routes_p3.rs`.

| Crate | Contents (new in P3) |
| --- | --- |
| `wordless-conversion` | P0 prototype promoted: lexer/parser (verbatim from the prototype) + emitter (2,218-line port of v1 `emitter.ts`) + safety + analyser + auto-convert + jobs over `wl_jobs`. **Measured:** Hello Dolly (real `hello.php`) **3/3 functions, 2/2 hooks, 0 unconverted**, generated TS **byte-identical to the checked-in v1 fixture**; Twenty Twenty-Five **23 converted / 76 partial / 0 blocked of 99 PHP files — exactly the v1 M11 record**. The "generated-TS-still-typechecks" check is exact-string assertions (tsc cannot run inside `cargo test`). `analyze_php` verdicts follow the M11 auto-convert record; the faithful `analyze.ts` port is also present as `analysis::analyze_php`. |
| `wordless-elementor` | `_elementor_data` renderer: sections → columns → 10 widgets (heading, text-editor, image, button, spacer, divider, video, icon-list, image-box, google-maps), depth limit 10, content only, no editor. Wired into `render_post_content` (falls back to block rendering when it renders nothing — the v1 rule). The escapers moved to `wordless-kernel::html` to break the render↔elementor cycle. |
| `wordless-ai` | Providers (Workers AI default via native `worker::Ai`, OpenAI-compatible fallback, deterministic mock), orchestrator, all **15 agents**, approvals, memory. **Measured:** mock-provider task completes and approval creates **exactly one draft post** (test `supervised_task_creates_approval_and_approval_creates_exactly_one_draft`). Live Workers AI calls never observed in this VM (no account, no `[ai]` binding). `AgentContext.object_count` is `None` unless the worker supplies it. |
| `wordless-commerce` | Products, server-side cart (`wordless_cart` cookie), orders, HMAC webhooks (Paystack SHA512, Flutterwave `verif-hash`, Stripe SHA256 + 300 s tolerance, mock SHA256 — v1 test vectors ported, verified against Python-computed HMACs). **Measured:** mock checkout + signed webhook → order `paid`, stock 5→3; replay → `duplicate`, stock stays 3; bad signature → 401, order stays `pending`. `start_checkout` appends `?order=<id>` to the provider return URLs (v1 behaviour). Real-provider checkout HTTP lives in the worker (`gateways.rs`, never verified live — no keys). |
| `wordless-integrations` | Registry (8 integrations, live configured detection), config with secret rejection, email (mock logs to `wl_mail_log`, `delivered: false`; HTTP provider via `EMAIL_API_KEY`), analytics snippets, social share links, cloudflare binding status, `wl_extensions` + marketplace ZIP install (10,000/50 MB, manifest, traversal rejection) + compat reports. |
| `wordless-security` | `security_overview`: audit log (20), live lockout state, TOTP status, active session count, 6 security headers, failed-login tail. The Security Centre route is `GET /api/security/overview`. |
| `wordless-importer` | Hand-rolled WXR reader + SQL-dump parser + safe PHP unserialize (objects/references refused, depth cap 200 — v1 has none, hardening). **Measured:** WXR fixture matches v1 counts (users 2, terms 4, posts 1, pages 1, `by_type["post"].imported` 3, skipped 1 revision, comments 2, attachment_media 1); SQL fixture (users 2, posts 2, skipped 1, comments 2). Attachments fetch through `WorkerAttachmentFetcher` into R2; an unreachable URL is recorded `skipped`, not failed. |
| `wordless-backup` | Checksummed JSON bundle + media manifest as one ZIP in R2, retention 10, verify-before-restore, typed `RESTORE` confirmation, version gate, tamper detection. Backup IDs are 16 random bytes as hex, not UUIDs. |
| `wordless-compat` | All **45** v1 compatibility entries (6 `full` hooks, 1 `stub`, 38 `partial`) + the `wl_*` surfaces as separate lists. The callable `createWordPressApi` is NOT in this crate — assembled from owning service crates at wiring time. |
| `wordless-graphql` | Hand-rolled query parser + executor (queries only; no mutations/subscriptions/directives/introspection). Root fields: `posts`, `post`, `terms`, `term`, `media`, `user`, `users`, `option`, `siteInfo` — same capability guards as REST (draft hidden from anonymous; `users`/`media` need their capabilities). `operationName` supported (added in P3 wiring). A GraphQL query returns the same post a REST call returns (smoke-verified). |
| `wordless-webhooks` | Event subscriptions + HMAC-SHA256-signed deliveries (`X-Wordless-Signature: sha256=<hex>`), retry with backoff (60 s × 2^attempts, cap 1 h, max 5), delivery log. Migration `0011_p3.sql`. **Measured:** dispatch fires exactly once per active subscription per event; signature verified against a Python-computed vector. Automatic retry sweeping needs P5 cron wiring. |
| `wordless-worker` (wiring) | `routes_p3.rs` (~2,400 lines): all P3 routes; marketplace report builder (v1-shaped compat report: per-file verdicts from the faithful analyser, top reasons, option defaults, auto-convert record with generated TS stored in R2 under `extensions/<slug>/converted/`, settings migrated never-overwrite); conversion-job fixture fetch from the pinned upstream URL (502 on failure); import job runner (reads the R2 object, runs the import, updates the `wl_jobs` row); backup-create job flow; shop routes (cart cookie, mock test-checkout flow that signs a mock webhook with `MOCK_PAYMENT_WEBHOOK_SECRET`); outbound dispatch wired into post create/update/publish, comment create, and order-paid; order-confirmation mail best-effort after webhook-paid. `gateways.rs`: real Paystack/Flutterwave/Stripe gateways + `WorkerHttpPost` + `fetch_text` + `WorkerAttachmentFetcher`. `lib.rs`: `opt_secret` falls back to `env.var()` (local `wrangler dev --var` values are vars, not secrets). |

**P3 done-when proofs (all measured, not claimed):**
- Hello Dolly 3/3 functions, 2/2 hooks, 0 unconverted (unit + smoke).
- TT25 23/76/0 of 99 (unit test, exact v1 M11 record).
- Mock AI task → approval → exactly one draft (unit + smoke, draft count +1).
- Mock checkout → paid exactly once, replay duplicate, bad signature 401 (unit + smoke).
- WXR/SQL fixture counts match v1 (unit + smoke).
- Backup verifies and restores (unit + smoke).
- GraphQL returns the same post as REST under the same guard (smoke, live).
- Subscribed webhook fires exactly once per event with a verifiable signature + delivery-log entry (unit + smoke; delivery to a dead URL records `failed` with backoff).
- Security Centre shows audit log, lockout state, 2FA status, headers in one screen (smoke).

**P3 bugs found & fixed by the wiring/smoke runs (all verified):**
1. **v1 test-expectation bug (verified in v1):** v1's `conversion.test.ts`
   expects an upload job for `hello.php` to report slug `"hello-dolly"`,
   but v1's own `service.ts` slugs uploads via `slugifyFilename` =
   `"hello"` (the `"hello-dolly"` slug applies only to fixture jobs);
   the test never runs in v1 (its fixture path does not exist, so it is
   skipped). The Rust implementation follows v1's service code.
2. **`Some(Value::Null)` product fields:** the first wiring passed
   `Some(body["imageId"])` for absent fields and `require_int(Null)`
   400d every product create. Routes now use `json_field()` (absent or
   JSON null → `None`, matching v1's `??` semantics) for both product
   POST and PUT.
3. **Payments route shapes:** v1's `GET /api/commerce/payments/providers`
   returns just the array; `GET .../selected` returns `{provider, mode}`
   (not `{selected, mode}`); `POST .../select` always (re)sets the
   provider (absent clears it) and returns `{provider, mode,
   providers}`. The Rust getter is `get_selected_provider_id`
   (returns `Option<String>`), the setter `set_selected_provider_id`
   (takes `Option<&str>`) — both in `payments.rs`, not the names the
   wiring first assumed.
4. **`start_checkout` return URLs:** v1 appends `?order=<id>` to the
   success/cancel URLs so the result pages can load the order. Added
   `append_order_query` in `wordless-commerce` (unit-tested).
5. **WASM size (see Measurements):** first P3 build 5,017,385 bytes.
   `strip = true` → 4,319,346. `opt-level = "z"` → **3,179,092**
   (33,364 bytes / 1.1% over the limit). `panic = "abort"` was tried
   and made it slightly LARGER (3,182,736) — reverted, with a comment
   in `Cargo.toml` saying so. The profile change is recorded in
   `Cargo.toml` (`[profile.release]`).

**WASM size — the honest state:** **3,179,092 bytes (101.1% of the
3 MiB free-tier limit)** with the size-tuned profile. The binary is
OVER the limit until wasm-opt `-Oz` is run on a machine that can
download binaryen (expected to close the 1.1% gap — not measured
here). Alternatives if `-Oz` does not close it: split the conversion
engine out of the Worker (it already runs as an async job — the
natural split candidate), or Workers Paid ($5/mo, 10 MB limit).
Do not deploy this binary to the free tier as-is, and do not claim
the size is fine.

## What P4 (API) built (2026-11-19)

P4's API half (§0.1 A entry criteria + login customiser + dashboard-theme
contract fix). The React admin redesign itself is the parallel admin track;
this section is the worker side only. Route count: **173 v1 routes + 19 new
P4 routes** (10 CPT/taxonomy, 2 profile, 1 change-password, 1 plugins list,
2 plugin activate/deactivate aliases, 1 post meta, 2 plugin settings) +
`GET /api/login-branding` + the `publishAt` / `featuredImageId` behaviour
on the existing post create/update routes.

New/changed routes (guard = the router's `RouteOptions`; `capability`
implies authentication):

| Method + path | Guard | Notes |
| --- | --- | --- |
| `GET /api/post-types` | public | Built-ins (`post`, `page`) first, then registered types |
| `POST /api/post-types` | `manage_options` (admin-only) | 409 `post_type_exists`, 400 reserved slug |
| `GET /api/post-types/:slug` | public | 404 unknown |
| `PUT /api/post-types/:slug` | `manage_options` | Built-ins 400; 404 unknown |
| `DELETE /api/post-types/:slug` | `manage_options` | 409 `post_type_in_use` while posts exist |
| `GET /api/taxonomies` | public | Built-ins (`category`, `tag`, `nav_menu`) first |
| `POST /api/taxonomies` | `manage_options` | 409 `taxonomy_exists`, 400 reserved slug |
| `GET /api/taxonomies/:slug` | public | 404 unknown |
| `PUT /api/taxonomies/:slug` | `manage_options` | Built-ins 400; 404 unknown |
| `DELETE /api/taxonomies/:slug` | `manage_options` | 409 `taxonomy_in_use` while terms exist |
| `GET /api/users/:id` | auth; self or `edit_users` (else 403) | NEW — v1 has no single-user GET; returns the profile (below) |
| `PUT /api/users/:id/profile` | auth; self or `edit_users` (else 403) | displayName, email (409 `user_exists`), bio, avatar, theme |
| `POST /api/auth/change-password` | auth | current + new password (min 8); clears the forced-change flag, revokes all sessions (client signs in again) |
| `GET /api/plugins` | `install_plugins` | Installed list + active state + compat verdict summary (read-only view over `wl_extensions`) |
| `POST /api/plugins/:slug/activate` | `install_plugins` | Thin alias of the marketplace activate route |
| `POST /api/plugins/:slug/deactivate` | `install_plugins` | Thin alias of the marketplace deactivate route |
| `GET /api/login-branding` | **public, no auth** | Login-page customisation (below); read-only |
| `POST/PUT /api/posts` | unchanged (`edit_posts`) | Now accept `publishAt` (scheduling, below) and `featuredImageId` (below) |
| `PUT /api/posts/:id/meta` | `edit_posts` | Set/delete postmeta keys (flat object; `null` deletes); returns the full meta map; `_wl_publish_at` blocked, `_thumbnail_id` validated as an attachment id |
| `GET /api/plugins/:slug/settings` | `install_plugins` | Stored settings merged over the carried-over `optionDefaults` (below) |
| `PUT /api/plugins/:slug/settings` | `install_plugins` | Merges a patch into the stored settings (`null` deletes an override); returns the effective settings |
| `GET /api/posts/:slug` | unchanged (public) | Lazy publish check + `publishAt` / `featuredImageId` in the response |

**Scheduling — decision: `draft` + a `_wl_publish_at` meta, NOT a `future`
status.** v1 has no `future` status (the `PostStatus` enum asserts it is
invalid, and every public query filters `post_status = 'publish'`), so
adding a fifth status would touch the enum, all status validation, the
GraphQL filter, the render queries, and the v1 contract. Instead a
scheduled post stays `draft` (already invisible on the public site — every
render/feed/sitemap query requires `publish`, and the API single-post
handler 401s anonymous readers of drafts) with its target time in
`wp_postmeta` under `_wl_publish_at` (plain ISO-8601 text, no migration).
Two flip paths call the same service (`crates/wordless-cms/src/scheduling.rs`):
(1) a **Cron Trigger sweep** — `#[event(scheduled)]` in the worker entry,
`[triggers] crons = ["*/5 * * * *"]` in `wrangler.toml`, flips every due
draft every 5 minutes on deploy; (2) a **lazy publish check** on
`GET /api/posts/:slug` (and on create/update via `set_publish_at`), so a due
post flips the moment anyone reads it — the verifiable half locally, since
local `wrangler dev` fires no real cron (the sweep is the same code,
unit-tested; its *firing* can only be observed on a deployed Worker, P5 —
though `wrangler dev` documents a local test URL,
`http://localhost:<port>/cdn-cgi/local/scheduled`). `publishAt` semantics:
future → stored as draft with the meta (a `status: "publish"` in the same
request is overridden — no `post.published` webhook fires yet); past/present
→ publishes now; explicit `null` → clears the schedule; absent → no change;
a manual `status: "publish"` with no `publishAt` clears a stale schedule.
The single-post response carries `publishAt` when a schedule is pending
(list responses are unchanged — no API break).

**Forced password reset — changed vs v1.** v1's admin reset *cleared*
`wl_password_reset_required`, so the user signed in with the temporary
password and was merely asked by email to change it; there was no
change-password endpoint and nothing enforced it. P4 adds a second flag,
`wl_password_change_required`: the admin reset now **sets** it (and still
clears the imported flag, as v1 did); `login_guarded` is unchanged (the
imported flag still 403s — an imported phpass hash has no known password —
while the change flag does *not* block sign-in); the login and `/me`
responses carry `mustChangePassword: true` while it is set; `POST
/api/auth/change-password` sets the new hash, clears the flag, and revokes
all sessions. The v1 route `POST /api/users/:id/reset-password` is the
admin-initiated reset — no separate `POST /api/auth/password-reset` alias
was added (it would duplicate it; the doc's name differs from v1's actual
route, and v1 source wins per doc §2.5).

**User profile contract (`wp_usermeta`, all JSON-encoded per v1).**
Bio is the WordPress core key `description`; avatar is the NEW key
`wl_avatar_id` (a media attachment `wp_posts` id — v1 has no avatar key;
WordPress core uses Gravatar, not a stored attachment); theme is the NEW
key `wl_admin_theme`, one of the 8 ids `teal, midnight, ocean, forest,
sunset, berry, slate, contrast` (400 otherwise). **Theme null contract
(admin-foundation fix): `GET /api/users/:id` returns `theme: null` when
nothing is stored — never a server-side default — so the admin can tell
"unset" from an explicit `"teal"` pick and apply the role-based default
itself (Administrator→teal, Editor→ocean, Author→slate,
Contributor→slate, Subscriber→berry).** An invalid stored theme also reads
as `null`.

**Login branding contract (`wp_options`, JSON-encoded).** Public
`GET /api/login-branding` returns `{ logo, accent, deep, bgMode, bgImage,
layout, heading, tagline, showSocial, showBackLink, customCss }` from keys
`wl_login_logo`, `wl_login_accent`, `wl_login_deep`, `wl_login_bg_mode`,
`wl_login_bg_image`, `wl_login_layout`, `wl_login_heading`,
`wl_login_tagline`, `wl_login_show_social`, `wl_login_show_back_link`,
`wl_login_custom_css`. Defaults reproduce the v1 login look when nothing is
stored: accent `#0d9488`, deep `#07332e`, bgMode `"soft"`, layout `"card"`,
heading `"Welcome back"`, tagline `"The autonomous website operating
system"`, showSocial `false` (v1 login has no social links), showBackLink
`true` (v1 has "← Back to the site"), logo/bgImage/customCss `""`. A bad
stored value (bad hex, wrong type) reads as its default — branding never
500s the login page. **Write path (found by checking, not assumed): there
is NO `PUT /api/options/:key` route in v1 or v2, and `PUT /api/settings`
previously accepted only the 7 fixed site fields (unknown keys ignored) —
so the Settings screen could NOT have written these keys. P4 extends the
settings patch with the eleven branding fields (`loginLogo`,
`loginAccent`, `loginDeep`, `loginBgMode`, `loginBgImage`, `loginLayout`,
`loginHeading`, `loginTagline`, `loginShowSocial`, `loginShowBackLink`,
`loginCustomCss`; colours must be `#rgb`/`#rrggbb`, 400 otherwise) stored
under the `wl_login_*` keys; the 7 v1 fields behave exactly as before.
`GET /api/settings` still returns only the site settings — read branding
from `GET /api/login-branding`.**

**Plugins decision.** `GET /api/plugins` was added as a thin read-only
view: the Plugins page needs installed list + active state + verdict
summary without the full per-file report, and `GET /api/extensions`
ships the whole report JSON per row. Activate/deactivate are aliases of
the marketplace routes.

**Per-plugin settings (follow-up, 2026-11-19).** `GET/PUT
/api/plugins/:slug/settings` now exist. Backing store: one `wp_options`
row per plugin, `wl_plugin_settings_<slug>`, holding a JSON object —
chosen over a `wl_extensions` column (no migration, no schema change)
and over the raw option keys the install path migrates (those are the
plugin's runtime options, read by its converted code via `get_option`,
not the settings page's store). GET returns the stored overrides merged
over the carried-over defaults — the compat report's top-level
`optionDefaults` (`{key, defaultValue}` per literal `get_option('key',
default)` call in the PHP; first occurrence per key wins). PUT merges a
patch into the stored object (`null` deletes an override, so the default
shows again) and returns the effective settings. Validation is shape-only
(no schema exists for arbitrary plugins): object, ≤ 100 keys, ≤ 64 KiB
serialised, keys non-empty / ≤ 200 chars / no control characters; 404
when the plugin is not installed. A corrupt stored row (invalid JSON)
reads as "no overrides", never a 500.

**Featured image + post meta (follow-up, 2026-11-19).** v1 has no post
featured-image support at all (the admin's `featuredImageId` only existed
on products), and `set_post_meta` existed in the service with no route
exposing it — the editor's `PUT {"featuredImageId": 1}` returned 200 and
silently dropped the field. Fixed both ways: `POST/PUT /api/posts` accept
`featuredImageId` (absent = no change, `null` = clear, number = set) and
write the WordPress-standard `_thumbnail_id` postmeta row; the id is
validated as an existing `attachment` post (400 otherwise — a dangling
thumbnail would render as a broken image), before any row is written on
create so a bad id cannot leave an orphaned post. The single-post
responses (create/update/`GET /api/posts/:slug`) carry `featuredImageId`
when one is set (absent otherwise; list responses unchanged — same
pattern as `publishAt`). `PUT /api/posts/:id/meta` (guard `edit_posts`)
takes a flat object of key → value (`null` deletes; strings/numbers/
booleans stored as their string form; anything else 400), returns the
post's full meta map, and validates the same way: `_thumbnail_id` must be
a valid attachment id, `_wl_publish_at` is rejected (schedules go through
`publishAt`, which validates the ISO time and forces a future post to
draft — a raw meta write would bypass both). Changing the thumbnail on a
published post purges its cached URLs.

**WASM size:** `index_bg.wasm` **3,318,127 bytes** at the P4 follow-ups
(P3: 3,179,092; **+139,035, +4.4% — now 105.5% of the 3 MiB free-tier
limit**; vs the P4 end state 3,291,933 it is +26,194, a span that covers
both the external default-theme 0.7.0 bundle refresh and this follow-up's
routes — the two were not measured separately, so no split is claimed).
The P4 growth itself (+112,841) was larger than "routes only" because
wiring the routes pulls the previously dead-code-eliminated CPT/taxonomy
register/update/delete service functions into the binary for the first
time (they were only referenced by unit tests), plus the new
profile/scheduling/plugins/branding code and the scheduled handler. The
over-limit blocker from P3 is therefore worse, not better — wasm-opt
`-Oz` (still unmeasurable in this VM) or the split/paid-tier decision is
now more urgent, and must be settled before any deploy.

**What P3 did NOT do (explicitly out of scope / still open):**
- The **parity harness** from doc §5.1 (Node, identical seeded D1,
  all 173 routes, zero unexplained diffs) does **not** exist yet —
  the P3 smoke suite (273 checks) covers the route surface live, but
  it is not the per-route parity comparison the doc requires. That is
  P5 work (with the staging deploy).
- **P1's staging-deploy exit gate** is still not done (local
  verification only; staging deploy is P5).
- **P3's own exit gate** (parity suite on staging) is therefore NOT
  met — P3 is verified locally only. P4 may start (its entry criteria
  are API-level, and the API is complete), but the P5 cutover gate
  still requires the staging parity run.
- Live Workers AI calls, live payment-provider checkouts, and live
  HTTP email delivery have never been observed (no keys/accounts
  connected, by design).
- Converted plugins/themes still do not run (P6 extension runtime).
- No CPT/taxonomy REST routes (service only) — P4 added them (see
  "What P4 (API) built").
- Comment notification and reset-password email are still unwired.

## What P4 built — admin foundation (2026-11-19, foundation only — no page redesigns)

The React admin SPA (`admin/`) redesigned at the foundation layer against
the **unchanged** v2 API: tokens, themes, element + icon libraries, and the
theme wire-up. No page was redesigned in this workstream (wave 2 migrates
the 20 pages onto the new elements); every existing page renders as
before, re-skinned through tokens. Verified: vite build + `tsc -p
admin/tsconfig.json --noEmit` both pass.

**Package + build script.** `admin/package.json`: react + react-dom
^18.3.1; devDeps vite ^6, @vitejs/plugin-react ^4.3.0, typescript ^5.9.0,
@types/react + @types/react-dom ^18.3.0, @types/node ^24.0.0 (same
versions as the TS repo's package.json). `scripts/build-admin.sh` builds
the admin: on a normal machine it runs `npm install` in `admin/` first
if `node_modules` is absent, then vite build; in this VM (npm registry
unreachable) it uses the TS repo's vite binary with the existing
`admin/node_modules` symlink (a VM-local build aid, never committed —
see the script's header comment). Dist layout unchanged: `dist/admin/`
(vite outDir `dist/admin`, base `/admin/`), SPA entry at
`/admin/index.html`.

**Token / theme system.** `admin/src/styles.css` is rewritten around the
preview's token system (`--bg/--surface/--surface2/--ink/--ink2/--ink3/
--line/--teal/--teal-deep/--teal-ink/--teal-soft/--amber/--red/--blue` +
radius/shadow). All **8 themes** (teal, midnight, ocean, forest, sunset,
berry, slate, contrast) are token sets with the **exact `--th-*` vars
copied from the preview THEMES array**, applied via `data-theme="<id>"`
on `<html>`; default theme teal. True dark mode is the `midnight` theme
(not prefers-color-scheme). Focus-visible styles, `prefers-reduced-motion`
handling, 360 px mobile-first rules, and density hooks
(`.density-compact` on root, default comfortable) are included. The
preview's Manrope/JetBrains Mono webfonts are intentionally NOT loaded
(offline-first admin, system font stack, no network assets). Legacy v1
var names (`--accent`, `--text`, `--border`, `--surface-muted`, …) are
kept as aliases so existing pages and their inline `var(--border)` /
`var(--surface-muted)` styles keep working. One intentional visual
correction: `.btn-primary`, `.badge.pass/.fail`, `.form-grid`, `.actions`
were used by pages but never defined in v1 (buttons rendered unstyled);
P4 defines them token-driven — recorded, not a redesign.

**Element library.** `admin/src/components/elements.tsx` — **25 exports**:
Button (primary/secondary/ghost/danger/dark, small/medium/large,
icon-only with required aria-label), Input, Select, Toggle (role=switch),
Tabs (arrow-key navigable), Modal + Drawer (Escape to close), Toast +
ToastProvider/useToast host, Table, Badge/Tag, Avatar + AvatarStack,
Breadcrumbs, Stepper, EmptyState, Skeleton, Progress (bar + range),
CommandPalette (Cmd/Ctrl+K, keyboard navigable, page + action entries),
Card, Alert (ok/warn/err/info), StatCard. One look, one behaviour, one
dark mode per element; no element hard-codes a colour outside the token
vars. `components/ui.tsx` is untouched (existing pages still use it);
wave 2 consolidates the two.

**Icon library.** `admin/src/components/icons.tsx` — **104 icons**: the
preview's full 72-icon set (ICON_LIB path data ported; 6 groups) + 31
v1-only icons still referenced by existing pages, kept in the same style
(+ `x` kept as an alias of preview Close — identical path data). The
Icon component keeps strokeWidth 2 (the existing v1 pattern; the preview
draws at 1.8 — path data identical). The 150+ count in doc §6 principle 6
is a [target], not the P4 bar — 150+ remains open. Full audit note at the
top of `icons.tsx`.

**Theme system + contract.** `admin/src/lib/theme.ts`: the 8 THEMES,
ROLE_DEFAULTS (Administrator→teal, Editor→ocean, Author→slate,
Contributor→slate, Subscriber→berry; Shop Manager→sunset, Teacher→forest,
Student→ocean kept commented as future roles P8/P9), `getEffectiveTheme`
(personal choice overrides role default), `applyTheme` (sets `data-theme`
+ local cache), `saveTheme` / `loadTheme`. Contract with the API worker
(landed in the Rust worker source during this task — `PUT
/api/users/:id/profile { theme }` writing `wl_admin_theme`, `GET
/api/users/:id` returning `UserProfile.theme`; verified in source,
**not yet verified live**): the theme store calls exactly that PUT; the
read is `GET /api/users/:id` (there is no `GET /api/users/:id/profile`
route). `/api/auth/me` carries no theme field (User =
id/login/email/displayName/role/registered). **Wrinkle, flagged for the
API worker:** `get_user_profile` returns `theme: "teal"` when nothing is
stored, indistinguishable from an explicit teal pick — so `loadTheme`
treats server-"teal" as "unset" when the role default is not teal and
applies the role default instead. Clean fix would be `theme: null` /
omitted when unset. If the API is unreachable, the client falls back to
localStorage (`wl_admin_theme`) and the top bar shows "Saved locally".
`App.tsx` applies the effective theme on load (cached first for no flash
— `main.tsx` applies it pre-render — then refined from the profile) and
exposes a theme switcher `<select>` with all 8 themes in the top bar;
Cmd/Ctrl+K opens the command palette (nav pages + New post + Log out).

**Bundle sizes (measured 2026-11-19, this VM).** Baseline (pre-P4):
JS 309,044 B / gzip 87.64 kB, CSS 17,593 B / gzip 4.47 kB. After P4:
**JS 325,290 B / gzip 92.75 kB** (+16,246 B, +5.3%), **CSS 31,942 B /
gzip 7.50 kB** (+14,349 B — the 8 theme sets + element library). Theme
logic verified in Node (esbuild bundle, stubbed fetch): 12 + 4
assertions pass, incl. role defaults, personal-overrides-role, the
teal-as-unset rule, and the no-API fallback. Static-server check: built
`index.html` 200, JS + CSS 200, bundle parses (`node --check`).
**Playwright was NOT available** (`/tmp/pw` absent, no playwright/jsdom
in node_modules), so no headless render / console-error check was run —
that check is still open (see below).

**What is left for wave 2.** Migrate the 20 pages onto `elements.tsx`
(and consolidate `ui.tsx`); run the §6.4 design QA checklist (1440/768/
360 px review, keyboard-only walkthrough, WCAG 2.1 AA contrast + focus
on login/dashboard/editor, icon audit, screenshot set retaken); verify
the theme contract live against the Rust worker once the P4 profile
work is deployed (PUT + GET round-trip, cross-device pick); decide the
API's unset-theme representation (null vs `"teal"`, see wrinkle above);
dashboard-themes check (every theme renders every screen; role defaults
for a fresh user of each role) and login-customisation screen (Settings —
logo/colours/background/layout/custom CSS) are still P4 scope, not done
here.

**Incidents during P4 (recorded honestly).** (1) The icon generator
briefly wrote its output to the TS repo's `icons.tsx` (OUT path pointed
at the reference file instead of the Rust admin). Caught immediately;
the file was restored byte-identical from the GitHub main copy
(md5-verified) — 414 lines, no P4 icons. No other TS-repo file was
touched. (2) The parallel API worker landed the P4 profile endpoints
(`PUT /api/users/:id/profile`, `GET /api/users/:id` with `theme`) in the
Rust worker source mid-task; the admin's read path was corrected from
the assumed `GET /api/users/:id/profile` to the real `GET /api/users/:id`
before the final build. No `.rs` file was edited by this workstream
(WASM still `index_bg.wasm` **3,179,092 bytes**, P3 build, untouched).

## What P4 (entry criteria UI) built (2026-11-19)

The §0.1 A entry criteria, admin side — the full editor, the dedicated
Plugins page, and full user profiles — built on the P4 foundation
(`elements.tsx` + token system + `icons.tsx` only; no one-off buttons,
inputs or cards; no emoji as UI; every icon-only control has an
aria-label). No `.rs` file touched, no TS-repo file written, nothing
pushed, live site untouched. The API worker's P4 endpoints were already
in the Rust worker source (`PUT /api/users/:id/profile`, `GET
/api/users/:id`, `POST /api/auth/change-password`,
`GET/POST/PUT/DELETE /api/post-types` + `/api/taxonomies`, `publishAt`
on post create/update, `GET /api/plugins` + activate/deactivate) —
every endpoint used here was verified in `routes.rs` / `routes_p3.rs`
before use. The P4 API smoke (`scripts/smoke-p4-api.sh`, 115 checks)
already covered those endpoints; this work verified the UI's use of
them live (clean-room D1, `wrangler dev` on :8799, script at
`/tmp/smoke-p4.sh` — ephemeral, not committed).

**Editor rebuilt** (`admin/src/pages/Editor.tsx`, + word-count helpers
in `admin/src/editor/blocks.ts`): category checklist (`GET /api/terms?
taxonomy=category`, inline add via `POST /api/terms`, assignments via
`POST /api/posts/:id/terms` with `termTaxonomyId`s), tag input with
suggestions from `GET /api/terms?taxonomy=post_tag` (Enter creates a
missing tag), excerpt field (stored as `post_excerpt`, up to 10,000
chars), schedule control (datetime-local → ISO-8601 UTC `publishAt`;
future time saves as draft, publishing now sends `publishAt: null` to
clear), revisions panel (list + restore + per-revision diff vs latest
via `GET /api/revisions/:from/diff/:to`), SEO panel kept with the live
snippet preview now wired to the excerpt fallback, plus the §6.3
polish: slash-command insertion (type `/` in an empty paragraph),
drag-to-reorder via the block handle (up/down buttons remain as the
keyboard route), live word count + reading time (~200 wpm), an
autosave status that is always truthful (dirty → saving → saved-with-
time, or failed-with-message — the old silent swallow is gone), and a
distraction-free toggle (hides the sidebar cards). Featured-image
picker is built (media-library modal, upload or pick) — but see the
blocker below: it cannot persist yet.

**Plugins page** (new `admin/src/pages/Plugins.tsx`, route `#/plugins`,
nav entry first in the System group, in the command palette via the NAV
list): `GET /api/plugins` list with name, version, active status and
compatibility verdict (compatible/partial/blocked/not-analysed) plus
converted/partial/blocked file counts; per-plugin detail from `GET
/api/marketplace/:slug/report` with top blocking reasons + file paths
and the blocked-file list; activate/deactivate via
`POST /api/plugins/:slug/activate|deactivate` (blocked plugins cannot
be activated from the page); carried-over settings shown read-only
from the conversion report. Activation records status only — converted
code does not run until the P6 sandbox (stated on the page).

**Users / profiles** (`admin/src/pages/Users.tsx`): profile editor
modal for any user (display name, email, bio up to 5,000 chars, avatar
picked/uploaded from the media library and stored as the avatar
attachment id via `PUT /api/users/:id/profile`), admin password reset
unchanged in flow (one-time temporary password modal) with the forced-
reset note, a "must change password on next login" badge from the
loaded profile, role descriptions in plain language in the create form
and the users table, and a self-service "Change my password" section
on your own profile (`POST /api/auth/change-password`; the API
revokes sessions and clears the cookie, so the UI offers a sign-in-
again reload). `App.tsx` only gained the `#/plugins` route + nav
entry; `lib/api.ts` + `lib/types.ts` only gained the new endpoint
helpers/types (`TermList`, `UserProfile`, `PluginSummary`,
`RevisionDiff`, `publishAt` on `Post`).

**Acceptance results (live, 2026-11-19, clean room, :8799).**
Publish a post with categories + tags + excerpt end-to-end: PASS
(post 2, category `p4-test-category` + tag `p4-test-tag` assigned and
still present on re-read, status `publish`, excerpt stored). Scheduled
post publishes at its time: PASS for the lazy-publish half — a post
with `publishAt` +90 s was forced to `draft`, and a single-post read
after the time returned `publish` with the schedule cleared; the
5-minute cron sweep itself cannot fire under local `wrangler dev`
(same caveat as the P4 API smoke). Revisions list + restore + diff:
PASS (4 revisions after two edits; diff `exact: true`). Plugins page:
PASS — hello-dolly installed via marketplace (verdict `compatible`,
1/1 converted), activate → `active: true`, deactivate → `false`; a
crafted `eval()` plugin (`blocked-demo`) lists as verdict `blocked`
with top reason `eval` on `bad.php` and the file reason "contains a
construct blocked by the conversion sandbox." Users: PASS — profile
edit (display name, bio, avatar attachment id 1) persisted across
re-read; reset returned a 16-char temporary password once; login
with it returned `mustChangePassword: true`; change-password then
login with the new password returned `false`. Bundle strings for all
three screens verified in the live-served JS. NOT verified live: a
headless-browser click-through (no Chromium/Playwright/Puppeteer in
this VM) — UI flows were verified via the API + the built bundle,
not by driving the SPA; the 360 px check is the existing mobile-first
CSS (single-column cards, no new styles added).

**Bundle sizes (measured 2026-11-19, this VM).** Editor-only
checkpoint (before Plugins/Users/App): **JS 362,740 B** (+37,450 B vs
the 325,290 B foundation build), CSS unchanged 31,942 B. Final build
**JS 400,530 B / gzip 118.31 kB, CSS 40,814 B / gzip 9.01 kB** —
that final number includes the parallel worker's concurrent edits to
other pages (its Health/Import rewrites landed in the same build), so
it is not this workstream's delta alone. `tsc -p
admin/tsconfig.json --noEmit` clean; `bash scripts/build-admin.sh`
green (final build 20:24 UTC, same hashes as the live-verified
bundle). Note: two transient build/tsc failures during the task were
the parallel worker mid-edit on `Health.tsx` (esbuild `Expected "}"
but found "==="` at 20:18–20:19); not this workstream's files, not
edited here, resolved when their edit landed.

**Blockers (API-side, recorded exactly — no UI workarounds).**
1. **Featured image cannot persist — NO postmeta endpoint exists —
   CLOSED by the P4 API follow-ups (see "What P4 (API) built").**
   `PUT /api/posts/:id/meta` and `featuredImageId` on post
   create/update now exist, with `_thumbnail_id` read back on the
   post JSON.
   `wordless-cms/src/postmeta.rs` but no route exposes it, and
   `POST/PUT /api/posts` ignore any featured-image field
   (`CreatePostInput`/`UpdatePostInput` have no thumbnail field; the
   `Post` JSON has no featured-image field). Verified live: `PUT
   /api/posts/:id {"featuredImageId": 1}` returns 200 but the field
   is absent from the post JSON; `PUT /api/posts/:id/meta` is 404.
   The editor's picker keeps the choice in state only and warns
   honestly that it will be lost on reload. Needed: `PUT
   /api/posts/:id/meta` (or `featuredImageId` on post create/update)
   + read-back of `_thumbnail_id` in the post detail. Until then the
   "publish with featured image" acceptance leg FAILS on the API,
   not the UI.
2. **Per-plugin settings do not exist — CLOSED by the P4 API
   follow-ups (see "What P4 (API) built").** `GET/PUT
   /api/plugins/:slug/settings` (§0.1 A) were deliberately NOT added
   by the API worker (comment in `routes_p3.rs`: no settings store
   exists). Verified live: `GET /api/plugins/hello-dolly/settings`
   is 404. The Plugins page shows the conversion's carried-over
   setting defaults read-only and says so.
3. **Forced password change is not enforced by the login UI — CLOSED
   by the screen-redesign workstream (see "What P4 (screen redesign)
   built").** `Login.tsx` now shows a forced-change step when the
   login response carries `mustChangePassword: true`, calls
   `POST /api/auth/change-password`, and re-signs-in with the new
   password before entering the admin; no skip. Live-verified in
   `scripts/smoke-p4-api.sh` (115 passed, 0 failed). One remaining
   gap: `App.tsx` session restore does not check the flag — recorded
   for the coordinator in that section.

## What P4 (screen redesign) built (2026-11-19)

Every remaining §6.3 screen, rebuilt ONLY from `components/elements.tsx`
+ tokens + `icons.tsx` (no one-off buttons/inputs/cards, no emoji as UI,
aria-labels on every icon-only control — enforced by the `Button` type;
keyboard-operable; usable at 360px via the existing 900/760/400px
breakpoints). No `.rs` file touched, no TS-repo file written, nothing
pushed, live site untouched. Two `tsc` errors found and fixed in this
workstream (`Health.tsx` missing `}` on a Card `sub` expression;
`Card` requires `children` — Import/Settings/ThemeEditor). Final:
`tsc -p admin/tsconfig.json --noEmit` clean; `bash scripts/build-admin.sh`
green; **JS 403,810 B / gzip 118.97 kB, CSS 40,814 B / gzip 9.01 kB**
(foundation was JS 325,290 B / CSS 31,942 B).

**Screens redesigned** (`admin/src/pages/`): Dashboard (command deck —
live totals only, next 3 actions from real queues with one-click into
each, all-clear empty state, theme switcher in the dashboard itself),
Login (see forced-change below), Posts, Media (drag-and-drop upload
with real XHR progress, exact-mime filter, client-side search/date
over the loaded page — labelled as such, the API has no server search;
derivatives note when unbound), AI Command Centre (conversation-style
composer, live task polling, approvals as cards, agent directory in
plain language, memory notes edit = delete + add — no update endpoint
exists), Commerce (orders pipeline with one-click transitions, revenue
chart from paid+fulfilled orders only, single currency, max 200 orders,
empty state when none), Conversion (Stepper, live progress, per-file
verdicts, dependency-free TS highlighter, "what this means" computed
from counts), Themes/customizer (live preview as you type, preset
palettes, per-block styles, reset with undo), Settings (grouped,
searchable, Appearance tabs), Security (status board: headers,
lockout, 2FA coverage, sessions), Backup (timeline with verify
badges), Import, Terms, Comments, Menus, Files, Health, Activity,
Studio, Seo (per-post fields + deterministic analysis; the per-URL
Lighthouse audit is P10 and is NOT built here). New shared libs:
`lib/contrast.ts` (WCAG ratios), `lib/loginBranding.ts` (see below).
Editor, Plugins, Users and `App.tsx` are the parallel worker's —
not touched here.

**Dashboard themes** (Settings → Appearance → Dashboard themes, and
the Dashboard switcher): all 8 themes with live swatch previews,
role-default display, personal override via `saveTheme` (`PUT
/api/users/:id/profile { theme }`), reset-to-role-default. Role
defaults: Administrator→teal, Editor→ocean, Author→slate,
Contributor→slate, Subscriber→berry (Shop Manager→sunset,
Teacher→forest, Student→ocean are future roles, kept as constants).
The API now returns `theme: null` when unset (P4 API fix), so the
earlier "teal means unset" client workaround in `lib/theme.ts` was
removed — an explicit stored teal now wins over a non-teal role
default. Themes check (static + token ratios, not a visual pass —
see QA): every redesigned screen uses only tokens/elements; no
hard-coded admin-chrome colours remain (only ThemeEditor preset
palettes, which ARE theme.json values, and Login branding colours,
which are the feature). Contrast per theme, ink/bg, ink2/surface,
link/surface — all ≥ AA 4.5: teal 14.94/7.64/5.06, midnight
16.05/7.51/9.86, ocean 15.58/7.27/6.70, forest 15.02/6.72/7.13,
sunset 15.39/6.58/7.09, berry 15.99/7.75/7.88, slate 16.30/6.32/10.35,
contrast 21.00/12.63/10.11. A per-theme visual re-check of every
screen in a browser is still open (no browser in this VM).

**Login customiser** (Settings → Appearance → Login page): logo
upload (media library) or URL, accent + deep colours, background
(soft / gradient / dark / photo), layout (centred card / split
panel), heading, tagline (blank hides it), social/back-to-site
toggles, custom CSS, live preview, WCAG contrast ratios computed
live with AA pass/fail badges and a warnings Alert that never
silently applies a failing pair. Persistence is the LANDED API
contract, not localStorage: public `GET /api/login-branding`
returns the eleven values (defaults reproduce the v1 login look:
accent `#0d9488`, deep `#07332e`, bgMode `soft`, layout `card`,
heading "Welcome back", tagline "The autonomous website operating
system", showSocial false, showBackLink true); writes go flat
camelCase through `PUT /api/settings` (`loginLogo`, `loginAccent`,
`loginDeep`, `loginBgMode`, `loginBgImage`, `loginLayout`,
`loginHeading`, `loginTagline`, `loginShowSocial`,
`loginShowBackLink`, `loginCustomCss`) and are stored under the
`wl_login_*` option keys (`wl_login_logo`, `wl_login_accent`,
`wl_login_deep`, `wl_login_bg_mode`, `wl_login_bg_image`,
`wl_login_layout`, `wl_login_heading`, `wl_login_tagline`,
`wl_login_show_social`, `wl_login_show_back_link`,
`wl_login_custom_css`); bad hex is rejected 400. The Login page
reads the endpoint with a local-copy/default fallback. A local copy
is kept only as a read cache. `bgMode` "image" on the wire is
normalised to "photo"; layout "card" is the centred card.

**Forced password change — the entry-criteria gap in Login is now
CLOSED in this file** (this closes item 3 of the blockers in "What
P4 (entry criteria UI) built" above, with one caveat recorded
below). Verified in the API worker's routes: the login and `/me`
responses carry `mustChangePassword: true` while the admin-reset
flag (`wl_password_change_required`) is set. `Login.tsx` now reads
it from `api.login` (types extended in `lib/api.ts` — the flag was
already on the wire, just untyped): on `true` the user is NOT let
into the admin; instead a forced-change step appears in the same
card (new password + confirm, ≥ 8 chars, different from the
temporary password, visibility toggle, focus moves to the field),
calling `POST /api/auth/change-password` with the just-used
temporary password as `currentPassword`. The change revokes every
session and clears the cookie, so the page signs in again with the
new password before calling `onLoggedIn`. There is no skip button;
the only exit is "Back to sign-in", which logs nothing in — the
flag stays set server-side, so the next sign-in prompts again.
Live-verified via `scripts/smoke-p4-api.sh` (115 passed, 0 failed,
clean room, :8799): temp login signals `mustChangePassword`,
`/me` signals it, wrong current → 400, change with temp as current
→ 200 `{"changed":true}`, old temp login → 401, new-password login
→ `mustChangePassword: false`. The forced-change strings are in the
live-served bundle (`index-OglReG3E.js`: "Choose a new password",
"Set new password and sign in", "Back to sign-in",
`mustChangePassword` ×3). **Session restore — CLOSED by the
coordinator (2026-11-19):** `App.tsx` now checks the flag on `/me`;
when set it revokes the session (best effort), shows the sign-in page
with an explanation notice, and does not enter the admin — the Login
page's forced-change step takes over after sign-in with the temporary
password. The notice prop was added to `LoginPage` for this.

**Public default theme — 0.7.0** (`themes/wordless-default/`):
`style.css` header bumped 0.6.0 → 0.7.0; `theme.json` contentSize
680→720px, wideSize 1080→1140px, new "muted" palette colour
`#43584F`, new fluid "display" size (2.25–3.5rem), post-title
line-height 1.15, heading 1.2; link colour primary `#0d9488` →
accent `#0f766e` (primary-on-white is 3.74:1, below AA 4.5:1 for
normal text; accent is 5.47:1 — see QA); `home.html` gains a
generic "Latest posts" intro group (no invented copy; featured
images are NOT added — the renderer renders them as nothing);
post cards/post lists 1.5→2rem padding, shop price in accent,
footer padding 2.5rem, focus-visible rings, `::selection`,
`prefers-reduced-motion` disables card lift. Still one stylesheet
at `/theme.css?v=<hash>`; OG/Twitter head output is untouched Rust
(`wordless-seo/src/head.rs`) and was verified live, not regressed.
Live-verified (clean room, :8799, post "Hello from Theme 0.7"
published via API): home 200 contains "Latest posts", the post
title, `og:title`, `twitter:card`, `/theme.css?v=`, `og:type`,
`og:site_name`; `/theme.css` 200 (10,663 B) contains the muted
colour, 720px content size, display size, accent `#0F766E`;
`/admin/` 200 serves the new bundle; `GET /api/login-branding`
live returns the v1 defaults.

**QA (§6.4) — per item, with evidence.**
- 1440/768/360px review: OPEN — no headless browser in this VM
  (`/tmp/pw` Playwright absent; `shots-capture.mjs` cannot run).
  Static check only: breakpoints 900/760/400px in `styles.css`,
  P4 styles collapse the customizer/preview at 900px and hide the
  preview panel at 400px; no redesigned page sets a fixed width.
- Keyboard-only walkthrough (login → publish → approve an AI
  task): OPEN — same missing-browser reason. Static: all controls
  are native buttons/inputs/selects or elements with arrow-key
  Tabs; focus moves to the TOTP field and to the new-password field
  when those steps open; focus-visible rings on links/buttons/
  inputs in both admin and public theme.
- WCAG 2.1 AA contrast + focus: PASS (computed, not eyeballed) —
  all 8 admin themes ≥ 4.5:1 for ink, ink2 and links (table above);
  public theme: contrast `#122019` 16.85:1, secondary `#64748b`
  4.76:1, muted `#43584F` 7.64:1, accent `#0f766e` 5.47:1, excerpt
  `#3d4f48` 8.71:1; the one failure found and fixed was the public
  link colour primary `#0d9488` at 3.74:1 (now accent). Login
  customiser computes the same ratios live for custom colours and
  warns on AA failure.
- Icon audit: PASS (static) — every `iconOnly` usage in the
  redesigned pages has an `aria-label` nearby; all icons used are
  from `icons.tsx` (104-icon set); zero emoji found in any
  redesigned page or lib file (unicode-range scan).
- Element-library review: PASS (static) — redesigned pages import
  only from `components/elements.tsx` (+ `icons.tsx`, tokens);
  `components/ui.tsx` is imported only by `Integrations.tsx`,
  which was not a §6.3 redesign target. One exception outside this
  workstream: `Editor.tsx:935` (parallel worker's file) hard-codes
  `#1a0dab` for the SEO snippet title — flagged, not edited.
- `prefers-reduced-motion`: PASS — already in `styles.css` for the
  admin; added to the public theme 0.7 stylesheet.
- No fake numbers/states: PASS (reviewed) — dashboard counts,
  revenue chart, conversion verdicts, health checks and AI
  approvals all render only what the API returned, with empty
  states where there is nothing; the only explicitly-labelled
  non-live note is the social sign-in toggle (no provider
  configured, so no button is rendered).

**Blockers / decisions for the coordinator.**
1. **Theme 0.7.0 registry mismatch — API worker must update two
   Rust lines** (editing `.rs` is forbidden to this workstream).
   `crates/wordless-themes/src/registry.rs:56`
   `DEFAULT_THEME_VERSION` is still `"0.6.0"` (with an asserting
   test at :1142), and the test at :1144 asserts the style.css
   header contains "Version: 0.6.0" — so the header now says 0.7.0
   while the const reports 0.6.0. Measured: `cargo test -p
   wordless-themes --lib` 61 passed, 2 failed — that test, plus
   `generated_default_theme_css_matches_v1_byte_for_byte`
   (:1157), whose golden is 2184 bytes / hash `50ae0b9fe999`
   from the old theme.json. New golden, measured both ways: the
   v1 Node generator (`generateThemeCss` on the new theme.json,
   0 parse errors) gives **2346 bytes / hash `b2f0cc32848c`**,
   and the Rust test now measures 2346 bytes too — the API
   worker should update the golden to those values and re-run.
2. **App.tsx session restore gate — CLOSED by the coordinator
   (2026-11-19)** — see the forced-change note above.
3. **No server-side media search** (`GET /api/media` has no search
   param) — Media search/date filters are client-side over the
   loaded page and say so; no AI memory-note update endpoint
   (edit = delete + add, stated in the UI); no API to change role
   defaults (constants in `lib/theme.ts`).
4. **No per-theme visual check and no keyboard walkthrough in a
   real browser** — Playwright is unavailable in this VM; the
   coordinator handles capture separately. Do not claim
   screenshots exist.
5. **WASM over-limit (carried over, not this workstream):**
   3,291,933 bytes per the P4 API section — do not deploy to the
   free tier as-is; wasm-opt `-Oz` remains the lever.

## What P5 built — parity suite, staged cutover (2026-11-19)

P5 is the proof phase: a permanent differential harness that runs the
same seeded D1 against v1 (TypeScript) and v2 (Rust) and diffs every
response, measured latency, a rollback rehearsal that swaps one D1
file between the two binaries, and the cutover runbook with the
deploy/seed guards the user's machine runs. **Nothing was deployed
from this VM** — there are no Cloudflare credentials here by design.

### The parity harness (`tests/parity/`)

- `gen-seed.mjs` → `seed.sql`: 32 deterministic INSERTs (3 users,
  5 posts incl. a revision, SEO postmeta, terms, 2 comments, options,
  a redirect, a 404-log row, a product, an AI memory fact). Fixed
  PBKDF2 salts, so stored hashes are byte-identical on both workers.
- `corpus.mjs`: **320 steps** in 34 groups — every v1 route (all 173
  registrations verified present in v2, guard comparison 0
  mismatches across all 173 shared routes) plus GraphQL, outbound
  webhooks, shop routes and the 27 extra v2 (P3/P4) routes. Payloads
  come from the smoke scripts and from reading v1 handlers.
- `harness.mjs` (Node 24, no deps): runs steps in lockstep against
  both workers, per-worker variables + auth slots (admin/editor/
  subscriber/cart), `{{var}}` resolution, multipart/form/JSON bodies,
  real HMAC webhook signatures (mock SHA256, Paystack SHA512,
  Flutterwave `verif-hash`, Stripe `t=<unix>,v1=<hmac>`), RFC 6238
  TOTP from the captured secret, `poll`, `expect` (incl.
  MATCH-BUT-UNEXPECTED), `compare` modes (json/text/status-only/
  bytes-hash), `v2only`, `knownDiff` classification, `ignorePaths`.
  The normalizer folds ISO/SQL/RFC-822 datetimes under time-ish keys,
  secret-ish keys, UUIDs, 32/40-hex strings, media-key hex segments
  and `Request #<n>`; `publishAt` is deliberately NOT folded (corpus-
  set, must match exactly). Autoincrement ids are compared EXACTLY —
  lockstep from an identical seed must assign identical ids.
  Writes `results/parity-report.json` + `results/latency.json`;
  exits 0 only on zero unexplained diffs.
- `known-diffs.mjs`: 17 classified entries, each with cause/decision/
  owner — version strings, the P4 SPA redesign, default theme 0.7.0
  vs 0.6.0, CPT archives now serving, tag archive fixed in v2 only,
  `?p=` single-post resolution, the AI binding declared on v2,
  backup size delta (2 extra tables from migration 0011), the
  conversion-fixture 502 (deliberate v2 design), the extra
  `mustChangePassword` field, and the backup-schema delta.
- `run-parity.sh`: clean room (`rm -rf` both `.wrangler` dirs),
  v1 scratch copy via tar (the TS repo is never modified;
  node_modules symlinked back), fixtures built by python3 (PNG,
  theme ZIPs, hello-dolly.zip — `rm -f` first: `zip -r` UPDATES an
  archive instead of replacing it, and a stale flat `hello.php`
  broke the marketplace install in run 3), migrations + seed on
  both, both workers on :8801/:8802, harness run, cleanup trap.
  Gotcha: wrangler local D1 rejects explicit `BEGIN`/`COMMIT` in
  `d1 execute` — the seed has no transaction wrapper.

### Parity runs (measured, local workerd)

| Run | Steps | Match | DIFF | Unexpected | Known-diff | v2-only | Unexplained |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 2 | 318 | 200 | 79 | 17 | 2 | 20 | 96 |
| 3 | 320 | 243 | 21 | 9 | 13 | 34 | 30 |
| 4 | 320 | 262 | 4 | 0 | 16 | 38 | 4 |
| **5** | **320** | **265** | **0** | **0** | **17** | **38** | **0 — exit 0** |

(DIFF here is the true unexplained-diff outcome count. The harness's
console summary before the P5 counter fix printed DIFF including
known diffs — run 2 printed 81, run 3 printed 34, run 4 printed 20 —
and its old `unexplained` total (98 / 43 / 20) counted known diffs
as unexplained too. The fixed counter reports the true unexplained
count; run 5's report confirms it: `diff: 17, knownDiff: 17,
unexplained: 0`.)

Latency (local workerd, ms — same band, not a Cloudflare claim):
run 5: **v1 p50 57 / p95 207.5 | v2 p50 68.2 / p95 211.3**.
Per-group p50/p95 in `tests/parity/results/latency.json`.

### v2 bugs the harness found and fixed (F1–F15 + run-4 triage)

- **F1** Redirects (301/303) carried `Content-Type: text/html` (v1
  has none) — `RouteResponse::empty(status)` in the kernel; and
  **G1** empty bodies got `application/octet-stream` from workerd's
  `from_bytes(vec![])` — empty responses now use `Response::empty()`.
- **F2** Commerce checkout returned 200 (v1: 201) and built URLs
  from the request host (v1 uses the stored `siteurl` option).
- **F3** Shop POST forms read snake_case fields; v1's forms use
  camelCase (`productId`, `customerName`, `customerEmail`, `orderId`,
  `reference`).
- **F4/F5** Mock checkout rendered a 200 test page / 404d on missing
  orders; v1 303s in every branch (`/shop/checkout/test`,
  `/shop/checkout/success`, `/shop/cart?notice=…`).
- **F6** Shop GET pages had no SEO head — `with_seo` attached with
  v1's titles/canonicals (success AND cancelled canonicalise to
  `/shop/checkout/success`).
- **F7** Security overview reshaped to v1's exact shape (`headers:
  {policy, applied}`, `twoFactor` list, `sessions: {active}`,
  `failedLogins`); the P3 struct matched neither v1 nor its own SPA.
- **F8** ImportReport reshaped: `counts: {imported, skipped,
  failed, byType}` (v2 had the byType nested wrong and flat
  post/page/user/term/comment counts v1 never had).
- **F9** Theme imports returned `report.conversion: null` for
  PHP-free themes; v1 returns the (empty) report + theme.json
  settings summary on every import — the `has_php` gate is gone.
- **F10** `GET /api/integrations/cloudflare` returned
  `{bindings: [...]}`; v1 returns the bare array.
- **F11** Backup ids were 16 random hex bytes; v1 uses UUID v4.
- **F12** `/tag/<slug>` 404d on BOTH workers — the term lookup
  queried `taxonomy = 'tag'` but tags are stored as `post_tag`.
  Fixed in v2 only (v1 is frozen); the step is a known diff.
- **F13** Conversion fixture job: v1 creates the job (201, failed);
  v2 returns 502 with no job — deliberate v2 design, classified
  as a known diff, not changed.
- **Run-4 triage:** pages carried a BreadcrumbList v1 only adds
  for kind `"single"` (fixed: crumbs only for `PageKind::Single`);
  the classic-theme node count was off by one — v1's `analyze.ts`
  counts the program node itself via `walkAst`, while the Rust
  analysis excluded it (`count_ast_nodes`, used by the conversion
  report, correctly excludes it — it mirrors v1's `countAstNodes`);
  backup-create/import-create returned the full job view instead of
  v1's slim `{id, status, result}` / `{id, kind, status, result}`;
  the import report's `source` was the kind (`"wxr"`) instead of
  the uploaded filename (now threaded through from the job payload,
  as v1 stores it); backup size/files/restoredTables deltas are the
  2 extra 0011 tables, classified under `backup-schema-delta`.
- **Harness fix:** `totals.unexplained` counted known diffs as
  unexplained (`diffCount + unexpectedCount`); it is now
  `diffCount - knownCount + unexpectedCount`. Location headers are
  UUID-folded before comparing (fresh order UUIDs per worker).

### Rollback rehearsal (`tests/parity/rollback.mjs`) — 15/15 PASS

One shared D1 file, v1 → v2 → v1, on ports 8811/8812: fresh v1 D1
→ login + create a post → stop v1 → copy the D1 file into v2's
state dir → apply ONLY migration 0011 (the real upgrade path) →
start v2 → the **unchanged v1 session cookie still works**
(`GET /api/auth/me` 200), the v1 post reads, a v2 post creates →
stop v2 → copy the file back → start v1 → the same cookie still
works, the v2-created post reads, a new v1 post creates, all three
rehearsal posts list. This is the proof for the doc §1.3 claim that
rollback needs no data work. Run: `node tests/parity/rollback.mjs`.

### Cutover runbook + deploy/seed guards

- `docs/cutover-runbook.md`: prerequisites (size decision, written
  user approval, v2 repo location Q1, toolchain), exact PowerShell
  commands for the user's machine in order with expected output at
  each step, staging deploy + verify, the R3 deployed-PBKDF2
  measurement (bar: login CPU < 10 ms on the free tier), the staging
  soak checklist (incl. the P4 open items: §6.4 visual review at
  1440/768/360, per-theme re-check, keyboard walkthrough, screenshot
  retake, Galaxy A56 checks, the real 5-minute cron sweep),
  production cutover (backup first, then deploy, then verify incl.
  an existing browser session surviving), rollback
  (`wrangler rollback --env production`, or redeploy the v1 commit),
  and the 17 known diffs the user will see on staging.
- `scripts/deploy.mjs` (+ `package.json`, `npm run deploy:staging` /
  `deploy:production`): refuses without `--env` + `--confirm`; runs
  cargo tests + the WASM build; enforces the **WASM size gate**
  (3 MiB free / 10 MiB with `--paid`); refuses while any
  `[env.*]` id is still `REPLACE-ME` or the zero UUID; checks the
  `wrangler whoami` OUTPUT (it exits 0 even logged out — a guard-test
  catch); then remote migrations → deploy → health check. Verified:
  no-`--confirm` refuses; free-tier `--confirm` refuses at the size
  gate (3,328,519 bytes); `--paid` refuses at the placeholder check —
  all before any Cloudflare contact.
- `scripts/seed-admin.mjs` (+ `npm run seed-admin`): first-admin
  seed mirroring v1 (same rows, same PBKDF2 format, refuses when an
  admin exists); `--remote` requires `--env` (no silent production
  default — the first v2 admin belongs on staging). Verified:
  seeded admin logs in against the v2 worker (200); refusal path
  correct.
- `wrangler.toml` gained `[env.staging]` / `[env.production]`
  (REPLACE-ME placeholders + the production fill-ins: same D1 id
  `ebefbad6-…`, same R2 bucket, same KV as the live v1 worker) and
  per-env `[env.*.ai]` bindings — wrangler does NOT inherit the
  top-level `[ai]` into environments (its own warning, caught by
  the guard test).
- `npm install` in this directory installs wrangler 4.165.0 as a
  project dependency (verified: 80 packages, 17 s). Both scripts
  fail fast with "run `npm install` first" when it is missing
  (without it `npx` tries to download wrangler and fails with a
  cryptic EPIPE).

### What P5 did NOT close (owners in the runbook)

- **WASM over the limit: 3,328,519 bytes, 105.8%, 182,791 over**
  (+10,392 vs P4 — the parity fixes). Size decision resolved
  2026-11-19 ~23:30 WAT (user: no money, Workers Paid OUT, free tier
  hard limit; wasm-opt first, split the converter second) and the
  split was implemented 2026-11-20 (see "Converter split" below):
  main 3,004,187 (95.5%), converter 603,285 (19.2%).
- **R3 (PBKDF2 CPU)** open until the deployed measurement (runbook
  step 6). Local login wall ~86 ms is not the number.
- **P4 visual items** open until the staging soak (runbook step 7).
- **P5 exit gate NOT met**: no staging deploy and no production
  cutover happened here (no credentials by design); the gate needs
  the size decision, the user's written approval, and the user-run
  deploy + soak. Local evidence (harness 0 unexplained, rollback
  15/15) is complete.

## What P6 built — extension runtime: converted plugins actually run (2026-11-19)

P6 is the phase that makes "supports WordPress plugins" a true
statement. Before it, converted code was stored in R2 and never
executed; now a converted plugin or theme loads into a sandboxed
extension Worker of its own and its hooks fire, with every WordPress
call capability-checked against a per-extension grant. **Nothing was
deployed from this VM** — there are no Cloudflare credentials here by
design; the deployed proof for both production paths is still open.

### Sandbox decision (measured 2026-11-19, BEFORE any extension code ran — §7 Q10)

- **Dispatch namespaces (Workers for Platforms) are the design
  target, service bindings are the free-tier production path and the
  local/test mechanism** — one fetch-based protocol, two resolvers
  behind one `ExtensionExecutor` trait.
- **Dispatch: NOT locally testable.** wrangler 4.165.0 reports the
  binding as "not supported"; a dispatch fetch fails with `Binding
  EXT needs to be run remotely`. Needs a deployed test.
- **⚠️ Workers for Platforms is NOT free: $25/mo, no free tier**
  (verified against Cloudflare's WFP pricing page via two independent
  summaries; re-check at deploy time). The user has no money — not
  even Workers Paid ($5/mo) — so dispatch namespaces are gated on
  money, not code. The free-tier production path is **service
  bindings**: each extension is a normal account-level Worker (free
  tier, own 3 MB limit) bound as `[[services]] binding =
  "EXT_<SLUG>" service = "<slug>"`; the publish script adds the
  binding and redeploys the main Worker per extension change.
- **Service bindings: MEASURED working locally** (two-worker test,
  main → extension, 200 in 72 ms). The resolver tries, in order:
  `dev_url` (local) → service binding `EXT_<SLUG>` (free
  production) → dispatch namespace (WFP, $25/mo).
- **QuickJS-in-WASM (Boa 0.22): REJECTED, measured twice over.** A
  minimal Boa eval harness is **9,149,717 bytes (~8.7 MiB)** — three
  times the free-tier limit for the engine alone — and **panics at
  runtime** on wasm32-unknown-unknown (`std::time::Instant::now`
  inside `ContextBuilder::build`). rquickjs was not built (no wasm32
  C toolchain in this VM) — moot, since any real JS engine is ≥700 KB
  against zero headroom. The free-tier constraint (user decision
  2026-11-19 ~23:30 WAT: Paid OUT, 3 MB hard) decides Q10 on size
  alone as well as on isolation: dispatch namespaces add zero bytes
  to the main binary; Boa would add ~8.7 MiB.

### The crate: `wordless-extensions` (3,256 lines)

- `capabilities.rs` — the decide-now #4 capability model. **11
  capabilities** (`options.read/write`, `hooks.register`,
  `posts.read`, `posts.write.draft`, `media.read`, `users.read/write`,
  `network.declared`, `log`, **`theme.enqueue`**): every `wp.*` call
  the converter can emit maps to exactly one capability (or to
  "local" — a pure function in the shim). Unknown or unimplemented
  calls are load blockers with the exact reason. `theme.enqueue`
  was added during P6 smoke testing: TT25's `functions.php` calls
  `wp_enqueue_style`, which the emitter knows but the host did not
  implement, so the whole theme refused to load. The host now records
  enqueues in the extension log only — the theme engine serves the
  actual assets, so an enqueue can never inject a URL into a page.
  The `"log"` call is routable too (it was not: `logDenied` in the
  shim threw `not_implemented`, so undeclared-fetch denials were
  never recorded in D1).
- `host.rs` — the host protocol (`HostCall`/`HostResult`), per-load
  bearer token. **Token lifecycle fix:** the token is DERIVED, not
  random — HMAC-SHA256(secret, `"wordless-ext-host:v1:<slug>:<epoch>"`)
  with deployment secret `WORDLESS_EXT_HOST_SECRET`; load increments
  `token_epoch` and stores `token_hash(derived)`; dispatch re-derives
  the same token; the host endpoint verifies it. A random token could
  not be reproduced by a stateless Worker at dispatch time. If the
  secret rotates after load, host calls fail closed until reload.
- `registry.rs` — D1 service functions for instances/runs/logs
  (migration `0012_p6.sql`: `wl_extension_instances` incl.
  `token_epoch`, `wl_extension_runs`, `wl_extension_logs`).
- `loader.rs` — the load gate: refuses on blocked report files (with
  per-file reasons), no report, no converted code, hostile constructs
  (`eval(`/`Function(`/dynamic `import(`/`WebAssembly`), `fetch(`
  without declared hosts, unknown/unimplemented `wp.*` calls.
  Partial files load with warnings. Tolerates both report shapes
  (analyser `verdict`/`file`, auto-convert `status`/`file`).
- `script.rs` — the extension Worker project generator: entry (aliased
  `activate_N`/`renderedOutput_N`/`extensionMeta_N`, `/__wordless/invoke`
  + `/__wordless/health`), shim (frozen globals, capability-checked
  fetch, host-call transport, hook registry, sync-preserving
  `apply_filters`; the token is read from per-invocation context only,
  never baked in), `wordpress.d.ts` generated per type-import path,
  `wrangler.toml` (name = slug). The shim's `hostCall` reads the v1
  envelope correctly (`{ok, data}` wrapping the HostResult); a denial
  throws so the extension sees it.
- `executor.rs` — `ExtensionExecutor` / `ExtensionTransport` traits
  (both `#[async_trait(?Send)]`, matching the webhooks crate's
  `HttpPost` pattern), `race_with_timeout` + `worker::Delay` sleep on
  wasm32, `FetcherTransport` (wasm32-only), `FakeTransport` for tests.
- `hooks.rs` — `invoke_hook`: lists loaded instances, re-reads each
  instance's status immediately before dispatch (the kill switch),
  records every invocation incl. `skipped_disabled` in
  `wl_extension_runs`, threads filter values through extensions in
  registry order, 5,000 ms default timeout.
- `test_support.rs` — in-memory rusqlite `Db` + `SCHEMA_P6` for tests.

### Worker wiring (`crates/wordless-worker/src/routes_p6.rs`, 1,565 lines)

- Routes: `GET /api/extensions/runtime` (loaded + installed-not-loaded;
  see the regression note below), `GET /api/extensions/:slug`, `POST
  .../load|unload`, `POST .../disable|enable` (kill switch), `GET/POST
  .../permissions` (validates capability ids), `GET .../logs`, `GET
  .../runs`, `GET .../script` (regenerates the project on the fly),
  `POST .../host` (token-authenticated, no session auth), `POST
  .../invoke`, `POST /api/extensions/invoke-all`. Load supports
  `{"source":"theme"}` — theme conversion reports are read from
  `wl_themes.report_json` (no migration needed).
- Host-call implementations (`run_host_call`): options (writes scoped
  to `<slug>_*` keys + carried-over defaults), posts (insert forces
  draft, update/delete drafts only), user meta (actor-scoped),
  terms/categories/menus/revisions, media, and the enqueue arms
  (recorded only). `media_handle_upload` and `wp_schedule_event`
  are still load-blocking (not implemented).
- `Ctx` gained `ext_secret`, `ext_dispatcher_present`,
  `ext_resolver`; `lib.rs` `build_ext_resolver` resolves per instance
  in order: `dev_url` → service binding `EXT_<SLUG>` → dispatch
  namespace `EXT_DISPATCHER` (`wrangler.toml` declares the dispatch
  namespace binding at top level + per env; local dev reports it "not
  supported", expected).
- **Regression found and fixed in P6 (route shadowing):** the P6
  `GET /api/extensions` returned `{loaded, installed}` and shadowed
  the P3 route (an `ExtensionRecord[]` array) — except the kernel
  router returns the FIRST matching registration, so the P6 route
  was actually dead code and the P3 array kept winning (the smoke's
  list check only asserted the slug was present, so it passed either
  way). The admin UI had been built against the unreachable shape.
  Fixed: the runtime list moved to **`GET /api/extensions/runtime`**;
  `GET /api/extensions` stays the P3 array (Conversion page + parity
  corpus step preserved). The smoke now asserts both shapes.
- **P3 double-`converted/` prefix bug fixed (found during P6 theme
  work):** `convert_php_files` returns keys already prefixed
  `converted/<path>.ts`, but the P3 plugin route stored them at
  `extensions/{slug}/converted/{path}` → `converted/converted/hello.ts`.
  Fixed to `extensions/{slug}/{path}`.
- **Converter fix (Rust emitter, `crates/wordless-conversion/src/emitter.rs`):**
  the converted TT25 `functions.php` was nearly empty — functions
  declared inside `if (!function_exists('x')): ... endif;` guards
  were invisible to the emitter (it only collected top-level
  functions), so their `add_action`s were silently dropped, and
  `function_exists` was unmapped (emitted `__unmapped_function_exists`,
  which throws at module import, so the extension Worker could not
  boot). Fix: `collect_nested_functions` hoists guarded functions to
  module level (faithful to PHP, which defines them at compile time);
  `function_exists` with a literal arg resolves statically against
  the function table. Two regression tests added (`guarded_functions_hoist_and_register_hooks`,
  `converts_tt25_functions_php`: 7 functions, 6 hooks).

### Publish script (`scripts/publish-extensions.mjs`, `npm run publish-extension`)

User-machine script: `<slug> --env staging|production --confirm
[--dispatch-namespace <name>] [--source theme] [--skip-main-redeploy]`.
whoami guard (wrangler exits 0 even logged out). Downloads the
generated project from R2 via `wrangler r2 object get` (this wrangler
4.165.0 has NO `r2 object list` — converted paths are derived by
parsing the entry file's `./converted/<path>.js` imports). Default
path (free): deploy the extension as a normal account-level Worker,
add the `[[env.<env>.services]]` binding to the main wrangler.toml,
redeploy the main Worker (the deploy script enforces the WASM size
gate). Dispatch path: `wrangler deploy --dispatch-namespace <name>`
(needs WFP $25/mo; a 403 means the account lacks it). **NOT verified
live** (no Cloudflare account in this VM) — the first live run is a
real test.

### Admin UI — Plugins page (`admin/src/pages/Plugins.tsx`, 296 → 744 lines)

Built by a subagent against the P6 API contract (typecheck clean,
vite build 2.21 s, JS 414.64 kB): the outdated "converted plugin code
does not run yet" copy replaced with the three states (Installed /
Active / Loaded in sandbox); runtime state per plugin (loaded /
disabled / not loaded + granted permissions); Load (no body) / Unload;
Disable / Enable kill switch (stops hooks within one request,
distinct from Deactivate); permissions editor rendering the server's
`available` list as toggles (no hardcoded capability list); logs +
runs with level badges; load-refusal panel showing the exact
`reasons[]` and `blockedFiles[]`. `listExtensionRuntime()` now calls
`/api/extensions/runtime` (fixed after the route-shadowing fix above);
`listExtensions()` still returns the P3 array. `Conversion.tsx` copy
updated ("stored code runs once loaded through the sandboxed runtime").
No end-to-end verification of the admin UI yet — the parent verifies
live. Theme loading via `source=theme` has no Themes-page UI yet.

### P6 smoke (`scripts/smoke-p6.sh`, port 8800 main; 8801 hello-dolly; 8802 runaway; 8803 TT25) — 62/62 PASS

Clean-room (`rm -rf .wrangler` first). Hello Dolly end-to-end (ZIP →
marketplace install → compat report `compatible` → load with devUrl
→ assemble ext project from `/script` + converted files from local
R2 → start ext worker → health → invoke `admin_notices` → the lyric
HTML); host endpoint (wrong token → 401; `get_option` without
`options.read` → `capability_denied` + a `denied` log row; grant →
call succeeds); disable → invoke `skipped_disabled`, renders nothing;
re-enable → runs again; hostile plugin (`eval()`) blocked at analysis
and refused at load with the file named; TT25 import → load
`source=theme` (grants `theme.enqueue`, 99 convertedPaths) → invoke
`after_setup_theme` → status `error` with `Unmapped function
add_theme_support()` (the converted function RUNS and reports the
honest unmapped boundary); invoke `wp_enqueue_scripts` → status
`error` with `constant SCRIPT_DEBUG is not defined` (the callback
throws at the unconverted constant before reaching the enqueue call —
the enqueue host arm itself is proven by the routes_p6 unit test);
runaway (never-responding worker) → invoke returns promptly (0 s),
status `error`, main Worker stays alive (local workerd detects the
hung isolate and answers HTTP 500 itself before the 5,000 ms
client-side race fires; the 5 s race is unit-tested in
`race_with_timeout`); `GET /api/extensions/runtime` shape +
`GET /api/extensions` P3-array guard. The full 99-file TT25 entry as
a Worker is a known gap (template/pattern files' top-level code is
not a runnable module); the smoke proves functions.php only.

Smoke-test gotchas learned in P6: this wrangler has no `r2 object
list` (only get/put/delete) — list converted files via `/script`'s
`convertedPaths` or by parsing the entry's imports; `r2 object get
--local` works against the local bucket but returned a 0-byte file
once (the object WAS there — read the blob from
`.wrangler/state/v3/r2/.../blobs/<blob_id>` via the metadata sqlite
when that happens); a wrangler dev parent RESPAWNS workerd when it
dies, so cleanup must kill the parent by pattern (`pkill -f
"wrangle[r] dev --port <port>"` — the bracket avoids self-match)
before the ss backstop; a zombie workerd can LISTEN without
responding, so the health curl needs `--max-time 5` or the script
hangs forever (run 5 proved it).

### Parity after P6 — still green (re-run 2026-11-19)

**320 steps, 265 match, 17 DIFF (all 17 known-diff), 0 unexplained,
38 v2-only, exit 0** — identical to P5 run 5. Latency (local workerd):
v1 p50 61.5 / p95 215.9 | v2 p50 73.8 / p95 221.1 (same band as run 5:
57/207.5, 68.2/211.3). The P6 behaviour changes (converter fix, P3
double-prefix fix, enqueue capability, /runtime route) caused zero
parity regressions.

### Tests & size (measured 2026-11-19, this VM)

- `cargo test --workspace --lib`: **516 passed, 0 failed** (P5: 480;
  +36: extensions 28, conversion +2, worker +6). Per crate: ai 34,
  auth 25, backup 8, cms 48, commerce 54, compat 11, conversion 49,
  db 1, elementor 13, extensions 28, graphql 12, importer 17,
  integrations 42, kernel 17, media 28, render 21, security 3,
  seo 27, themes 63, webhooks 5, worker 10.
- WASM `index_bg.wasm`: raw (no-opt) **3,512,198 bytes (111.6% of the
  3 MiB limit, 366,470 over)** — P6 added 183,679 bytes over P5.
  With wasm-opt `-Oz` (binaryen 0.132, same flags as the parent's
  size-fix patch, measured on a COPY in a scratch dir — the patch
  itself is NOT applied to this workspace yet): **3,059,170 bytes
  (97.2%, 86,558 headroom, ratio 0.8710)** — P6 FITS the free tier
  once the patch lands. Functional verification of the P6 `-Oz`
  binary is still open (to be done after the patch is applied).
- Admin bundle: JS 414.72 kB / CSS 40.81 kB (vite build, after the
  Plugins page work).

### What P6 did NOT close

- **Deployed proof for BOTH production paths** (service bindings and
  dispatch namespaces) — needs a Cloudflare account; not claimed.
  The publish script is unverified live.
- **WASM size**: raw is 111.6% of the limit; `-Oz` brings it to
  97.2% (86,558 headroom) — measured, not yet applied or functionally
  verified on the P6 binary. The size-fix patch applies ONLY after P6
  lands (`npm install && npm run build && npm run size`).
- **R3 (PBKDF2 CPU)** still open until a deployed measurement.
- **P6 exit gate NOT met**: no staging deploy (no credentials by
  design); needs the size-fix patch applied, a staging deploy of one
  real extension through a published service binding, and the user's
  written approval.
- The full 99-file TT25 entry as a Worker (template/pattern files are
  render fragments, not runnable modules); `media_handle_upload` and
  `wp_schedule_event` still load-blocking; admin UI end-to-end
  verification; theme loading has no Themes-page UI yet.
- `P2-NOTES.md` still to remove before any push; v2 repo shape (Q1)
  undecided; Q11–Q14 undecided.

- Rust 1.100.0, wasm-pack 0.15.1, Node 24. **`/tmp` is a 512 MB tmpfs —
  always set `TMPDIR=$HOME/.tmp-cargo` for cargo installs/builds.**
- **`worker = "0.9"`, not the doc's pinned 0.5.** worker-build 0.9.0
  (installed) requires `worker >= 0.9.0`, and the 0.5-era builder
  (worker-build 0.1.14) hardcodes a wasm-bindgen CLI 0.2.105 that is
  incompatible with this workspace's wasm-bindgen 0.2.130 (js-sys/web-sys
  0.3.107 pin it exactly). worker 0.9.0 was released 2026-11-11; its
  pinned wasm-bindgen CLI is 0.2.130, exactly the workspace version. The
  D1/Env/Request/Response/event-macro surface used here is unchanged
  between 0.5 and 0.9 — the worker crate compiled cleanly after the bump
  with no code changes. **Flag for the parent: fold the 0.9 pin into
  doc v1.3.**
- **worker-build must run from the crate directory** (`cd
  crates/wordless-worker && worker-build …`). Invoking it with a PATH
  argument from the workspace root breaks its staging flow: the pkg lands
  in the real out_dir while the shim step looks for `staging/index.js`
  and fails with a bare "No such file or directory". `wrangler.toml`
  therefore builds via `bash scripts/build-worker.sh`, which cds first.
- **VM download workarounds:** this VM cannot fetch from GitHub releases
  or the npm registry (TLS `UnknownIssuer`). `scripts/build-worker.sh`
  supplies worker-build's tools from local installs when present:
  `WASM_BINDGEN_BIN=$HOME/.cargo-wb01/bin/wasm-bindgen` (wasm-bindgen-cli
  0.2.130, cargo-installed) and `ESBUILD_BIN` (v1 repo's Vite esbuild
  0.28.2, read-only reuse). On a normal machine both are unset and
  worker-build downloads its own tools.
- **`--no-opt`:** wasm-opt (binaryen) also cannot be downloaded here.
  The unoptimized WASM is ~800 KiB, far under the 3 MB script limit.
- `worker-macros` + `syn` workaround: worker-macros declares `syn 2`
  with no features, and nothing in the host graph enables `syn/full`,
  so the `#[event(fetch)]` expansion fails to compile. Fix in place:
  `[build-dependencies] syn = { version = "2", features = ["full"] }`
  in `wordless-db/Cargo.toml` + a trivial `build.rs` (a normal
  dependency does not fix it — only build-dependencies unify features
  for proc-macro crates). The "unused dependency `syn`" warning is
  expected. **Re-check at P2: does worker-macros 0.9.0 still need it?**
  (It compiled fine with the workaround in place; whether it is still
  *needed* was not re-tested after the 0.9 upgrade.)
- **Async is `?Send`, `Rc` not `Arc`:** the `worker` crate's D1/R2
  methods return `JsFuture`, which is `!Send` (holds an
  `Rc<RefCell<…>>`). The `Db`/`ObjectStore` traits are
  `#[async_trait(?Send)]`, the router's handler futures are not `Send`,
  and contexts hold `Rc<dyn Db>`. This matches the Workers runtime
  (single-threaded isolate, `spawn_local`).

## What P7 built — social networking: BuddyPress-like community (2026-11-20)

**Q11 decided first (doc §7): friends (mutual, request/accept), the
BuddyPress model; follows NOT built.** Reasoning: the brief is
explicitly "BuddyPress-like"; mutual consent gives a clean
privacy/trust story (messaging and friends-only visibility require
mutual acceptance); blocking severs the mutual edge both ways; the
feed is driven by friends + groups. `wl_follows` deliberately NOT
created — a future phase can add it without touching the friends model.

### The crate: `wordless-social` (profiles, friends, activity, groups, messaging, notifications)

- **Migration `0013_p7.sql`** — 9 tables (10 with the extra
  `wl_thread_participants`, required for 2-person threads + read
  state): `wl_social_profiles` (display name, bio, avatar/cover media
  ids, fields JSON, visibility public/friends/private),
  `wl_friendships` (requester, addressee, status pending/accepted/
  blocked — the block lives here, either direction blocks both ways),
  `wl_activity_items` (visibility public/friends/group/private,
  status visible/hidden/deleted), `wl_activity_likes`,
  `wl_groups` (public/private/hidden), `wl_group_members` (role
  member/moderator/admin, status member/requested/invited/banned),
  `wl_message_threads`, `wl_messages`, `wl_notifications`.
- **Visibility rules are pure functions** (`visibility.rs`, 14 unit
  tests, one per rule): blocked beats everything; hidden/deleted
  activity never appears in any feed; private-group content is
  members-only; hidden groups are unlisted for non-members. Feed and
  directory privacy filters run in SQL (indexed subqueries), so list
  totals are exact.
- **Friends state machine:** request → pending → accepted (or row
  deleted on decline/remove); `blocked` overwrites any state in
  either direction. A request to someone who already asked auto-
  accepts instead of duplicating.
- **Activity:** post (public/friends/group/private), like (idempotent),
  `@login` mentions notify the mentioned user, moderator hide removes
  the item from every feed/profile/group view (admin list still shows
  it as hidden), actor soft-delete.
- **Groups:** create (creator is first admin), public direct join,
  private join request → admin approve, hidden invite-only, ban
  refuses re-join, leave promotes the oldest active member when the
  last admin leaves. Invited users can see a hidden group to accept
  the invite (a bug the tests caught: invitees were 404'd).
- **Messaging:** two-participant threads created on demand; sending
  requires friendship OR shared group membership, never across a
  block; per-participant `last_read_at` unread counts.
- **Notifications:** in-app rows for friend requests/accepts, likes,
  mentions, group requests/approvals/invites, messages. The email copy
  is best-effort via the integrations mailer (mock provider only logs
  — never presented as delivered); a mail failure never fails the
  request.
- **Routes (`routes_p7.rs`, 30 routes):** profiles, members directory,
  friends (+ block/unblock/blocked list), activity (+ like/unlike/
  hide/admin list), groups (+ join/leave/approve/invite/ban/members/
  activity), message threads (+ send/read), notifications (+
  unread-count/mark-read), and public pages `/members`,
  `/members/:id`, `/groups`, `/groups/:slug`, `/activity` (standalone
  inline-CSS pages, same visibility rules as the API; router routes
  win over the public fallback).
- **Admin UI — Community page (`#/social`):** members directory with
  search, groups management (create/join/leave), activity moderation
  (hide), messaging overview. Admin bundle JS 423.09 kB (was 422.96
  kB), tsc clean.

### Tests & size (measured 2026-11-20, this VM)

- `cargo test --workspace --lib`: **540 passed, 0 failed** (P6
  baseline 516 + 24 new P7: 14 visibility-rule unit tests + 10
  service flow tests in `wordless-social`).
- Smoke `scripts/smoke-p7.sh` (port 8804): **99/99 PASS** — every P7
  flow plus all privacy denials (blocked profile 404, private-group
  activity 403, hidden group 404, stranger thread 403, like on an
  invisible item 403, subscriber hide 403). One smoke expectation
  was fixed during the run (directory total 3, not 4, while admin's
  profile is friends-only and ed is not yet a friend — the code was
  right, the expectation was wrong).
- Parity harness extended with 23 v2-only P7 steps: **343 steps, 265
  match, 17 known-diff, 0 unexplained, 61 v2-only, exit 0** (latency
  local workerd v1 p50 54.3/p95 210.1, v2 p50 73/p95 226 — same band).
- **⚠️ WASM: 3,204,402 bytes (101.9% of the 3 MiB free-tier limit,
  58,674 over) after wasm-opt -Oz** (raw 3,670,976). P6 merged tree
  was 3,059,170 (97.2%) — P7 added 145,232 bytes post-Oz. **No
  features were deleted to fit.** Consequence: `npm run build` FAILS
  (size gate exits 1) and `wrangler dev` cannot start — the smoke and
  parity runs above used the local-only bypass
  `WORDLESS_NO_WASM_OPT=1` (serves the raw build locally; the deploy
  gate still refuses the binary). **Decision needed: the
  converter-split fallback (SIZE-REPORT.md §5) or another lever.**

### What P7 did NOT close

- **WASM size (the blocker) — RESOLVED 2026-11-20 by the
  converter split (section below):** main 3,004,187 bytes (95.5%),
  converter 603,285 bytes (19.2%). P7 is deployable on the free tier
  again (subject to the staging gate).
- **P7 exit gate NOT met**: no staging deploy (no credentials by
  design); needs a staging deploy with a seeded community, and the
  user walking through it on a phone with moderation demonstrated live.
- **R3 (PBKDF2 CPU)** still open until a deployed measurement.
- No live email observed (mock provider only); no push notifications;
  no @mention autocomplete in the admin UI; messages are text-only
  (no attachments); no group activity email digest.

## Converter split — free-tier fallback (2026-11-20, between P7 and P8)

P7 pushed the single Worker over the free-tier limit (3,204,402
bytes, 101.9%), so the fallback the user approved on 2026-11-19
(wasm-opt first, split the converter second) was implemented:

| Worker | Raw build | After wasm-opt -Oz | % of 3,145,728 | Headroom |
|---|---|---|---|---|
| main (`wordless-worker`) | 3,443,402 | **3,004,187** | 95.5% | 141,541 |
| converter (`wordless-converter`) | 685,991 | **603,285** | 19.2% | 2,542,443 |

The split saved 200,215 bytes post-Oz (227,460 raw). `npm run build`
SUCCEEDS (size gate green) and `npm run size` exits 0, reporting
both Workers. No features were deleted to fit. Caveat: the main
Worker is 4,187 bytes over the 3,000,000 soft target — headroom is
thin, so P8 must watch size from the start.

- **What moved:** `wordless-conversion` (PHP lexer/parser/analyser/
  emitter) + `wordless-elementor` (content renderer) into a second
  Worker (`crates/wordless-converter/`, own `wrangler.toml`, envs
  `wordless-converter-staging` / `wordless-converter-production`).
  A new crate `wordless-conversion-jobs` (kernel + db only) holds the
  job-record code the main Worker keeps; job reports are stored
  verbatim as JSON, so the API response is byte-identical.
- **Two deviations from the literal plan, both measured:**
  `wordless-compat` did NOT move — `cargo tree` shows it was never
  linked into the main binary (moving it saves 0 bytes); the
  converter Worker has NO D1/R2 bindings — it is stateless compute
  (JSON in, JSON out), all persistence stays in the main Worker.
- **Wiring:** one `ConverterClient` trait (`wordless-kernel/src/
  converter.rs`); transport order mirrors P6's resolver —
  `WORDLESS_CONVERTER_URL` (plain HTTP, local dev) first, then the
  service binding `CONVERTER` (production). Converter routes:
  `GET /__convert/health`, `POST /__convert/convert-php`,
  `/__convert/analyze-files`, `/__convert/convert-files`,
  `/__convert/theme-settings`, `/__convert/render-elementor`.
  Failure semantics: marketplace report fail-soft (install proceeds
  unanalysed), conversion jobs fail explicitly, public Elementor
  rendering falls back to block content.
- **Local dev:** `npm run dev:split` (converter :8805, then main
  :8787) or `bash scripts/dev-split.sh`. Smoke/parity scripts start
  the converter themselves (`scripts/converter-dev.sh`) and pass
  `--var WORDLESS_CONVERTER_URL:http://127.0.0.1:8805` to the main
  `wrangler dev`. **Deploy:** `scripts/deploy.mjs` builds + size-gates
  BOTH Workers and deploys the converter first, then the main Worker,
  verifying `/api/health` and `/__convert/health`.
- **Re-verification on the split build (2026-11-20):** cargo test
  **549/0** (540 + 9 new); smoke-p6 **62/62**, smoke-p7 **99/99**,
  P4 API smoke **150/150**; parity **343 steps, 265 match, 17
  known-diff, 0 unexplained, 61 v2-only, exit 0**. End-to-end: Hello
  Dolly install → convert → report (compatible) → load →
  `admin_notices` hook ran and rendered the lyric; TT25 99 converted
  files.

## What P8 built — WooCommerce module: the store as a compatibility subset (2026-11-20)

**Q12 + reuse-or-extend decided first (doc §3 P8, before any code):**
import is **CSV only** — products in WooCommerce's native product
export format, orders in the de-facto WebToffee CSV shape (core Woo
has no native order exporter; a WXR order import would create orders
with no items), customers in WooCommerce's native customer export
columns. **Tables are extended, not duplicated** — `wl_products`
gains `type` + `sku`, `wl_order_items` gains `variation_id` +
`variation_label`, `wl_orders` gains `coupon_code`,
`discount_minor`, `tax_minor`, `shipping_minor`,
`shipping_method`, `source`, `source_id`; the
`wl_woo_products` / `wl_woo_orders` / `wl_woo_order_items` names
from the plan are deliberately NOT created (one order pipeline, one
cart, one webhook path). Money is integer minor units; tax in basis
points, round-half-up on (subtotal − discount); tax OFF and prices
exclusive by default, so P3 checkout totals cannot change until the
owner enables tax. A coupon's usage count increments exactly once,
atomically with the paid transition — never at apply time.

### The crate: `wordless-woocommerce` (4,347 lines, 10 modules)

- **Migration `0014_p8.sql`** — the ALTERs above (partial unique
  index on non-empty `sku`) + 8 new `wl_woo_*` tables:
  `attributes`, `variations`, `product_terms`, `coupons`,
  `tax_rates`, `shipping_zones`, `shipping_methods`, `customers`.
- **Variable products:** attributes (slug-derived, duplicate name
  refused, delete refused while a variation uses the attribute) +
  variations (validated against the parent's attributes, SKU unique
  across products AND variations, delete refused when in an order,
  parent price re-synced to the minimum variation price; effective
  price = sale price when > 0, else price). A cart line is (product,
  variation?) — the variation resolves server-side from the
  variation id (API) or the chosen attribute values (storefront
  form, no JavaScript). Parent stock is ignored for variable
  products; variation stock decrements on paid, restores on cancel.
- **Coupons:** fixed / percentage (basis points, half-up), min
  order, max uses, expiry, active flag; codes stored upper-case;
  validate never errors on a bad code (reports discount 0 + reason).
  A bad coupon is a 400 at checkout, reported as discount 0 +
  reason by the quote endpoint.
- **Taxes:** rates by country/state in basis points
  (most-specific match), `wl_woo_tax_enabled` /
  `wl_woo_prices_include_tax` in `wp_options` (both default false);
  inclusive prices return the tax portion inside the price.
- **Shipping:** zones (country list; empty = rest of world) +
  methods (`flat_rate` cost, `free_shipping` threshold); checkout
  takes an optional method id, else the cheapest applicable method
  in the matching zone wins (free shipping at threshold). No zones /
  methods configured → cost 0, never blocks the sale.
- **Checkout:** `start_woo_checkout` — strict totals → order →
  customer upsert + order count → P3 provider resolution + gateway
  (same 503 `payment_not_configured` with no provider; never a fake
  success). An order is stamped `source: 'woo'` only when Woo
  totals actually apply (coupon / discount / tax / shipping);
  a plain storefront checkout stays a `wordless` order.
- **CSV import** (hand-rolled parser: quotes, `""` escapes,
  embedded newlines, CRLF/LF/CR, BOM): products (two passes —
  parents then variations; SKU identity, update vs import; sale
  price applied as current price for simple products; private →
  draft; images recorded as not fetched), orders (idempotent
  re-import → skipped; status mapping completed→fulfilled,
  processing→paid, on-hold/pending→pending,
  cancelled/refunded/failed→cancelled; `source='woo-import'`,
  `stock_applied=1` — stock never re-decremented), customers
  (upsert by email). NOT imported, recorded per row: downloadable
  files, order notes, refunds as separate records, payment-gateway
  tokens, Woo tax/shipping settings, customer passwords (empty +
  forced reset, same phpass rule as WXR users), remote product
  images.
- **Routes (`routes_p8.rs`, 47 routes):** products 6 (incl.
  `sku/:sku`, registered before `:id`), attributes 4, variations 5,
  cart 4, quote + checkout + coupon-validate 3, coupons 5, taxes 6,
  shipping 9, customers 2, import 1, variation-by-SKU 1,
  coupon-by-code 1. P3 routes pass `type`/`sku` through; the
  storefront checkout runs the Woo totals with the P3-shaped
  outcome, so the mock-303 / provider-redirect logic holds.
- **Storefront:** product page shows "From {min price}" + the
  variation selector for variable products; cart page shows
  variation labels and gains coupon / country / shipping-method
  fields (parity known-diff `shop-cart-woo-fields`: 36 → 40 lines,
  layout shift only).
- **Admin UI — Commerce (`Commerce.tsx`, 8 tabs):** products
  (Type select + SKU, "From …" price, variations editor with
  attributes + variations panels), orders (Woo breakdown: subtotal,
  coupon discount, tax, shipping + method, source/sourceId;
  variation labels on items), coupons, tax & shipping (settings +
  rates + zones with methods), customers (orders count + total
  spent), import (kind select + CSV textarea + per-row report),
  payments, marketplace. Admin bundle JS 448.31 kB (was 404.20 kB
  at P4), tsc clean.

### Tests & size (measured 2026-11-20, this VM)

- `cargo test --workspace --lib`: **574 passed, 0 failed** (split
  baseline 549 + 23 new Woo tests + 2 commerce variation tests).
- Smoke `scripts/smoke-p8.sh` (port 8806): **121/121 PASS** —
  variable product CRUD + variation pricing (parent price syncs to
  the min variation), coupon apply/reject (incl. case-insensitive
  validate), tax + shipping quote **5507** (3297−500=2797 taxable,
  tax 210, shipping 2500), no-provider 503, mock checkout 201,
  webhook paid exactly once / replay duplicate / bad sig 401 /
  re-pay 400, coupon usage 1 after one paid order, variation stock
  5→4 and tee stock 10→8, CSV import (products 4 imported,
  re-import 4 updated; orders 2 imported as fulfilled, stock NOT
  re-decremented, re-import 2 skipped; customers 1 imported),
  storefront pages + form checkout (form order total **3359**,
  paid, SAVE500 usage 2).
- Parity harness extended with 42 v2-only Woo steps: **385 steps,
  263 match, 18 known-diff, 1 computed, 103 v2-only, 0
  unexplained, exit 0** (latency local workerd v1 p50 50.4/p95
  208.4, v2 p50 69.6/p95 231.6 — same band). One new known-diff:
  `shop-cart-woo-fields`.
- **Bugs found by verification and fixed in P8:** (1)
  `/api/woo/variations/sku/:sku` returned the `(product_id,
  variation_id)` tuple instead of the Variation object; (2) admin
  `WooCustomer` used `ordersCount` while the API returns
  `totalOrders`; (3) checkout resolved the payment provider BEFORE
  validating the coupon, so a bad coupon with no provider selected
  answered 503 instead of 400 — fixed in both the API and the
  storefront checkout; (4) `start_woo_checkout` stamped every order
  `source: 'woo'`, including plain v1-style storefront orders —
  now `woo` only when Woo totals actually apply.

### ⚠️ What P8 did NOT close — WASM size (the blocker)

- **Main Worker 3,241,322 bytes (103.0% of the 3 MiB free-tier
  limit, 95,594 over)** after wasm-opt -Oz (raw 3,718,652).
  Split baseline was 3,004,187 — P8 added **237,135 bytes
  post-Oz**. Converter unchanged at **603,285 bytes (19.2%)**.
  `npm run build` FAILS at the size gate (exit 1) and plain
  `wrangler dev` cannot start — the smoke and parity runs above
  used the local-only bypass `WORDLESS_NO_WASM_OPT=1`
  (documented in `scripts/smoke-p8.sh`; serves the unoptimized
  build locally, behaviourally identical; never a deploy path).
  **No features were deleted to fit.**
- **P8 exit gate NOT met**: no staging deploy (no credentials by
  design), and the size must be resolved first. Levers for the
  parent to decide: apply the converter-split pattern to another
  crate (Woo import or social out via service binding),
  release-profile tuning, or Workers Paid — ruled out, the user
  has no money.
- **R3 (PBKDF2 CPU)** still open until a deployed measurement.
- No live payment-provider checkout observed (mock only, by
  design); no live email; no subscriptions/memberships (P8 forward
  pointer, §9).

## What P9 built — School portal + CBT (2026-11-20)

**Q13 + the P9 decisions were recorded first (doc §3 P9, before any code):**
certificates are a **server-rendered HTML page + print stylesheet, NOT a
PDF** (no good in-Worker PDF story; pure-Rust PDF generation would add
binary size on a Worker already near the free-tier limit, and is unproven);
exactly four question types (`mcq`, `true_false`, `multi_select`,
`short_answer`); short answers are **never** auto-graded; server-side
timing is load-bearing (`deadline_ms` computed once at start and stored,
browser clock never trusted); `max_attempts` on the exam (0 = unlimited),
second start refused with 409 `attempt_limit`, re-start while
`in_progress` resumes the same attempt; question order is a deterministic
seeded shuffle (seed = attempt id), stored on the attempt; answer keys are
never sent to the student; enrolment is open for published courses (no
payment in P9); certificates auto-issue only at or above the pass mark, on
grading completion, one per passing attempt, code `WL-` + 16 hex chars from
SHA-256 of (attempt, user, exam, issued-at-ms).

### The crate: `wordless-school` (~2,900 lines, 7 modules)

- **Migration `0015_p9.sql`** — 11 new tables: `wl_courses`,
  `wl_lessons`, `wl_enrolments`, `wl_lesson_progress` (one row per
  completed lesson), `wl_question_banks`, `wl_questions`, `wl_exams`,
  `wl_exam_attempts`, `wl_exam_answers`, `wl_results`,
  `wl_certificates`. Times are ISO-8601 UTC text; attempt timing is
  epoch ms; scores are REAL rounded to 2 dp (marks are integers;
  multi-select partial credit can be fractional).
- **Grading (pure functions, one test per rule):** mcq / true-false =
  full marks or 0. Multi-select partial credit = `(correct selected −
  incorrect selected) / total correct`, clamped at ≥ 0, × marks, rounded
  to 2 dp — no negative marking. A malformed submitted answer scores 0
  (wrong answer, not a crash); a malformed stored answer KEY returns
  None (corrupt data). Unanswered short answers score 0 automatically
  (nothing for a human to grade). An expired attempt's `submitted_at_ms`
  is its deadline.
- **Courses:** draft courses are invisible to non-instructors/non-admins
  (404); the creator becomes the instructor and manages their own
  courses, banks, questions, exams and grading queue;
  `manage_options` (administrator) manages any course. Lesson content
  is shown only to the enrolled / instructor / admin; titles are public.
  Progress = completed lessons / total lessons; a course completes
  automatically when every lesson is complete. Delete is refused with
  409 `course_in_use` when enrolments or exams exist.
- **Questions:** validation per type — mcq needs ≥ 2 options with an
  index key; true/false forces options `["True","False"]` with a
  boolean key; multi-select needs a non-empty distinct index list;
  short answer has no options/key, reference in `model_answer`. Edit
  is refused with 409 `exam_in_progress` while an attempt is in
  progress; delete is refused with 409 `question_in_use` when answers
  exist.
- **Exams:** explicit `questionIds` on the exam; empty = all questions
  in the bank. Exam access: enrolled students only, only inside the
  exam window (outside → 409 `exam_not_open` / `exam_window_closed`).
  Attempt states: `in_progress` → `submitted` (objective-only) /
  `awaiting_grading` (short answers pending) / `expired` (closed by
  deadline, objective-only) → `graded` (manual grading finished). A
  result row exists ONLY once grading is complete; while
  `awaiting_grading` the result is 404 "still being graded" — a partial
  score is never presented as a final result. Expired attempts are
  closed by the cron sweep (every 5 min) AND lazily on attempt read —
  the lazy half is the locally verifiable one. Leaderboard: score
  descending, then submitted-at ascending (earlier submission wins
  ties).
- **Routes (`routes_school.rs` in the school Worker — moved verbatim
  from `routes_p9.rs` in the 2026-11-20 school split, 42 routes):** courses 8, lessons 4,
  enrolment + progress 3, banks 4, questions 5, exams 5, attempts 4,
  grading queue + grade 2, results 2, leaderboard 1, certificates 2,
  public pages 3 (`/school`, `/school/courses/:slug`,
  `/school/certificates/:code`). Guards: management `edit_posts`,
  student actions `require_auth`, public reads open;
  instructor-vs-admin checks live in the service
  (`require_manage_course`).
- **Anti-cheating — what IS provided:** server-side timing,
  randomised question order, attempt limits, exam-window enforcement,
  enrolment-only access, session-bound attempts (the attempt belongs
  to the logged-in user; `cf-connecting-ip` recorded when present).
  What is NOT: live proctoring, webcam monitoring, lockdown browser,
  IP-based identity proof.
- **Admin UI — School (`School.tsx`, 4 tabs):** Courses (create /
  publish / enrol, lesson add / complete, progress), My exams (exam
  add / publish, exam-taking UI with countdown display, per-question
  review), Grading queue (marks + feedback, model answer shown),
  Certificates (issued codes, verification links). Honest-limits note
  shown on the page. Admin bundle JS 468.38 kB (was 448.31 kB at P8),
  `tsc` clean.

### Tests & size (measured 2026-11-20, this VM)

- `cargo test --workspace --lib`: **596 passed, 0 failed** (574 split
  baseline + 22 new school tests: 10 grading unit tests + 12 service
  tests incl. the full journey enrol → study → sit → result →
  certificate; second-attempt block; non-enrolled denial; window
  open/closed; late submit 409 + auto-submit with 5/11 = 45.45%; sweep
  closes expired attempts with `submitted_at` = deadline, idempotent;
  short-answer manual grading queue → graded → result 62.5% +
  certificate; multi-select partial credit 2.0/4 in a real exam;
  question validation; draft invisibility; edit-refused-while-in-
  progress; course-delete-refused-when-in-use).
- Smoke `scripts/smoke-p9.sh` (port 8810, clean room, main Worker
  only): **107/107 PASS** — course create + enrol (duplicate 409),
  lessons + progress (1/2, then completed), question bank CRUD (4
  types; bad mcq 400; true/false options forced), exam start → answer
  → submit with server-side deadline enforcement (deadline forced past
  in D1 → submit 409 `attempt_expired`, attempt auto-submitted as
  `expired` with saved answers scored 5/5 = 100%; late answer save
  409 too), auto-grade scoring 9/11 = 81.82% (mcq 5 + tf 2 +
  multi-select partial 2), manual-grading queue state (student 403,
  queue lists the item, grade full marks → graded, result 100% +
  certificate, grade twice 409), results, certificate issue (API +
  public page with `@media print`), access denials (non-enrolled
  student 403, outside the exam window 409, second attempt 409),
  empty exam start 400, course delete refused 409. First run 106/107:
  the one failure was a test-ordering bug (the non-enrolled check ran
  while the exam was still a draft, so the draft 404 masked the
  enrolment 403) — fixed in the script, re-run 107/107.
- Parity harness extended with 34 v2-only school steps: **419 steps,
  264 match, 18 known-diff, 0 unexplained, 137 v2-only, exit 0**
  (latency local workerd v1 p50 47.5/p95 188.4, v2 p50 84/p95 233.5;
  school group v2 p50 84.7/p95 227.9 — same band). No new known-diffs.
- **Bugs found by verification and fixed in P9:** (1) admin build
  failed — `School.tsx` imported a `Textarea` the shared elements
  library does not export (raw `<textarea>` like the Editor) and
  passed an `options` prop to `Select` (it takes `<option>` children);
  (2) three service-test failures during the run (malformed-answer
  grading rule, an over-broad "answer key leaked" assertion matching
  the legitimate `answers` field, a wrong expectation that the grading
  queue errors for students — it returns an empty list; the route
  guard is what 403s) — fixed in the tests/service, no production-code
  change.

### ✅ What P9 did NOT close — WASM size — RESOLVED the same day by the school split

- **P9 as built: main Worker 3,111,496 bytes (98.9% of the 3 MiB free-tier limit,
  34,232 bytes of headroom)** after wasm-opt -Oz — **111,496 bytes
  over the 3,000,000 soft target**. P9 added **190,853 bytes post-Oz**
  on a baseline of 2,920,643. Converter 603,285, social 618,060, woo
  883,345 (unchanged). `npm run build:all` and `npm run size` exit 0:
  the deploy size gate is the HARD limit (3,145,728), which still
  passed, so the build was deployable on the free tier — but only
  34,232 bytes of headroom remained, and P10 would almost certainly
  have breached it. **No features were deleted to fit.**
- **Resolution (2026-11-20, same day):** the split pattern was applied
  to `wordless-school` itself — new `wordless-school-worker` (42
  routes, own `wrangler.toml`, `SCHOOL` service binding, port :8809
  locally). Main Worker now **2,923,351 bytes (92.9%) — 76,649 under
  the soft target**, 222,377 headroom; school Worker 642,604. Full
  record: doc §3 "School split". P10 is unblocked on size.
- **P9 exit gate NOT met**: no staging deploy (no credentials by
  design). **R3 (PBKDF2 CPU)** still open until a deployed measurement.
- No live proctoring, webcam, lockdown browser or IP identity proof;
  short-answer grading is manual only; no payment for enrolment in P9.

## What P10 built — SEO "second to none" (2026-11-20)

**Scope (doc §0.1 F / §3 P10):** the M7 engine extended — full JSON-LD,
image/video sitemaps, `llms.txt` + `llms-full.txt`, Core Web Vitals budgets,
an audit history that records real measurements and never fabricates a score.
SEO code is split since 2026-11-20 (see §3 "SEO split" in the architecture
doc): the SEO surface (31 routes — sitemaps, robots, llms.txt, CWV/audit/FAQ
APIs, preview API, redirects) runs in `wordless-seo-worker` behind the `SEO`
service binding (local port :8812); the render primitives the main Worker
still needs (JSON-LD/head/meta builders, 404-log write) live in the shared
`wordless-seo-core` crate, linked by main + school + SEO Workers. The Course
JSON-LD node is rendered by the school Worker on course pages, via core.

### JSON-LD (`wordless-seo/src/jsonld.rs`)

- **Types shipped:** WebSite + SearchAction (`{site_url}/?s={search_term_string}`),
  Organization (home page only; logo only when configured), Article,
  BreadcrumbList, FAQPage, Product (offers with price from minor units,
  InStock/OutOfStock), Course (provider Organization, optional instructor
  Person). `PostSeo`'s API shape is unchanged — FAQ lives beside it.
- **Validator:** `validate_json_ld` parses the served JSON and checks
  `@context`, known `@type`, and required fields per type (Article headline;
  BreadcrumbList ≥1 item with position/name/item; FAQPage ≥1 Question with
  `acceptedAnswer.text`; Organization name+url; Product name+offers.price+
  priceCurrency; Course name+description+provider.name; WebSite name+url and
  SearchAction `urlTemplate` containing `{search_term_string}`). It is a
  **structural** validator — NOT Google's Rich Results Test, and no rich
  result is claimed. JSON-LD escaping (`<`→`\u003c` etc.) is retained so a
  hostile title cannot break out of `<script>`.
- **Preview API:** `GET /api/posts/:id/seo/json-ld` (`edit_posts`) returns the
  served graph + `valid` + `problems`.

### Visible breadcrumbs (`wordless-render/src/pages.rs` `finish_page`)

- `<nav aria-label="Breadcrumb"><ol>`, Home link first, last crumb plain text
  with `aria-current="page"`, empty when there is no trail. Rendered on
  **both** paths: fallback layout (top of `<main>`) and block-theme path (top
  of `<body>`, before the template output — the theme's own navigation is
  untouched). The first implementation only covered the fallback path, so
  post/product pages showed no breadcrumbs until verification caught it.

### Sitemaps, robots, llms.txt (`wordless-seo/src/sitemap.rs`, `robots.rs`, `llms.rs`)

- `/sitemap.xml` behaviour is **unchanged** (v1 parity). New:
  `/sitemap-index.xml` (paginated main chunks + images + videos),
  `/sitemap-images.xml` (only URLs with images), `/sitemap-videos.xml`
  (sources: `wp:video` blocks, `<video>`/`<source>` tags, YouTube/Vimeo
  embeds — a video is listed **only** when its post has a featured-image
  thumbnail, because the protocol requires `thumbnail_loc` and none is
  invented; `<loc>` is the page URL, `<video:content_loc>` the video URL).
- `robots.txt` default now lists all four sitemap files; a stored file with
  any `Sitemap:` line is served exactly as written.
- `/llms.txt` (100 links per section: Pages/Posts/Products/Courses) and
  `/llms-full.txt` (newest 50 posts, 20,000 chars each, markup stripped).
  Products/courses are queried directly from `wl_products`/`wl_courses`
  with error tolerance (missing tables → empty section, never 500).

### CWV budgets + audit runs (`wordless-seo/src/cwv.rs`, `audit.rs`, migration `0016_p10.sql`)

- Budgets stored as `wl_cwv_budgets` (defaults **LCP 2500 ms, INP 200 ms,
  CLS 0.1** — web.dev "good" thresholds; ranges LCP 500–10,000, INP
  50–1,000, CLS 0.01–1.0). `evaluate_cwv` passes only when every measured
  metric passes and at least one is measured — an unmeasured metric is never
  a pass. **No RUM beacon is shipped** — the Worker never measures a
  visitor's vitals itself.
- `wl_seo_audit_runs` (url, tool default `'lighthouse'`, four nullable score
  columns, lcp_ms, inp_ms, cls_milli, notes, created_at) — append-only, NULL
  means "not measured", never 0. `POST /api/seo/audit-runs` returns 201 and
  refuses an empty run (400) or a score outside 0–100.
- Admin SEO page gained an **Audit** tab: CWV budgets editor, "Record a
  Lighthouse run" form (blank = not measured), runs table with per-vital
  ✓/✗ vs budgets, sitemap viewer, structured-data preview card.

### FAQ (`wordless-seo/src/meta.rs`)

- `GET/PUT /api/posts/:id/seo/faq` (`edit_posts`; PUT accepts a bare array or
  `{faq:[...]}`, replaces the whole list, empty clears). Max 20 items,
  question 1–200 chars, answer 1–2000 chars; corrupt stored JSON reads as
  empty, never 500s.

### Verification (2026-11-20, all measured)

- `cargo test --workspace --lib` **617/0** (596 baseline + 21 new P10 tests;
  `wordless-seo` 48/48). Admin `tsc` clean.
- `scripts/smoke-p10.sh` (port 8811) **89/89**: JSON-LD validity per page
  type (home SearchAction + Organization, post Article + FAQPage +
  BreadcrumbList, product Product + price, course Course + provider),
  visible breadcrumbs, sitemap index + all child sitemaps, robots.txt (all
  four lines), llms.txt / llms-full.txt, CWV budgets round-trip + validation,
  audit-run CRUD + validation, sitemap viewer, 404/401 denial behaviour.
  Two failures found and fixed during the run (breadcrumbs on the block-theme
  path; the smoke's `siteurl` seed, which exposed the JSON-encoded-options
  finding above) — re-run 89/89.
- Parity: corpus extended with 18 v2-only P10 steps and two new known-diffs
  (`p10-seo-head`, `robots-default-sitemaps`; 16 steps tagged in total) —
  **437 steps, 248 match, 34 known-diff, 155 v2-only, 0 unexplained, exit 0**
  (v1 p50 47.7/p95 175.8, v2 p50 91.1/p95 257.9). Zero regressions on the
  419 pre-P10 steps.

### ⚠️ What P10 did NOT close — WASM size (thin, not over) + Lighthouse (unmeasured)

- **Main Worker 2,994,900 bytes (95.2%) — only 5,100 bytes under the
  3,000,000 soft target**, 150,828 headroom to the hard limit. P10 added
  71,549 bytes post-Oz; school Worker 649,081 (+6,477, the new `wordless-seo`
  dependency). `npm run build:all` and `npm run size` exit 0. **No features
  were deleted to fit.** **RESOLVED the same day (2026-11-20) by the SEO
  split:** main Worker now **2,854,587 bytes (90.7%), 145,413 under the soft
  target**, 291,141 headroom; SEO Worker 800,785. P11 is unblocked on size.
- **Lighthouse NOT measured locally.** There is no browser/Chromium in this
  VM and the parity harness has no Lighthouse support, so the doc's targets
  (SEO 100, A11y ≥ 95, Perf ≥ 90, Best Practices ≥ 90; LCP < 2.5 s, INP <
  200 ms, CLS < 0.1) are **not claimed** for any page. The audit machinery
  above exists precisely so a real staging run can be recorded per URL.
  **P10 exit gate NOT met** (needs staging deploy + Lighthouse runs + the
  user seeing the scores; no credentials by design). R3 stays open.
- Honest limits: validator is structural, not Google's Rich Results Test;
  variable products report the parent's minimum price and are reported in
  stock; video sitemap only with a featured-image thumbnail; llms caps
  (100 links/section, 50 posts / 20,000 chars); no RUM beacon; P4 visual
  items (§6.4) still stand.

## What P11 built — Visual Builder: drag-and-drop website design (2026-11-20)

**Scope (doc §3 P11):** a real drag-and-drop editor in the React admin —
sections, columns, widgets, live preview, responsive controls — saving to
the same Elementor-compatible data the P3 renderer already displays. The
palette IS the library: it embeds the complete shared element library and
the full icon library, not a subset.

### Palette (`wordless-builder` + `GET /api/builder/palette`)

- **Contents (measured by `scripts/smoke-p11.sh`):** 10 core widgets (the
  P3 Elementor set), 26 elements (one per `admin/src/components/
  elements.tsx` export), 104 icons (the full `IconName` union), 3 devices
  (desktop 1440 / tablet 768 / phone 360).
- **Same source, not a copy:** the Rust palette constants are GENERATED
  from the TS sources by `scripts/gen-builder-palette.mjs` (run it after
  adding an element or icon), and `scripts/check-builder-palette.mjs`
  proves they still match — element exports, icon names, every palette
  icon known, every palette widget type rendered by
  `wordless-elementor`. Run it in CI or before a release; it exits 1 on
  any drift.
- **The 19 `wordless-*` widget renderers** live in `wordless-elementor`
  (one per shared element; `Button` maps to the existing `button`
  widget), so every palette element renders through the converter
  Worker. All values escaped; tones clamped to allow-lists.
- **Extension gating (P6 rule):** an extension widget appears only when
  its extension is installed (`wl_extensions`) AND its instance is
  `loaded` (`wl_extension_instances` — the P6 kill switch). Disabled
  instances lose their palette entries within one request.
  Extension widgets do NOT render on the public site in P11
  (`rendersPublicly: false` in the palette and the preview).

### Layout storage + API (`crates/wordless-worker/src/routes_p11.rs`, migration `0017_p11.sql`)

- A layout IS Elementor JSON (sections → columns → widgets) stored in
  the `_elementor_data` postmeta key — the P3 renderer displays it, so
  there is no second format to drift. `GET /api/posts/:id/meta` reads a
  post's meta map (the P4 `PUT` writes it).
- `POST /api/builder/preview` validates the layout (depth ≤ 10, max 500
  nodes, unique ids; structural problems are 400 with the whole list)
  and renders it through the converter Worker's `render_elementor`,
  returning `{ html, unsupported, device, deviceWidth }`. An unknown
  widget type is NOT an error: it is listed in `unsupported`, kept in
  the layout, and not rendered — preview and public page agree.
- `POST /api/builder/widgets` registers one (`manage_options`):
  extension installed (404 otherwise), lowercase-slug type not colliding
  with a built-in (400), label non-empty (400), icon from the library
  (400), settings schema a JSON object/array (400). `GET
  /api/builder/widgets` lists all (with extension status);
  `DELETE /api/builder/widgets/:slug/:type` removes one (404 when
  absent). New table `wl_builder_widgets` (migration 0017).

### Admin builder (`admin/src/pages/Builder.tsx`, `#/builder`)

- Page picker (pages with/without a layout, new-page create), canvas
  (sections with 1–4 columns, HTML5 drag-and-drop between columns,
  click-to-add for touch, ↑/↓ reorder, delete), schema-driven settings
  panel (text/textarea/url/number/checkbox/select/list-text/list-object
  as one-JSON-object-per-line), live preview iframe with the device
  width applied, save (PUT meta) + publish, undo/redo (pure history
  stack in `admin/src/lib/builderHistory.ts`, capped at 100, Node-tested
  3/3). Icon browser: all 104 icons, click copies the name for an icon
  setting. Unsupported widgets show as amber blocks with their raw
  settings; the preview banner lists types that will not render
  publicly. Styles in `admin/src/styles.css` (`.builder-*`); the grid
  collapses to one column below 1100 px.

### Measured (2026-11-20, local workerd)

- `cargo test --workspace --lib` **630/0**; admin `tsc` clean;
  `build-admin.sh` green (68 modules, 498.41 KiB JS);
  `check-builder-palette.mjs` PASS; history Node tests 3/3;
  `scripts/smoke-p11.sh` (port 8814) **54/54**; parity **458 steps,
  247 match + 1 computed, 34 known-diff, 176 v2-only, 0 unexplained,
  exit 0** (21 new builder steps, all v2-only).
- **Size:** main Worker **2,886,899 bytes (91.8%)** — P11 added 32,312
  post-Oz; converter 619,641; no split needed, nothing deleted to fit.

### What P11 did NOT close

- **Exit gate NOT met:** no staging deploy (no credentials by design);
  the user walkthrough (build a page unaided on phone + desktop,
  publish it) has not happened. §6.4 visual review (1440/768/360),
  per-theme visual re-check, keyboard walkthrough and screenshot
  retake were NOT done locally (no browser in this VM) — staging soak.
- Extension widgets do not render publicly yet (listed as unsupported).
  No real WordPress Elementor export file was exercised locally (the
  import path is proven at the data level: parse + render).
- **Harness gap found in P11:** parity does not validate v2's status on
  v2-only steps — its `expect` check only fires when v1 and v2 agree.
  The 21 builder steps were verified by hand from
  `parity-report.json`; the harness should gain a v2-only expect check
  before P12.

## What P12 built — Multisite: many websites under one installation (2026-11-20)

**Scope (doc §3 P12, §7 Q14 DECIDED):** one install running many sites —
shared users and code, per-site content/settings/themes, a site switcher
in the admin, per-site domains. The final build phase: P0–P12 are now all
complete locally; after P12 only the cutover gate remains (staging deploy
from the user's machine, the user's walkthrough, written approval).

### Data model + routing (`wordless-multisite`, migration `0018_p12.sql`)

- **Site-id column** on wp_posts/wp_postmeta/wp_terms/wp_term_taxonomy/
  wp_term_relationships/wp_comments/wp_commentmeta/wp_termmeta/
  wl_redirects/wl_404_log/wl_seo_audit_runs, plus `wl_sites` (seeded with
  site 1, the main site) and `wl_site_options`. Single-site installs
  carry site id 1 by default, so nothing migrates. `wl_404_log` was
  rebuilt as `PRIMARY KEY (site_id, path)` — its old PK was just `path`,
  so a second site logging the same path would have silently vanished.
- **Per-site options (13 keys + 2 prefix families):** blogname,
  blogdescription, template, stylesheet, posts_per_page,
  permalink_structure, timezone_string, default_comment_status,
  comment_max_links, active_theme, siteurl, wl_robots_txt,
  wl_cwv_budgets, plus `wl_login_*` and `wl_theme_variation_*`. A new
  site inherits the network defaults until it sets its own.
- **Resolution order:** admin header (`X-Wordless-Site`, id or slug,
  honoured only for a logged-in user with `edit_posts`) → subdomain
  `<slug>.<network_domain>` (skips `www`, needs `WORDLESS_NETWORK_DOMAIN`)
  → path prefix `/<slug>/…` (never for `/api` or `/admin`, never for a
  reserved word) → site 1. Unknown/deleted header or subdomain sites
  fall through to site 1 rather than failing the request. Archived
  sites resolve but their public pages return 404, while the API can
  still administer them. Custom-hostname domain mapping is OUT (needs
  Cloudflare for SaaS, paid — ruled out by no-money).
- **Threaded site_id through:** posts, postmeta, terms, comments,
  search, menus, revisions, permalinks, scheduling, CPT/taxonomy
  registries; the whole public render layer (posts, terms, redirects,
  search, RSS, shop pages); settings, login branding, themes
  (activation/variations/previews per-site; `delete_theme` refuses when
  active on ANY site); GraphQL; the AI agent context; the SEO and school
  sub-workers (site from the forwarded header, validated against
  `wl_sites`); `/api/options/:key`. `Post.site_id` is internal-only
  (`#[serde(skip)]`), so parity responses are byte-identical.

### Sites API (`crates/wordless-worker/src/routes_p12.rs`, `manage_options`)

- `GET /api/sites`, `POST /api/sites` (slug validated: lowercase
  a-z0-9-, 1–63 chars, 22 reserved first-segments; duplicate 400),
  `GET/PUT/DELETE /api/sites/:id` (delete wipes ALL of the site's
  content rows and returns `deletedPosts`; site 1 is refused),
  `GET /api/sites/current` (the site the caller's request resolved
  to), `GET/PUT/DELETE /api/sites/:id/options/:key` (writes restricted
  to site-scoped keys — anything else is 400; delete removes the
  override so the site falls back to the network default).

### Admin (`admin/src/pages/Sites.tsx`, `#/sites` + header switcher)

- Sites list (id, slug, address, status, created), create form, archive/
  unarchive, delete with a wipe warning, per-site public-title edit
  (`blogname` option — changing one site's title never touches another).
  The header carries a site switcher: switching sets `X-Wordless-Site`
  (persisted in localStorage `wl_site`) and reloads so settings, posts,
  media and every other page re-fetch under the new site.

### Measured (2026-11-20, local workerd)

- `cargo test --workspace --lib` **636/0** (baseline 630 + 6 new
  multisite crate tests); admin `tsc` clean; `build-admin.sh` green;
  `scripts/smoke-p12.sh` (port 8815, clean room) **94/94** — sites API,
  current-site + unknown-header fallback, isolation per query path
  (posts, terms, settings, options, redirects, 404 log), switcher
  behaviour, path routing, subdomain routing, archive, delete,
  admin bundle contains the `#/sites` route; parity **482 steps, 248
  match, 34 known-diff, 0 unexplained, 200 v2-only, exit 0** (24 new
  sites steps, all v2-only).
- **Size:** main Worker **2,935,745 bytes (93.3%)** — P12 added 48,846
  post-Oz; **64,255 bytes under the 3,000,000 soft target**; converter
  619,641, social 618,060, woo 881,877, school 650,443, seo 807,836.
  No split needed, nothing deleted to fit.
- **Parity-harness gap from P11 — FIXED in P12:** the harness now
  validates v2's status on v2-only steps against `step.expect` (and
  `expectBody` against v2 only); a v2-only step without `expect` is
  flagged as a corpus bug. It also gained per-step `headers` support
  (with `{{var}}` resolution), used by the P12 steps for
  `X-Wordless-Site`.

### What P12 did NOT close

- **Exit gate NOT met:** no staging deploy (no credentials by design);
  two live sites on staging under one install, walked through by the
  user, have not happened. **R3 (PBKDF2 CPU)** still open until a
  deployed measurement.
- **Honest limits (network-wide in P12, by design):** WooCommerce,
  social and school tables are NOT site-scoped; `wl_ai_memory` is
  shared (inline UNIQUE on `key` — scoping it needs a table rebuild);
  `wl_media_derivatives` has no site_id column (gated by the
  site-scoped media-record lookup); extension host-call option
  reads/writes stay network-wide; the analytics snippet option stays
  network-wide; theme rows are network-wide (activation/variations/
  previews are per-site). Cross-site content sharing, per-site
  billing and domain mapping beyond basic routing remain OUT.

## Tests & smoke

```sh
# Native unit tests (596 tests: ai 34, auth 25, backup 8, cms 48,
# commerce 56, compat 11, conversion 43, conversion-jobs 8,
# converter 7, db 1, elementor 13, extensions 28, graphql 12,
# importer 17, integrations 42, kernel 17, media 28, render 21,
# school 22, security 3, seo 27, social 24, themes 63, webhooks 5,
# worker 10, woocommerce 23)
TMPDIR=$HOME/.tmp-cargo cargo test --workspace --lib

# WASM tests — both engines, incl. the PBKDF2 vector + timings (see below).
# NOTE: wasm-pack test swallows console output. To see the timing lines,
# run the test wasm directly through the test runner:
TMPDIR=$HOME/.tmp-cargo wasm-pack test --node --release -p wordless-kernel
~/.cargo-wb01/bin/wasm-bindgen-test-runner --nocapture \
  target/wasm32-unknown-unknown/release/build/wordless-kernel-*/out/wordless_kernel-*.wasm

# Release builds (WASM + shim.mjs) — both Workers.
# ⚠️ P8 (2026-11-20): the main Worker is OVER the free-tier limit
# (3,241,322 bytes, 95,594 over), so `npm run build` FAILS at the
# size gate until the size is resolved (see "What P8 built"). The
# converter is fine. Local verification uses WORDLESS_NO_WASM_OPT=1.
bash scripts/build-worker.sh      # main: 3,241,322 bytes (103.0%) — OVER
bash scripts/build-converter.sh   # converter: 603,285 bytes (19.2%)
npm run size                      # reports both Workers, exit 1 while over

# Full smoke: migrations + seed admin + wrangler dev + 273 checks
./scripts/smoke-p3.sh            # 273 passed, 0 failed (2026-11-19, clean room, re-run after the P4 follow-ups)
./scripts/smoke-p3.sh --keep     # leaves wrangler dev on :8798
# (smoke-p2.sh still exists: the P2 surface, 129 checks, port 8797.)

# P4 API smoke: CPT/taxonomy CRUD, profiles + theme null/persist,
# forced-reset flow, scheduling (past publishes / future stays hidden),
# plugins list + activate/deactivate, login branding, featured image +
# post meta, per-plugin settings — 150 checks, port 8799
./scripts/smoke-p4-api.sh        # 150 passed, 0 failed (2026-11-19, clean room)
./scripts/smoke-p4-api.sh --keep # leaves wrangler dev on :8799

# P5 parity harness: same seeded D1, both workers, 320 steps diffed
./tests/parity/run-parity.sh     # 265 match, 17 known-diff, 38 v2-only, 0 unexplained, exit 0 (run 5, 2026-11-19; re-run after P6: identical, exit 0)
# P8 parity: 385 steps (42 v2-only Woo steps), 263 match, 18 known-diff,
# 1 computed, 103 v2-only, 0 unexplained, exit 0 (2026-11-20)
# P9 parity: 419 steps (34 v2-only school steps), 264 match, 18 known-diff,
# 0 unexplained, 137 v2-only, exit 0 (2026-11-20; latency local workerd
# v1 p50 47.5/p95 188.4, v2 p50 84/p95 233.5).
# P5 rollback rehearsal: one D1 file, v1 -> v2 -> v1, session + posts survive both ways
node tests/parity/rollback.mjs   # 15/15 checks passed (2026-11-19)

# P6 extension-runtime smoke: hello-dolly end-to-end, host endpoint,
# disable kill switch, hostile plugin, TT25 theme, runaway timeout —
# 62 checks, ports 8800-8803
./scripts/smoke-p6.sh            # 62 passed, 0 failed (2026-11-19, clean room, run 6)
# P8 WooCommerce smoke: variable products, coupons, tax + shipping,
# mock checkout + webhook, CSV import, storefront — 121 checks, port 8806
WORDLESS_NO_WASM_OPT=1 ./scripts/smoke-p8.sh  # 121 passed, 0 failed (2026-11-20, clean room)
# (the bypass is needed while P8's WASM is over the free-tier limit;
# see "What P8 built" — never a deploy path.)
# P9 school + CBT smoke: courses, enrolment, question bank, timed exam
# with deadline enforcement, auto-grading, manual grading, results,
# certificates, denials — 107 checks, port 8810, main Worker only
./scripts/smoke-p9.sh            # 107 passed, 0 failed (2026-11-20, clean room)
# P10 SEO depth smoke: JSON-LD per page type, visible breadcrumbs, sitemap
# index + image/video sitemaps, robots.txt, llms.txt / llms-full.txt, CWV
# budgets, audit runs, sitemap viewer, 404/401 denials — 89 checks, port
# 8811 (starts the school Worker on :8809 for the course page)
./scripts/smoke-p10.sh           # 89 passed, 0 failed (2026-11-20, clean room)
# Publish an extension Worker from the user's machine (needs wrangler login):
node scripts/publish-extensions.mjs hello-dolly --env staging --confirm  # NOT verified live yet
```

Smoke seeds one admin the v1 way (login `admin`, password
`smoke-admin-2026`, PBKDF2 hash generated by Node `crypto.pbkdf2Sync`,
local D1 only). `smoke-p3.sh` is `smoke-p2.sh` plus the P3 surface:
conversion upload (real `hello.php` → done, 3 functions) + fixture
attempt (502 from this VM — upstream unreachable; documented in the
script, not a failure), AI task → approval → exactly one draft,
integrations, marketplace install + compat report (verdict
`compatible`, 1 PHP file, 1 converted, conversion record present),
commerce product/cart/mock checkout/webhook (paid once, replay
duplicate, bad signature 401), security overview, WXR + SQL import
(v1 counts), GraphQL (same post as REST, guards, `operationName`),
outbound webhooks (subscribe, fire on post create, retry, delete),
public shop flow (add → cart → mock checkout → test payment →
success), CPT archive (`/books`, seeded by SQL — no CPT routes exist
yet), redirect-update purge, backup create/verify/download/restore.
The mock payment secret is injected via
`--var MOCK_PAYMENT_WEBHOOK_SECRET:smoke-mock-secret` (local dev
only). Lockout stays LAST — 5 failed logins lock the name AND the IP,
so nothing after it can log in. Not covered live (unit-tested in their
crates): TOTP setup/confirm, password-reset email, live Workers AI,
live payment providers. The script is not idempotent against dirty
local state: use a clean room (`rm -rf .wrangler`) when in doubt.
Never `pkill -f` with a pattern that appears in your own command
line — it SIGTERMs your own shell; use the `wrangle[r]` bracket trick
or kill by PID (or `fuser -k <port>/tcp`).

## Measurements (all run 2026-11-19, this VM)

**WASM size (release, `--no-opt`, clean build): `index_bg.wasm`
3,328,519 bytes (~3.17 MiB)** at P5 — **105.8% of the 3 MB free-tier
script limit, 182,791 bytes over** (P4: 3,318,127 bytes, 105.5%; P3:
3,179,092 bytes, 101.1%; P2: 2,662,192 bytes, 85%; P1: 816,324 bytes,
26%). Bundled JS entry `index.js` 32.2 KiB. Size ladder, all measured
2026-11-19 on this VM: first P3 wiring build 5,017,385 →
`strip = true` 4,319,346 → `opt-level = "z"` **3,179,092** →
`panic = "abort"` tried, 3,182,736 (worse — reverted). A wasm-opt
`-Oz` pass (not possible in this VM — binaryen cannot be downloaded)
is the remaining lever and is expected to close the gap; until it is
measured, the binary is OVER the free-tier limit. `scripts/deploy.mjs`
enforces the gate (refuses a free-tier deploy; `--paid` allows 10 MiB).

**Update 2026-11-20 (converter split):** binaryen 132 installs in
this VM (`npm install binaryen`), so `-Oz` runs in both build scripts,
and the converter was split out (see "Converter split" above): main
**3,004,187 bytes (95.5%)**, converter **603,285 bytes (19.2%)** —
both under the limit, `npm run size` exit 0. The pre-split history
above is kept as the record of how the limit was reached.

**PBKDF2, 100,000 iterations, HMAC-SHA256** (vector: password
`correct-horse-battery-staple`, salt bytes 0..15 — generated with Node
`crypto.pbkdf2Sync`, same format as v1; both engines verified
byte-identical to the vector):

| Engine | Where | Time |
| --- | --- | --- |
| WebCrypto (SubtleCrypto) | WASM in Node 24, release | **31.0 ms** |
| pure-Rust (`pbkdf2` crate) | WASM in Node 24, release | **78.0 ms** mean (runs 92 / 68 / 74 ms) |
| pure-Rust | native x86_64, release | **23.4 ms** mean (runs 26.1 / 22.1 / 22.0 ms) |
| pure-Rust | native x86_64, debug | 2,910 ms (meaningless — debug mode; do not quote) |

**R3 verdict (10 ms free-tier CPU limit):** the pure-Rust fallback at
78 ms in WASM would blow the limit if it ran synchronously on the
request path — the doc's concern is real for that engine, which is why
the login path uses WebCrypto first and the pure-Rust engine is fallback
only. WebCrypto is a native async op (31 ms wall in Node). **Caveat:**
these are wall times in Node/local workerd, not Cloudflare CPU time;
workerd's CPU accounting for async WebCrypto differs from wall time,
and local dev cannot measure it. The definitive free-tier CPU check
needs a deployed measurement (P5). Live login wall time under local
`wrangler dev` (warmed up): good login ~86 ms, bad login ~57 ms,
`/api/health` floor ~29 ms — consistent with WebCrypto ~30 ms + D1 +
workerd overhead.

## Bugs found & fixed during P1

1. **HTTP status always 200.** `Response::from_bytes` defaults to 200
   and `to_worker_response` never applied `resp.status`. Every error
   envelope (401/404/405) came back as 200 with a correct body.
   Fixed with `.with_status(resp.status)`.
2. **`/admin/` 500 — immutable headers.** Responses from the ASSETS
   fetcher cannot be mutated (`TypeError: Can't modify immutable
   headers`). `with_headers` now rebuilds the response into a fresh
   mutable one (status + body + copied headers preserved).
3. **`/admin/` 404 — asset layout.** v1 builds the SPA to
   `admin/dist/admin/` (Vite base `/admin/`), so the fetcher maps
   `/admin/index.html` → `dist/admin/index.html`. The P1 placeholder
   was at `dist/index.html`; moved to `dist/admin/index.html` to match.
4. **Smoke script:** the `node -e "..."` seed block had broken shell
   quoting (rewritten as a heredoc), and the logout curl listed the URL
   twice (it logged out twice — masking the session-revocation check).
5. **Kernel time tests:** a wrong epoch constant in the test (my
   constant, not the code — `date -u` proved the code right) and an
   off-by-one fractional-seconds index (`b[19]`, not `b[20]`).
6. **web-sys signatures:** `import_key_with_str` takes the *algorithm*
   as the string and the key data as a `Uint8Array` (Object) — read the
   generated bindings, don't guess.
7. **`worker` feature flags:** D1 is behind `features = ["d1"]`;
   R2 is `env.bucket()`, returning `worker::Bucket` (not `ObjectBucket`,
   not `env.r2()`).

## Open blockers / notes for P4

- **WASM size: 3,318,127 bytes — 5.5% OVER the 3 MiB free-tier limit
  (P3: 3,179,092; P4 end state: 3,291,933).** Next step is wasm-opt `-Oz` on a
  machine that can download binaryen; if that does not close the
  172,399-byte gap, split the conversion engine out of the Worker or
  use Workers Paid ($5/mo, 10 MB limit). P4 made this worse, not
  better — see "What P4 (API) built".
- **wasm-opt is off in this VM** (binaryen download blocked).
- **PBKDF2 R3:** the free-tier CPU question is not closed until a
  deployed measurement exists (P5). If WebCrypto counts fully toward
  the 10 ms budget, the login path needs rethinking (e.g. fewer
  iterations is NOT an option — it must stay byte-compatible with v1;
  the lever would be the paid tier or a different verify strategy).
- **Session `last_seen_at` write throttle** (v1 updates at most once
  per 5 min) is implemented in `wordless-auth`; the `wl_sessions`
  schema comes from migration 0009.
- **CPT/taxonomy REST routes — DONE in P4** (see "What P4 (API)
  built"). The P3 smoke's CPT archive check still seeds by SQL; the
  P4 smoke exercises the routes live.
- **Cron sweep firing is NOT verified live** — local `wrangler dev`
  fires no real cron; the lazy publish check is the live half and the
  sweep is the same unit-tested code. Verify the trigger fires on
  staging (P5); locally it can be poked via
  `http://localhost:<port>/cdn-cgi/local/scheduled`.
- **Parity harness (§5.1) — DONE in P5** (see "What P5 built"): 320
  steps, 0 unexplained diffs (run 5); rollback rehearsal 15/15; the
  cutover runbook is `docs/cutover-runbook.md`. The P5 exit gate is
  still open — no staging deploy happened here (no credentials by
  design); it needs the size decision, the user's written approval,
  and the user-run deploy + soak.
- **P4 scope starts with:** the §0.1 A fixes (editor categories/tags/
  featured image/excerpt/scheduling, dedicated Plugins page, fully
  editable user profiles) FIRST, then the §6 redesign, then dashboard
  themes + tweakable login + element/icon libraries — per doc §3.
  The API half of those fixes is done, and the admin-track half is
  done too (see "What P4 (entry criteria UI) built") — including the
  featured-image leg (`featuredImageId` + `PUT /api/posts/:id/meta`)
  and per-plugin settings, both added in the P4 follow-ups.
- **Default theme 0.7.0 registry mismatch — FIXED in the follow-ups.**
  The redesign worker refreshed `themes/wordless-default/` to 0.7.0
  (style.css header, theme.json) while `DEFAULT_THEME_VERSION` in
  `crates/wordless-themes/src/registry.rs` still said 0.6.0, failing
  two golden tests. The const is now 0.7.0 and the CSS goldens are
  2346 bytes / hash `b2f0cc32848c` (measured by the v1 Node generator
  and confirmed by the Rust test); `cargo test -p wordless-themes
  --lib` is 63/63.
