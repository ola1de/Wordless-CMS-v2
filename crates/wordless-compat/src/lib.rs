//! WORDLESS v2 WordPress compatibility registry (P3).
//!
//! Port of the registry data in v1 `src/compat/wordpress.ts` (the
//! WordPress function compatibility registry) in the v1.1.0
//! TypeScript repo (read-only reference). Each entry records an
//! honest support level, exactly as v1 states it:
//!
//! - `full` — implemented with WordPress semantics (sync hooks).
//! - `partial` — implemented for the common path; documented gaps remain.
//! - `stub` — recorded/thrown, not functional yet — never silently faked.
//! - `unsupported` — no v1 entry uses this level yet; it exists so a
//!   future entry (or a lookup of a function Wordless does not
//!   provide) can be named honestly instead of inventing a level.
//!
//! # Public API
//!
//! - [`SupportLevel`] — `Full | Partial | Stub | Unsupported`, with
//!   [`SupportLevel::as_str`] (`"full" | "partial" | "stub" |
//!   `"unsupported"`).
//! - [`CompatEntry`] — `{ name, level, description }`.
//! - [`WORDPRESS_COMPAT`] / [`registry()`] — the 45 WordPress
//!   functions from v1's `WORDPRESS_COMPAT`, same names, same levels,
//!   same descriptions. No entries invented, no levels upgraded.
//! - [`support_level(name)`] — the level of one WordPress function,
//!   or `None` when the name is not in the registry.
//! - [`compat_entry(name)`] — the full entry for one function.
//! - [`description(name)`] — the v1 description of one function.
//! - The `wl_*` Wordless API surfaces from the same v1 file (NOT
//!   WordPress functions, so they live in separate lists, exactly as
//!   in v1): [`AI_COMPAT`] / [`ai_registry()`], [`SEO_COMPAT`] /
//!   [`seo_registry()`], [`COMMERCE_COMPAT`] /
//!   [`commerce_registry()`], [`INTEGRATIONS_COMPAT`] /
//!   [`integrations_registry()`], [`SECURITY_COMPAT`] /
//!   [`security_registry()`], [`PERFORMANCE_COMPAT`] /
//!   [`performance_registry()`], [`BACKUP_COMPAT`] /
//!   [`backup_registry()`], [`IMPORT_COMPAT`] /
//!   [`import_registry()`], [`M11_COMPAT`] / [`m11_registry()`].
//!   Every `wl_*` entry is `partial` in v1, and stays `partial` here.
//! - [`esc_html`] / [`esc_attr`] — v1's `esc_html` / `esc_attr`
//!   (same escaping: `& < > " '`).
//! - [`wptexturize`] — v1's small real subset of WordPress's
//!   texturize (ellipsis, em/en dashes, curly quotes, apostrophes).
//!
//! What this crate deliberately does NOT contain: the callable
//! `createWordPressApi` implementation from v1. That API is async
//! over D1 and is assembled in v2 from the service crates that own
//! each function (`wordless-cms`, `wordless-media`, `wordless-seo`,
//! `wordless-themes`, …) when the P3 worker wiring lands; a registry
//! crate must not grow DB or hook-system dependencies. The
//! DB-backed behaviour is covered by those crates' tests, and the
//! registry assertions from v1's `wordpress.test.ts` are ported here.
//!
//! Pure logic — no `Db`, no Worker types, no dependencies at all,
//! cargo-testable on native targets. Hand-rolled string matching
//! only (no `regex` crate).

/// Honest support level of a compat entry (v1 `SupportLevel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportLevel {
    /// Implemented with WordPress semantics.
    Full,
    /// Implemented for the common path; documented gaps remain.
    Partial,
    /// Recorded/thrown, not functional yet — never silently faked.
    Stub,
    /// Not provided. No v1 entry uses this level yet.
    Unsupported,
}

impl SupportLevel {
    /// The v1 level string: `"full" | "partial" | "stub" | "unsupported"`.
    pub fn as_str(self) -> &'static str {
        match self {
            SupportLevel::Full => "full",
            SupportLevel::Partial => "partial",
            SupportLevel::Stub => "stub",
            SupportLevel::Unsupported => "unsupported",
        }
    }
}

/// One registry entry (v1 `CompatEntry`): the function name, its
/// support level, and v1's description of what the level means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompatEntry {
    pub name: &'static str,
    pub level: SupportLevel,
    pub description: &'static str,
}

/// The WordPress function registry (v1 `WORDPRESS_COMPAT`): 45
/// entries, same names, same levels, same descriptions as v1.
/// No entries invented, no levels upgraded.
pub static WORDPRESS_COMPAT: &[CompatEntry] = &[
    CompatEntry { name: "get_option", level: SupportLevel::Partial, description: "Reads wp_options; values are JSON, not PHP-serialized. No autoload/transient handling yet." },
    CompatEntry { name: "update_option", level: SupportLevel::Partial, description: "Upserts wp_options as JSON. No autoload/transient handling yet." },
    CompatEntry { name: "delete_option", level: SupportLevel::Partial, description: "Deletes from wp_options. Returns whether a row was removed." },
    CompatEntry { name: "add_option", level: SupportLevel::Partial, description: "Inserts only if absent; returns false when the option already exists." },
    CompatEntry { name: "get_post", level: SupportLevel::Partial, description: "Reads wp_posts by ID. No WP_Post object extras (no filtered content, no meta preloading)." },
    CompatEntry { name: "get_posts", level: SupportLevel::Partial, description: "Basic list (status/type/author/page/per_page). No WP_Query taxonomy/meta queries yet." },
    CompatEntry { name: "wp_insert_post", level: SupportLevel::Partial, description: "Creates a post/page/attachment row with slug generation. No hooks fired yet; no meta input handling." },
    CompatEntry { name: "wp_update_post", level: SupportLevel::Partial, description: "Updates allowlisted fields. Creates a revision of the pre-update content (M2)." },
    CompatEntry { name: "wp_delete_post", level: SupportLevel::Partial, description: "Trashes by default; force=true hard-deletes the row and its postmeta." },
    CompatEntry { name: "add_action", level: SupportLevel::Full, description: "Sync hook registration, priority order (default 10), stable order." },
    CompatEntry { name: "remove_action", level: SupportLevel::Full, description: "Removes by callback identity, optionally scoped to a priority." },
    CompatEntry { name: "add_filter", level: SupportLevel::Full, description: "Sync filter registration, priority order (default 10), stable order." },
    CompatEntry { name: "remove_filter", level: SupportLevel::Full, description: "Removes by callback identity, optionally scoped to a priority." },
    CompatEntry { name: "apply_filters", level: SupportLevel::Full, description: "Chains a value through registered filters in priority order." },
    CompatEntry { name: "do_action", level: SupportLevel::Full, description: "Runs registered action callbacks in priority order." },
    CompatEntry { name: "register_post_type", level: SupportLevel::Partial, description: "Registers in memory only (not persisted, no admin UI, no rewrite rules yet)." },
    CompatEntry { name: "register_taxonomy", level: SupportLevel::Partial, description: "Registers in memory only (not persisted, no term UI yet)." },
    CompatEntry { name: "add_shortcode", level: SupportLevel::Partial, description: "Registers the tag and callback in memory; shortcode rendering in post content is not built yet." },
    CompatEntry { name: "wp_schedule_event", level: SupportLevel::Partial, description: "Persists a scheduled job row in wl_jobs; no cron runner executes it yet (Workers Cron Triggers are a later step)." },
    CompatEntry { name: "current_user_can", level: SupportLevel::Partial, description: "Checks the capability map for the current request user; false when anonymous." },
    CompatEntry { name: "get_user_meta", level: SupportLevel::Partial, description: "Reads wp_usermeta as JSON; single-value only (no multi-value arrays yet)." },
    CompatEntry { name: "update_user_meta", level: SupportLevel::Partial, description: "Upserts wp_usermeta as JSON." },
    CompatEntry { name: "wp_enqueue_script", level: SupportLevel::Partial, description: "Records the handle/src in memory; scripts are not rendered into pages yet." },
    CompatEntry { name: "wp_enqueue_style", level: SupportLevel::Partial, description: "Records the handle/src in memory; styles are not rendered into pages yet." },
    CompatEntry { name: "register_rest_route", level: SupportLevel::Stub, description: "STUB: routes are recorded for reporting but NOT mounted on the Worker router in M1." },
    CompatEntry { name: "wp_get_post_terms", level: SupportLevel::Partial, description: "Terms assigned to a post (M2). Returns Term objects; no WP_Term extras." },
    CompatEntry { name: "get_the_terms", level: SupportLevel::Partial, description: "Alias of wp_get_post_terms for one taxonomy (M2)." },
    CompatEntry { name: "wp_list_categories", level: SupportLevel::Partial, description: "HTML list of categories with counts (M2). No Walker/custom args yet." },
    CompatEntry { name: "wp_get_post_revisions", level: SupportLevel::Partial, description: "Revision snapshots for a post, newest first (M2)." },
    CompatEntry { name: "wp_restore_post_revision", level: SupportLevel::Partial, description: "Restores a revision; snapshots the pre-restore content first (M2)." },
    CompatEntry { name: "wp_nav_menu", level: SupportLevel::Partial, description: "HTML for a menu by location or id (M2). No Walker/theme args yet." },
    CompatEntry { name: "get_media_items", level: SupportLevel::Partial, description: "Attachment posts, paginated with a MIME filter (M3). Returns MediaRecord objects, not WP_Post objects." },
    CompatEntry { name: "wp_get_attachment_url", level: SupportLevel::Partial, description: "Public /media/<key> URL for an attachment id (M3). Empty string when the id is not an attachment." },
    CompatEntry { name: "media_handle_upload", level: SupportLevel::Partial, description: "Validates and stores a file in R2, creating the attachment post + postmeta (M3). Needs a MediaStorage in the context; throws a stub error when none is wired. Image derivatives exist since M9 via the Cloudflare Images binding (originals served when unbound)." },
    CompatEntry { name: "get_user_locale", level: SupportLevel::Partial, description: "The current user's locale: the locale passed in the WpContext (resolved once per request from the site_locale option), default en_US (M4). No per-user locale UI yet." },
    CompatEntry { name: "get_locale", level: SupportLevel::Partial, description: "Site locale from the WpContext (resolved once per request from the site_locale option), default en_US (M4)." },
    CompatEntry { name: "__", level: SupportLevel::Partial, description: "Returns the string unchanged: no translation files exist in M4, so there is nothing to translate from (M4)." },
    CompatEntry { name: "_e", level: SupportLevel::Partial, description: "Returns the string unchanged (same as __); the emitter turns the surrounding echo into output (M4)." },
    CompatEntry { name: "esc_html", level: SupportLevel::Partial, description: "Escapes & < > \" ' for HTML output (M4)." },
    CompatEntry { name: "esc_attr", level: SupportLevel::Partial, description: "Escapes & < > \" ' for attribute output — same escaping as esc_html in M4 (M4)." },
    CompatEntry { name: "wptexturize", level: SupportLevel::Partial, description: "Converts straight quotes to curly quotes, --/--- to dashes, ... to an ellipsis (M4). A small subset of WordPress's full texturize." },
    CompatEntry { name: "get_theme_root", level: SupportLevel::Partial, description: "Returns the R2 prefix \"themes/\" where theme files live (M5). WordPress returns a filesystem path; Workers has no filesystem, so this is the object-storage prefix instead." },
    CompatEntry { name: "wp_get_theme", level: SupportLevel::Partial, description: "Returns the active theme (or the named one): slug, name, version, author, status, kind (M5). No WP_Theme object extras (no template files list, no parent/child resolution)." },
    CompatEntry { name: "get_stylesheet", level: SupportLevel::Partial, description: "Returns the active theme's slug (M5). The stylesheet itself is served at /theme.css." },
    CompatEntry { name: "get_template", level: SupportLevel::Partial, description: "Same as get_stylesheet in v1 (M5): Wordless has no parent/child theme hierarchy yet, so template and stylesheet are always the same theme." },
];

/// The WordPress function registry ([`WORDPRESS_COMPAT`]).
pub fn registry() -> &'static [CompatEntry] {
    WORDPRESS_COMPAT
}

/// Look up a compat entry by its WordPress name (v1 `compatEntry`).
pub fn compat_entry(name: &str) -> Option<&'static CompatEntry> {
    WORDPRESS_COMPAT.iter().find(|e| e.name == name)
}

/// The support level of one WordPress function, or `None` when the
/// name is not in the registry.
pub fn support_level(name: &str) -> Option<SupportLevel> {
    compat_entry(name).map(|e| e.level)
}

/// The v1 description of one WordPress function, or `None` when the
/// name is not in the registry.
pub fn description(name: &str) -> Option<&'static str> {
    compat_entry(name).map(|e| e.description)
}

/// Wordless AI API surface (v1 `AI_COMPAT`, Milestone 6) — NOT
/// WordPress functions, so a separate list, exactly as in v1. Each
/// entry maps to a real HTTP endpoint under `/api/ai/*`.
pub static AI_COMPAT: &[CompatEntry] = &[
    CompatEntry { name: "wl_ai_complete", level: SupportLevel::Partial, description: "POST /api/ai/tasks — run an agent with the selected provider (Workers AI by default, mock in local dev). Fails honestly with ai_not_configured when no provider is configured." },
    CompatEntry { name: "wl_ai_providers", level: SupportLevel::Partial, description: "GET /api/ai/providers (+ /select, /:id/test) — list providers with live configured state; Workers AI is the default, OpenAI-compatible is an optional fallback." },
    CompatEntry { name: "wl_ai_mode", level: SupportLevel::Partial, description: "GET/PUT /api/ai/mode — manual / supervised / autonomous. The orchestrator enforces the mode; supervised never applies without approval." },
    CompatEntry { name: "wl_ai_agents", level: SupportLevel::Partial, description: "GET /api/ai/agents — the 15-agent roster with live availability (commerce-assistant is live since Milestone 8, reading real store data)." },
    CompatEntry { name: "wl_ai_approve", level: SupportLevel::Partial, description: "POST /api/ai/approvals/:id/approve|reject — human gate. Approving applies the proposal through the existing services; a failed apply leaves the approval pending." },
    CompatEntry { name: "wl_ai_memory", level: SupportLevel::Partial, description: "GET/POST /api/ai/memory — website facts + admin notes used as agent context. Rebuild re-ingests from the site; notes never contain secrets." },
];

/// The Wordless AI API surface ([`AI_COMPAT`]).
pub fn ai_registry() -> &'static [CompatEntry] {
    AI_COMPAT
}

/// Wordless SEO API surface (v1 `SEO_COMPAT`, Milestone 7) — NOT
/// WordPress functions. Each entry maps to a real HTTP endpoint under
/// `/api/posts/:id/seo` or `/api/seo/*`.
pub static SEO_COMPAT: &[CompatEntry] = &[
    CompatEntry { name: "wl_seo_fields", level: SupportLevel::Partial, description: "GET/PUT /api/posts/:id/seo — per-post SEO title, description, focus keyword, canonical, noindex, OG/Twitter fields, stored as wp_postmeta. Returned inside the post detail payload." },
    CompatEntry { name: "wl_seo_analysis", level: SupportLevel::Partial, description: "GET /api/posts/:id/seo/analysis — deterministic 0–100 score from stored content (title, description, keyword, headings, alt text, links, word count, readability estimate, canonical). No rankings or traffic are invented." },
    CompatEntry { name: "wl_seo_head", level: SupportLevel::Partial, description: "Public pages render <title>, meta description, canonical, robots, Open Graph, Twitter Card and JSON-LD (WebSite + Article + BreadcrumbList) with stored-field fallbacks." },
    CompatEntry { name: "wl_seo_sitemap", level: SupportLevel::Partial, description: "GET /sitemap.xml — paginated at 1,000 URLs (index above that), changefreq/priority from recency, image entries only for existing attachments." },
    CompatEntry { name: "wl_seo_robots", level: SupportLevel::Partial, description: "GET/PUT /api/seo/robots — configurable robots.txt; rules blocking /admin or /api are rejected." },
    CompatEntry { name: "wl_seo_redirects", level: SupportLevel::Partial, description: "GET/POST/PUT/DELETE /api/seo/redirects — manual 301s over wl_redirects (slug changes already create them automatically)." },
    CompatEntry { name: "wl_seo_404s", level: SupportLevel::Partial, description: "GET/DELETE /api/seo/404s — public 404 log (query strings stripped, capped at 500 paths) with one-click redirect creation." },
    CompatEntry { name: "wl_seo_templates", level: SupportLevel::Partial, description: "GET /api/seo/templates + POST /api/seo/templates/:slug/create — three content templates (announcement, how-to, news update) that create real draft posts." },
    CompatEntry { name: "wl_seo_calendar", level: SupportLevel::Partial, description: "GET /api/seo/calendar — stored posts grouped by date for the next 30 days (a draft with a future date is the schedule; no 'future' status yet)." },
    CompatEntry { name: "wl_seo_import", level: SupportLevel::Partial, description: "POST /api/seo/import — Yoast SEO (_yoast_wpseo_*) and Rank Math (rank_math_*) metadata import by post ID or slug; unknown keys and invalid rows are reported, never silently dropped." },
];

/// The Wordless SEO API surface ([`SEO_COMPAT`]).
pub fn seo_registry() -> &'static [CompatEntry] {
    SEO_COMPAT
}

/// Wordless commerce API surface (v1 `COMMERCE_COMPAT`, Milestone 8)
/// — NOT WordPress functions.
pub static COMMERCE_COMPAT: &[CompatEntry] = &[
    CompatEntry { name: "wl_commerce_products", level: SupportLevel::Partial, description: "GET /api/commerce/products (public, published only) + POST/PUT/DELETE (manage_options). Products: name, slug, block-HTML description, price in integer minor units, currency (NGN default), stock (-1 = unlimited), draft/published, featured image attachment id." },
    CompatEntry { name: "wl_commerce_shop", level: SupportLevel::Partial, description: "Public storefront at /shop (list) and /shop/:slug (product page, add-to-cart form, share links), rendered through the theme system; /shop/cart shows the server-side cart with checkout form." },
    CompatEntry { name: "wl_commerce_cart", level: SupportLevel::Partial, description: "GET/POST/PUT/DELETE /api/commerce/cart — server-side cart in D1 (wl_carts, 7-day expiry, pruned on read), keyed by the wordless_cart cookie. Stock is checked at add time." },
    CompatEntry { name: "wl_commerce_checkout", level: SupportLevel::Partial, description: "POST /api/commerce/checkout — validates a non-empty cart + stock, creates a pending order, starts a checkout with the selected provider. With no provider configured it returns 503 — never a fake success." },
    CompatEntry { name: "wl_commerce_orders", level: SupportLevel::Partial, description: "GET /api/commerce/orders (+ /:id) and POST /api/commerce/orders/:id/transition — pending → paid → fulfilled / cancelled. Stock decrements exactly once when an order becomes paid and is restored when a paid order is cancelled." },
    CompatEntry { name: "wl_commerce_payments", level: SupportLevel::Partial, description: "Payment providers: Paystack (HMAC-SHA512 webhook), Flutterwave (verif-hash), Stripe (HMAC-SHA256, 300 s timestamp tolerance), plus a deterministic mock for test mode. GET /api/commerce/payments/providers + /selected + POST /select. Secrets live only in env/secrets." },
    CompatEntry { name: "wl_commerce_webhooks", level: SupportLevel::Partial, description: "POST /api/commerce/webhooks/:provider — the provider signature IS the auth (401 on bad/missing). Every attempt is logged in wl_webhook_log. A paid webhook moves the order pending → paid exactly once; replays are idempotent." },
    CompatEntry { name: "wl_commerce_assistant", level: SupportLevel::Partial, description: "The commerce-assistant AI agent (M6) is live since M8: it reports real products, order counts by status, low-stock warnings and revenue (sum of paid orders, minor units), computed from stored rows only." },
];

/// The Wordless commerce API surface ([`COMMERCE_COMPAT`]).
pub fn commerce_registry() -> &'static [CompatEntry] {
    COMMERCE_COMPAT
}

/// Wordless integrations surface (v1 `INTEGRATIONS_COMPAT`,
/// Milestone 8) — the integration registry, email, analytics, social
/// and the marketplace skeleton. NOT WordPress functions.
pub static INTEGRATIONS_COMPAT: &[CompatEntry] = &[
    CompatEntry { name: "wl_integrations", level: SupportLevel::Partial, description: "GET /api/integrations (+ /:id/config GET/PUT) — registry of 8 integrations (email, paystack, flutterwave, stripe, analytics, social, cloudflare, marketplace) with live configured/not-configured detection from env." },
    CompatEntry { name: "wl_integrations_email", level: SupportLevel::Partial, description: "sendMail — mock provider logs to wl_mail_log in test mode (never claims delivery); generic HTTP provider (Resend/SendGrid-shaped) sends via a configured endpoint + secret. Used for order confirmations, comment notifications and password resets. GET /api/integrations/email/log (admin)." },
    CompatEntry { name: "wl_integrations_analytics", level: SupportLevel::Partial, description: "GET /api/integrations/analytics/status — GA4/Plausible configuration (provider id + measurement id, non-secret). The snippet is injected only when configured. No traffic numbers are ever fabricated." },
    CompatEntry { name: "wl_integrations_social", level: SupportLevel::Partial, description: "Share links (X/Facebook/WhatsApp/Telegram) on public post and product pages — plain links, no API calls, no fake counts." },
    CompatEntry { name: "wl_integrations_cloudflare", level: SupportLevel::Partial, description: "GET /api/integrations/cloudflare — read-only display of the bindings actually present in this environment (DB, MEDIA, CONFIG, ASSETS, AI). No IDs are invented." },
    CompatEntry { name: "wl_integrations_marketplace", level: SupportLevel::Partial, description: "GET /api/marketplace + POST /api/marketplace/install (ZIP upload, wordless.json manifest, ≤50 MB / ≤10,000 files) + activate/deactivate + GET /api/marketplace/:slug/report (compatibility + auto-conversion report). Local-ZIP only — no remote directory, and installed code is never executed." },
];

/// The Wordless integrations surface ([`INTEGRATIONS_COMPAT`]).
pub fn integrations_registry() -> &'static [CompatEntry] {
    INTEGRATIONS_COMPAT
}

/// Wordless security surface (v1 `SECURITY_COMPAT`, Milestone 9) —
/// NOT WordPress functions.
pub static SECURITY_COMPAT: &[CompatEntry] = &[
    CompatEntry { name: "wl_security_headers", level: SupportLevel::Partial, description: "Every response carries Content-Security-Policy, X-Content-Type-Options: nosniff, X-Frame-Options: DENY, Referrer-Policy and Permissions-Policy. The CSP allows the inline styles/scripts the admin and analytics need (documented in docs/security/security.md); a nonce-based policy is a later step." },
    CompatEntry { name: "wl_security_lockout", level: SupportLevel::Partial, description: "POST /api/auth/login is guarded: 5 failed attempts in 15 minutes lock a login name/IP for 15 minutes (doubling on consecutive lockouts, capped at 60); a successful login clears the counter. Thresholds are options (wl_security_max_attempts / wl_security_window_minutes / wl_security_lockout_minutes)." },
    CompatEntry { name: "wl_security_2fa", level: SupportLevel::Partial, description: "POST /api/auth/2fa/setup|confirm|disable + GET /api/auth/2fa/status — TOTP (RFC 6238, HMAC-SHA1, 30 s steps, ±1 step skew) with 8 one-time hashed recovery codes. The secret is stored only after a valid code confirms setup; login accepts an optional totp field." },
    CompatEntry { name: "wl_security_sessions", level: SupportLevel::Partial, description: "GET /api/auth/sessions + POST /api/auth/sessions/revoke-all — idle timeout (wl_session_idle_hours, default 24 h) enforced on verify; sessions are listed without token values." },
    CompatEntry { name: "wl_security_uploads", level: SupportLevel::Partial, description: "Uploads keep the M3 allowlist (jpeg/png/gif/webp/avif/svg/pdf, 50 MB) and add magic-byte sniffing — a PHP file renamed to .jpg is rejected; SVGs are checked for scripts and javascript: URLs." },
    CompatEntry { name: "wl_security_overview", level: SupportLevel::Partial, description: "GET /api/security/overview — measured posture: headers policy, lockout config + locked keys, 2FA per admin, active sessions, recent failed logins, and whether a user with the default login name \"admin\" exists (login name only — passwords are never tested)." },
    CompatEntry { name: "wl_security_agents", level: SupportLevel::Partial, description: "The M6 security-scanner agent now runs the real checks above and the performance-analyst measures real D1/R2 numbers; the backup-advisor reports the real backup registry and the migration-assistant points at the real M9 importer." },
];

/// The Wordless security surface ([`SECURITY_COMPAT`]).
pub fn security_registry() -> &'static [CompatEntry] {
    SECURITY_COMPAT
}

/// Wordless performance surface (v1 `PERFORMANCE_COMPAT`, Milestone 9)
/// — NOT WordPress functions.
pub static PERFORMANCE_COMPAT: &[CompatEntry] = &[
    CompatEntry { name: "wl_performance_cache", level: SupportLevel::Partial, description: "Anonymous public GETs are cached 60 s with stale-while-revalidate=60; publishing a post purges its URL, the home page and its term pages from the Cache API (purge failures never fail the publish). Imports do NOT purge — an imported post appears once the 60 s entry expires. Cache hit rates are not observable from inside the Worker and are not reported." },
    CompatEntry { name: "wl_performance_derivatives", level: SupportLevel::Partial, description: "GET /media/:key?width=&format= — resized/WebP variants via the Cloudflare Images binding when bound (stored in R2 + wl_media_derivatives for srcset); when unbound the endpoint serves the original and says so (X-Wordless-Derivatives: unavailable). A resize is never faked." },
    CompatEntry { name: "wl_performance_health", level: SupportLevel::Partial, description: "The Site Health response carries a performance section with measured numbers: D1 row counts, R2 object count (capped at 1,000, reported as 'at least'), cache config and derivative status. Compression is applied by the Cloudflare edge, not the Worker." },
];

/// The Wordless performance surface ([`PERFORMANCE_COMPAT`]).
pub fn performance_registry() -> &'static [CompatEntry] {
    PERFORMANCE_COMPAT
}

/// Wordless backup surface (v1 `BACKUP_COMPAT`) — NOT WordPress functions.
pub static BACKUP_COMPAT: &[CompatEntry] = &[
    CompatEntry { name: "wl_backup_create", level: SupportLevel::Partial, description: "POST /api/backup/create (admin, wl_jobs job) — a real ZIP in R2 (backups/<id>.zip): one JSON file per D1 table, a media manifest (R2 keys + sizes + etags), and a manifest with a SHA-256 checksum per file. wl_backups and wl_jobs are excluded by design." },
    CompatEntry { name: "wl_backup_verify", level: SupportLevel::Partial, description: "POST /api/backup/:id/verify — re-reads the bundle from R2 and recomputes every checksum; only then is the backup marked verified. GET /api/backup lists with size, checksum and verified flag." },
    CompatEntry { name: "wl_backup_restore", level: SupportLevel::Partial, description: "POST /api/backup/:id/restore — requires the typed confirmation \"RESTORE\" and a passing verification, refuses a different minor version, restores tables in dependency order inside batched transactions, and checks the media manifest key-by-key (missing keys are reported, never silently skipped). GET /api/backup/:id/download serves the ZIP." },
];

/// The Wordless backup surface ([`BACKUP_COMPAT`]).
pub fn backup_registry() -> &'static [CompatEntry] {
    BACKUP_COMPAT
}

/// Wordless import surface (v1 `IMPORT_COMPAT`) — NOT WordPress functions.
pub static IMPORT_COMPAT: &[CompatEntry] = &[
    CompatEntry { name: "wl_import_wxr", level: SupportLevel::Partial, description: "POST /api/import/wxr (admin, multipart ≤50 MB, wl_jobs job) — WordPress eXtended RSS via a real XML parser: authors (forced password reset), categories/tags, posts/pages (block content verbatim), comments, attachments (URL fetch → R2; failed fetches recorded per item). Per-item report with imported/skipped/failed counts." },
    CompatEntry { name: "wl_import_sql", level: SupportLevel::Partial, description: "POST /api/import/sql (admin, ≤50 MB, wl_jobs job) — mysqldump parsing for the 12 wp_* tables only (other tables reported as ignored). PHP-serialized values become JSON via the safe unserializer; serialized objects are refused and recorded. Original IDs are kept when free; revisions are skipped." },
    CompatEntry { name: "wl_import_fixtures", level: SupportLevel::Partial, description: "test/fixtures/wordpress/ carries a synthetic WXR export + SQL dump (posts/pages/terms/comments/users/options, serialized values, a refused object payload, a revision, a non-wp_* table) that the importer is proven against in tests; the admin Import page links to both files." },
];

/// The Wordless import surface ([`IMPORT_COMPAT`]).
pub fn import_registry() -> &'static [CompatEntry] {
    IMPORT_COMPAT
}

/// Milestone 11 surface (v1 `M11_COMPAT`) — auto-convert, Elementor
/// rendering, file manager. NOT WordPress functions.
pub static M11_COMPAT: &[CompatEntry] = &[
    CompatEntry { name: "wl_m11_limits", level: SupportLevel::Partial, description: "Upload caps raised to 50 MB / 10,000 files: theme ZIPs (was 200 files / 5 MB), marketplace ZIPs (was 100 files / 2 MB), media uploads (was 10 MB), WXR/SQL imports (was 10 MB). Near-cap uploads can still be slow or fail on the Cloudflare free tier (128 MB Worker memory, request buffering); chunked/resumable upload is a future improvement." },
    CompatEntry { name: "wl_m11_compat_report", level: SupportLevel::Partial, description: "GET /api/marketplace/:slug/report — per-plugin compatibility report stored in wl_extensions.report_json: PHP files analysed (max 2,000, 1 MB per file), convertible / partial / blocked / unparsed counts, top blocking reasons with file paths, per-file verdicts (capped at 500). Static analysis only — never a guarantee the plugin works; a plugin is never labelled as working while its report shows partial/blocked files." },
    CompatEntry { name: "wl_m11_auto_convert", level: SupportLevel::Partial, description: "Theme imports (block + classic) and marketplace installs automatically run the M4 converter over PHP files (max 500 files, 1 MB per file) and store the generated TypeScript in R2 under <prefix>/converted/<path>.ts — stored, never executed. Literal get_option() defaults are carried over as Wordless options (existing values never overwritten); theme.json settings are reported as applied or skipped with a reason. Per-file record (converted / partial / blocked + reason) and settings report are stored with the theme/plugin and shown in admin." },
    CompatEntry { name: "wl_m11_elementor", level: SupportLevel::Partial, description: "Posts carrying _elementor_data postmeta render their Elementor layout on the public site (sections → columns → widgets) for heading, text-editor, image, button, spacer, divider, video, icon-list, image-box and google-maps. Unknown widgets render nothing and are collected as unsupported; icon glyphs are not rendered (no icon font bundled). This renders Elementor *content* only — the Elementor drag-and-drop editor is NOT included; pages are edited in Wordless's own block editor." },
    CompatEntry { name: "wl_m11_files", level: SupportLevel::Partial, description: "GET /api/files (+ ?prefix=), POST /api/files/upload, GET /api/files/download, POST /api/files/rename, POST /api/files/delete — admin file manager over the R2 bucket (manage_options on every route). Folder navigation by prefix, uploads ≤50 MB with the media MIME allowlist + magic-byte sniffing, no in-place editing, no shell access; extensions/, themes/, backups/ and imports/ are read-only here; path traversal is rejected." },
];

/// The Milestone 11 surface ([`M11_COMPAT`]).
pub fn m11_registry() -> &'static [CompatEntry] {
    M11_COMPAT
}

/// Escape `& < > " '` for HTML output (v1 `esc_html` /
/// `wordpress.ts` `escapeHtml`). `esc_attr` is the same escaping in
/// v1, so [`esc_attr`] is an alias.
pub fn esc_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escape `& < > " '` for attribute output — same escaping as
/// [`esc_html`] in v1 (M4).
pub fn esc_attr(text: &str) -> String {
    esc_html(text)
}

/// `wptexturize` (v1, M4) — a small, real subset of WordPress's
/// texturize: ellipsis, em/en dashes, curly double quotes, and
/// apostrophes inside words. Hand-rolled (no `regex` crate); the
/// replacement order and consuming-match semantics match v1's
/// regex chain: `...` → `…`, `---` → `—`, `--` → `–`, then
/// apostrophes, then paired double quotes, then remaining opening
/// and closing double quotes.
pub fn wptexturize(text: &str) -> String {
    // Ellipsis and dashes first (longest first).
    let step1 = text.replace("...", "…");
    let step2 = step1.replace("---", "—");
    let step3 = step2.replace("--", "–");
    // Apostrophes: consuming left-to-right scan, as a global regex
    // match would consume its match.
    let step4 = texturize_apostrophes(&step3);
    // Paired double quotes on one line: "word" → “word”.
    let step5 = texturize_paired_quotes(&step4);
    // Remaining opening double quote (after space or start) → “,
    // remaining closing double quote → ”.
    texturize_lone_quotes(&step5)
}

fn texturize_apostrophes(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\''
            && i > 0
            && chars[i - 1].is_ascii_alphabetic()
            && (i + 1 < chars.len() && chars[i + 1].is_ascii_alphabetic()
                || i + 1 >= chars.len()
                || chars[i + 1].is_whitespace())
        {
            // ([A-Za-z])'([A-Za-z]) or ([A-Za-z])'(\s|$): the letter
            // before was already emitted, so only the curly quote is
            // appended; the following letter/space is emitted next.
            out.push('’');
            i += 1;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn texturize_paired_quotes(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '"' {
            // Find a closing quote on the same line, at least one char in.
            let mut j = i + 1;
            let mut close = None;
            while j < chars.len() && chars[j] != '\n' {
                if chars[j] == '"' {
                    close = Some(j);
                    break;
                }
                j += 1;
            }
            if let Some(end) = close {
                out.push('“');
                out.extend(chars[i + 1..end].iter());
                out.push('”');
                i = end + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn texturize_lone_quotes(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (i, c) in chars.iter().enumerate() {
        if *c == '"' {
            let prev = if i == 0 { None } else { Some(chars[i - 1]) };
            if prev.is_none()
                || prev == Some(' ')
                || prev == Some('\t')
                || prev == Some('\n')
                || prev == Some('\r')
                || prev == Some('(')
            {
                out.push('“');
            } else {
                out.push('”');
            }
        } else {
            out.push(*c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn wordpress_registry_has_45_unique_entries_with_descriptions() {
        assert_eq!(registry().len(), 45);
        let mut names = HashSet::new();
        for entry in registry() {
            assert!(names.insert(entry.name), "duplicate name: {}", entry.name);
            assert!(
                !entry.description.is_empty(),
                "empty description: {}",
                entry.name
            );
        }
        // The brief's 25th entry is add_option, not a duplicated get_user_meta.
        assert!(registry().iter().any(|e| e.name == "add_option"));
        assert_eq!(
            registry()
                .iter()
                .filter(|e| e.name == "get_user_meta")
                .count(),
            1
        );
    }

    #[test]
    fn hook_functions_are_full() {
        for name in [
            "add_action",
            "remove_action",
            "add_filter",
            "remove_filter",
            "apply_filters",
            "do_action",
        ] {
            assert_eq!(support_level(name), Some(SupportLevel::Full), "{name}");
            assert_eq!(support_level(name).unwrap().as_str(), "full");
        }
    }

    #[test]
    fn register_rest_route_is_the_only_stub() {
        assert_eq!(
            support_level("register_rest_route"),
            Some(SupportLevel::Stub)
        );
        let stubs: Vec<&str> = registry()
            .iter()
            .filter(|e| e.level == SupportLevel::Stub)
            .map(|e| e.name)
            .collect();
        assert_eq!(stubs, vec!["register_rest_route"]);
    }

    #[test]
    fn media_functions_are_partial() {
        // Port of wordpress.test.ts "media compat functions (M3)".
        for name in [
            "get_media_items",
            "wp_get_attachment_url",
            "media_handle_upload",
        ] {
            let entry = compat_entry(name);
            assert!(entry.is_some(), "{name}");
            assert_eq!(entry.unwrap().level, SupportLevel::Partial);
        }
    }

    #[test]
    fn conversion_functions_are_partial() {
        // Port of wordpress.test.ts "conversion compat functions (M4)".
        for name in [
            "get_user_locale",
            "get_locale",
            "__",
            "_e",
            "esc_html",
            "esc_attr",
            "wptexturize",
        ] {
            assert_eq!(support_level(name), Some(SupportLevel::Partial), "{name}");
        }
    }

    #[test]
    fn unknown_names_return_none() {
        assert_eq!(support_level("wp_nonexistent_function"), None);
        assert_eq!(compat_entry("wp_nonexistent_function"), None);
        assert_eq!(description("wp_nonexistent_function"), None);
        assert_eq!(support_level(""), None);
    }

    #[test]
    fn unsupported_level_exists_but_is_unused_in_v1() {
        assert_eq!(SupportLevel::Unsupported.as_str(), "unsupported");
        assert_eq!(SupportLevel::Partial.as_str(), "partial");
        assert!(!registry()
            .iter()
            .any(|e| e.level == SupportLevel::Unsupported));
    }

    #[test]
    fn description_lookup_matches_v1() {
        assert_eq!(
            description("get_option"),
            Some("Reads wp_options; values are JSON, not PHP-serialized. No autoload/transient handling yet.")
        );
        assert_eq!(
            description("register_rest_route"),
            Some("STUB: routes are recorded for reporting but NOT mounted on the Worker router in M1.")
        );
    }

    #[test]
    fn wordless_surface_lists_have_v1_counts_and_are_all_partial() {
        // (list, v1 count) — all entries partial in v1.
        let lists: &[(&[CompatEntry], usize)] = &[
            (ai_registry(), 6),
            (seo_registry(), 10),
            (commerce_registry(), 8),
            (integrations_registry(), 6),
            (security_registry(), 7),
            (performance_registry(), 3),
            (backup_registry(), 3),
            (import_registry(), 3),
            (m11_registry(), 5),
        ];
        for (list, count) in lists {
            assert_eq!(list.len(), *count);
            assert!(list.iter().all(|e| e.level == SupportLevel::Partial));
            assert!(list.iter().all(|e| e.name.starts_with("wl_")));
            assert!(list.iter().all(|e| !e.description.is_empty()));
        }
    }

    #[test]
    fn esc_html_and_esc_attr_match_v1() {
        // Port of wordpress.test.ts conversion assertions.
        assert_eq!(esc_html("<b>"), "&lt;b&gt;");
        assert_eq!(esc_attr("a\"b"), "a&quot;b");
        assert_eq!(esc_html("A & B <\"x\">"), "A &amp; B &lt;&quot;x&quot;&gt;");
        assert_eq!(esc_attr("it's"), "it&#39;s");
    }

    #[test]
    fn wptexturize_matches_v1() {
        // Port of wordpress.test.ts conversion assertions.
        assert_eq!(wptexturize("Hello..."), "Hello…");
        assert_eq!(wptexturize("You're goin'"), "You’re goin’");
        assert_eq!(wptexturize("wait--what"), "wait–what");
        assert_eq!(wptexturize("wait---what"), "wait—what");
        assert_eq!(wptexturize("\"word\""), "“word”");
        assert_eq!(wptexturize("say \"hi\" now"), "say “hi” now");
        assert_eq!(wptexturize(""), "");
        assert_eq!(wptexturize("plain text"), "plain text");
    }
}
