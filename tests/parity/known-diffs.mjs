/**
 * WORDLESS v2 P5 — known differences between v1 and v2.
 *
 * A diff is "known" only when it is an INTENTIONAL, documented change
 * between the two versions — never a bug, never a guess. The harness
 * classifies a step as known-diff only when the step carries a
 * `knownDiff` id that exists here; every other diff is unexplained and
 * fails the run. Each entry must state the cause and the decision that
 * makes it intentional.
 */
export const KNOWN_DIFFS = {
  'health-version': {
    cause: 'Health endpoint reports the worker version: v1 returns "1.1.0", v2 returns "2.0.0-dev".',
    decision: 'Intentional — the two workers are different versions by design (compatibility contract §1.3 keeps the envelope, not the version string). The harness also normalises the version on that step.',
    owner: 'p5-harness',
  },
  'admin-spa-redesign': {
    cause: '/admin serves the redesigned React SPA (P4: 8 themes, new layout) on v2 and the v1 SPA on v1. The HTML/JS bundles differ.',
    decision: 'Intentional — P4 redesign, delivered and parent-verified. The harness compares status only for /admin/; the visual review is a staging-soak item in the cutover runbook.',
    owner: 'p4-redesign',
  },
  'theme-version': {
    cause: 'The bundled default theme is 0.6.0 on v1 and 0.7.0 on v2 (P4 public-theme rebrand: link colour fixed to AA 5.47:1, new tokens). theme.css content differs beyond the version string.',
    decision: 'Intentional — P4 public theme 0.7.0. The harness normalises the version string; the remaining CSS diff is classified here.',
    owner: 'p4-redesign',
  },
  'cpt-archive-v1-404': {
    cause: '/books and /books/dune: v1 has no custom post type API, so the routes fall through to 404; v2 serves the CPT archive and single pages (P2 gap closed in P3).',
    decision: 'Intentional — v2-only feature, a strict improvement over v1. The CPT API routes are marked v2only in the corpus; the public archive/single steps are classified here.',
    owner: 'p2-p3-cpt',
  },
  'must-change-password-field': {
    cause: 'v2 login/me responses carry an extra `data.mustChangePassword` boolean (false unless the user signed in with a temporary password after an admin reset); v1 has no such field.',
    decision: 'Intentional and additive — the P4 forced-password-change flow needs the client to know (P4 smoke checks `"mustChangePassword":false`). Handled in the harness by `ignorePaths: ["data.mustChangePassword"]` on login/me steps, so any other diff on those steps still surfaces as unexplained.',
    owner: 'p4-forced-reset',
  },
  'extra-createdat-field': {
    cause: 'v2 backup-create and import-job responses carry a `data.createdAt` timestamp that v1 does not return for those records.',
    decision: 'Additive only — the timestamp is real (creation time), nothing v1 returns changes shape. Handled by `ignorePaths: ["data.createdAt"]` on the three steps.',
    owner: 'p2-p3-records',
  },
  'ai-binding-config': {
    cause: 'v2 `wrangler.toml` declares the `[ai]` binding; v1 has it commented out (M6: honest not-configured). Under local workerd the binding exists, so v2 reports Workers AI as configured (providers list, Cloudflare binding status, provider test) while v1 reports it unbound.',
    decision: 'Configuration difference, not code: the two workers ship different wrangler.toml defaults by design. The mock AI path (used by every AI task step) is identical on both. Classified, not fixed — aligning the configs would hide a real deployment difference.',
    owner: 'p3-config',
  },
  'public-post-by-id': {
    cause: '`GET /?p=<id>`: v1 has an explicit `router.get("/")` that renders home and never reaches its `handlePublicGet` `?p=` branch; v2 `public_get` resolves `?p=` to the post first (WordPress behaviour).',
    decision: 'v2 is correct here; v1 has a routing quirk (its `?p=` branch is reachable only on non-"/" paths). Not "fixed" in v2 to match the bug.',
    owner: 'v1-quirk',
  },
  'tag-archive-v1-bug': {
    cause: '`GET /tag/<slug>`: v1 `findTermBySlug` queries `taxonomy = "tag"`, but tags are stored as `post_tag`, so every tag archive 404s on v1. v2 maps `tag` → `post_tag` in its resolver.',
    decision: 'v2 fixed the shared bug; v1 is frozen for P5 (the TS repo is read-only reference). The step expects the WordPress-correct 200.',
    owner: 'v1-bug-fixed-in-v2',
  },
  'conv-fixture-502': {
    cause: 'Conversion fixture job when upstream is unreachable: v1 creates the job (201) with status failed and the fetch error recorded on it; v2 returns 502 with no job ("upstream failure is a 502, never a half-created job" — deliberate, documented in `routes_p3.rs`).',
    decision: 'Deliberate v2 design difference, documented in code. Not changed in P5; the corpus expects v2 502 and classifies the v1/v2 status split here.',
    owner: 'p3-design',
  },
  // G7 (run 3): v2's migration 0011 adds wl_webhook_subscriptions +
  // wl_webhook_deliveries (P3 outbound webhooks, no v1 equivalent), so
  // a v2 backup legitimately dumps 2 more tables (33 vs 31) and a
  // larger file. Each worker faithfully backs up its own DB — the
  // delta is the schema, not the backup code.
  'backup-schema-delta': {
    reason: 'v2 has 2 extra tables from migration 0011 (webhooks); backup dumps its own DB faithfully',
    owner: 'expected — v2 schema extension, not a bug',
  },
  // P8: the cart page gained the Woo checkout fields (coupon code,
  // country, shipping method) so a shopper can apply them without
  // JavaScript. The page has 4 more lines than v1's and the
  // "Continue to payment" button sits lower; nothing v1 rendered
  // changed value. The step is text-compared, so the layout shift
  // is classified here rather than loosened to status-only.
  'shop-cart-woo-fields': {
    cause: 'GET /shop/cart: v2 renders the P8 coupon/country/shipping-method fields in the checkout form; v1 renders the plain P3 form (36 lines vs 40).',
    decision: 'Intentional — P8 storefront scope (doc §3 P8): the cart form carries the Woo checkout inputs. The fields are server-rendered, no JavaScript; the smoke test asserts each field is present.',
    owner: 'p8-storefront',
  },
  // P10: the robots.txt default now lists all four sitemap files
  // (main, index, image, video); v1 lists only the main sitemap.
  // The stored-file path is unchanged — a file with any Sitemap:
  // line is served exactly as written.
  'robots-default-sitemaps': {
    cause: 'GET /robots.txt and GET /api/seo/robots: v2 default (and the appended default lines) list /sitemap.xml, /sitemap-index.xml, /sitemap-images.xml and /sitemap-videos.xml; v1 lists only /sitemap.xml.',
    decision: 'Intentional — P10 scope (doc §0.1 F): crawlers must find the new sitemap files without guessing. The smoke test asserts all four lines.',
    owner: 'p10-seo',
  },
  // P10: public pages now carry more SEO head output — WebSite with
  // a SearchAction, Organization on the home page, and visible
  // breadcrumb navigation on pages with a trail. The underlying
  // content (titles, posts, links) is unchanged.
  'p10-seo-head': {
    cause: 'Public pages (home, single, page, archives, 404): v2 head carries WebSite+SearchAction (and Organization on home), and pages with a breadcrumb trail render a visible <nav aria-label="Breadcrumb"> at the top of <main>. v1 has the plain WebSite node and no visible breadcrumbs.',
    decision: 'Intentional — P10 scope (doc §0.1 F): full JSON-LD suite + visible breadcrumbs. The smoke test asserts each node and the breadcrumb nav per page type.',
    owner: 'p10-seo',
  },
};
