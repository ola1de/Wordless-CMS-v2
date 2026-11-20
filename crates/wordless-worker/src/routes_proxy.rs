//! Sub-worker proxy routes (size remedy, 2026-11-20).
//!
//! The social (P7), WooCommerce (P8), school (P9) and SEO (P10)
//! routes moved to their own
//! Workers, but the public API does not change: this table registers
//! every moved route on the main Worker's router with the SAME path,
//! method, and auth guard it had before the split, and each handler
//! forwards the request to the owning Worker (`proxy::forward`) and
//! returns its response verbatim.
//!
//! The tables below were extracted from the moved route files
//! (`routes_p7.rs` / `routes_p8.rs` / `routes_p9.rs` /
//! `routes_seo.rs`) — do not
//! hand-edit a guard: if a guard changes in a sub-worker, change it
//! here too, and vice versa.
//! The sub-worker enforces the same guards itself from the forwarded
//! session cookie, so auth is checked twice and can never drift.
//!
//! Social: 43 routes. WooCommerce: 47 routes. School: 42 routes.
//! SEO: 31 routes.

use wordless_kernel::envelope::RouteResponse;
use wordless_kernel::router::{HandlerResult, RouteOptions, Router};

use crate::context::Ctx;
use crate::proxy::forward;

/// One moved route: (method, path, require_auth, capability).
/// `capability` implies `require_auth` in the router (an anonymous
/// caller fails the capability check) — the values below reproduce
/// the moved files exactly, including `require_auth: false` on
/// capability-guarded routes.
type ProxyRoute = (&'static str, &'static str, bool, Option<&'static str>);

/// P7 social routes (moved to `wordless-social-worker`).
const SOCIAL_ROUTES: &[ProxyRoute] = &[
    ("GET", "/api/social/profile", true, None),
    ("PUT", "/api/social/profile", true, None),
    ("GET", "/api/social/profile/:id", true, None),
    ("GET", "/api/social/members", true, None),
    ("GET", "/api/social/friends", true, None),
    ("POST", "/api/social/friends/request", true, None),
    ("POST", "/api/social/friends/accept", true, None),
    ("POST", "/api/social/friends/decline", true, None),
    ("POST", "/api/social/friends/remove", true, None),
    ("POST", "/api/social/friends/block", true, None),
    ("POST", "/api/social/friends/unblock", true, None),
    ("GET", "/api/social/friends/blocked", true, None),
    ("GET", "/api/social/activity", true, None),
    ("POST", "/api/social/activity", true, None),
    ("POST", "/api/social/activity/:id/like", true, None),
    ("DELETE", "/api/social/activity/:id/like", true, None),
    ("DELETE", "/api/social/activity/:id", true, None),
    (
        "POST",
        "/api/social/activity/:id/hide",
        true,
        Some("moderate_comments"),
    ),
    (
        "GET",
        "/api/social/admin/activity",
        true,
        Some("manage_options"),
    ),
    ("GET", "/api/social/groups", true, None),
    ("POST", "/api/social/groups", true, None),
    ("GET", "/api/social/groups/:id", true, None),
    ("PUT", "/api/social/groups/:id", true, None),
    ("POST", "/api/social/groups/:id/join", true, None),
    ("POST", "/api/social/groups/:id/leave", true, None),
    ("POST", "/api/social/groups/:id/approve", true, None),
    ("POST", "/api/social/groups/:id/invite", true, None),
    ("POST", "/api/social/groups/:id/ban", true, None),
    ("GET", "/api/social/groups/:id/members", true, None),
    ("GET", "/api/social/groups/:id/activity", true, None),
    ("GET", "/api/social/messages/threads", true, None),
    ("POST", "/api/social/messages/threads", true, None),
    ("GET", "/api/social/messages/threads/:id", true, None),
    (
        "POST",
        "/api/social/messages/threads/:id/messages",
        true,
        None,
    ),
    ("POST", "/api/social/messages/threads/:id/read", true, None),
    ("GET", "/api/social/notifications", true, None),
    ("GET", "/api/social/notifications/unread-count", true, None),
    ("POST", "/api/social/notifications/read", true, None),
    ("GET", "/members", false, None),
    ("GET", "/members/:id", false, None),
    ("GET", "/groups", false, None),
    ("GET", "/groups/:slug", false, None),
    ("GET", "/activity", false, None),
];

/// P8 WooCommerce routes (moved to `wordless-woocommerce-worker`).
const WOO_ROUTES: &[ProxyRoute] = &[
    ("GET", "/api/woo/products", false, Some("manage_options")),
    (
        "GET",
        "/api/woo/products/sku/:sku",
        false,
        Some("manage_options"),
    ),
    ("GET", "/api/woo/products/:id", false, None),
    ("POST", "/api/woo/products", false, Some("manage_options")),
    (
        "PUT",
        "/api/woo/products/:id",
        false,
        Some("manage_options"),
    ),
    (
        "DELETE",
        "/api/woo/products/:id",
        false,
        Some("manage_options"),
    ),
    (
        "GET",
        "/api/woo/products/:id/attributes",
        false,
        Some("manage_options"),
    ),
    (
        "POST",
        "/api/woo/products/:id/attributes",
        false,
        Some("manage_options"),
    ),
    (
        "PUT",
        "/api/woo/products/:id/attributes/:attrId",
        false,
        Some("manage_options"),
    ),
    (
        "DELETE",
        "/api/woo/products/:id/attributes/:attrId",
        false,
        Some("manage_options"),
    ),
    ("GET", "/api/woo/products/:id/variations", false, None),
    (
        "GET",
        "/api/woo/products/:id/variations/:varId",
        false,
        None,
    ),
    (
        "POST",
        "/api/woo/products/:id/variations",
        false,
        Some("manage_options"),
    ),
    (
        "PUT",
        "/api/woo/products/:id/variations/:varId",
        false,
        Some("manage_options"),
    ),
    (
        "DELETE",
        "/api/woo/products/:id/variations/:varId",
        false,
        Some("manage_options"),
    ),
    ("GET", "/api/woo/cart", false, None),
    ("POST", "/api/woo/cart", false, None),
    ("PUT", "/api/woo/cart", false, None),
    ("DELETE", "/api/woo/cart", false, None),
    ("POST", "/api/woo/quote", false, None),
    ("POST", "/api/woo/checkout", false, None),
    ("POST", "/api/woo/coupons/validate", false, None),
    ("GET", "/api/woo/coupons", false, Some("manage_options")),
    ("POST", "/api/woo/coupons", false, Some("manage_options")),
    ("GET", "/api/woo/coupons/:id", false, Some("manage_options")),
    ("PUT", "/api/woo/coupons/:id", false, Some("manage_options")),
    (
        "DELETE",
        "/api/woo/coupons/:id",
        false,
        Some("manage_options"),
    ),
    ("GET", "/api/woo/tax-rates", false, Some("manage_options")),
    ("POST", "/api/woo/tax-rates", false, Some("manage_options")),
    (
        "PUT",
        "/api/woo/tax-rates/:id",
        false,
        Some("manage_options"),
    ),
    (
        "DELETE",
        "/api/woo/tax-rates/:id",
        false,
        Some("manage_options"),
    ),
    (
        "GET",
        "/api/woo/tax-settings",
        false,
        Some("manage_options"),
    ),
    (
        "PUT",
        "/api/woo/tax-settings",
        false,
        Some("manage_options"),
    ),
    (
        "GET",
        "/api/woo/shipping-zones",
        false,
        Some("manage_options"),
    ),
    (
        "POST",
        "/api/woo/shipping-zones",
        false,
        Some("manage_options"),
    ),
    (
        "PUT",
        "/api/woo/shipping-zones/:id",
        false,
        Some("manage_options"),
    ),
    (
        "DELETE",
        "/api/woo/shipping-zones/:id",
        false,
        Some("manage_options"),
    ),
    (
        "GET",
        "/api/woo/shipping-zones/:id/methods",
        false,
        Some("manage_options"),
    ),
    (
        "POST",
        "/api/woo/shipping-zones/:id/methods",
        false,
        Some("manage_options"),
    ),
    (
        "PUT",
        "/api/woo/shipping-methods/:id",
        false,
        Some("manage_options"),
    ),
    (
        "DELETE",
        "/api/woo/shipping-methods/:id",
        false,
        Some("manage_options"),
    ),
    ("POST", "/api/woo/shipping/quote", false, None),
    ("GET", "/api/woo/customers", false, Some("manage_options")),
    (
        "GET",
        "/api/woo/customers/:id",
        false,
        Some("manage_options"),
    ),
    ("POST", "/api/woo/import", false, Some("manage_options")),
    (
        "GET",
        "/api/woo/variations/sku/:sku",
        false,
        Some("manage_options"),
    ),
    (
        "GET",
        "/api/woo/coupons/code/:code",
        false,
        Some("manage_options"),
    ),
];

/// P9 school routes (moved to `wordless-school-worker`).
const SCHOOL_ROUTES: &[ProxyRoute] = &[
    ("GET", "/api/school/courses", false, None),
    ("POST", "/api/school/courses", false, Some("edit_posts")),
    ("GET", "/api/school/courses/:id", false, None),
    ("PUT", "/api/school/courses/:id", false, Some("edit_posts")),
    (
        "DELETE",
        "/api/school/courses/:id",
        false,
        Some("edit_posts"),
    ),
    ("POST", "/api/school/courses/:id/enrol", true, None),
    ("DELETE", "/api/school/courses/:id/enrol", true, None),
    (
        "GET",
        "/api/school/courses/:id/enrolments",
        false,
        Some("edit_posts"),
    ),
    ("GET", "/api/school/courses/:id/progress", true, None),
    ("GET", "/api/school/courses/:id/lessons", false, None),
    (
        "POST",
        "/api/school/courses/:id/lessons",
        false,
        Some("edit_posts"),
    ),
    ("PUT", "/api/school/lessons/:id", false, Some("edit_posts")),
    (
        "DELETE",
        "/api/school/lessons/:id",
        false,
        Some("edit_posts"),
    ),
    ("POST", "/api/school/lessons/:id/complete", true, None),
    (
        "GET",
        "/api/school/courses/:id/banks",
        false,
        Some("edit_posts"),
    ),
    (
        "POST",
        "/api/school/courses/:id/banks",
        false,
        Some("edit_posts"),
    ),
    ("DELETE", "/api/school/banks/:id", false, Some("edit_posts")),
    (
        "GET",
        "/api/school/banks/:id/questions",
        false,
        Some("edit_posts"),
    ),
    (
        "POST",
        "/api/school/banks/:id/questions",
        false,
        Some("edit_posts"),
    ),
    (
        "PUT",
        "/api/school/questions/:id",
        false,
        Some("edit_posts"),
    ),
    (
        "DELETE",
        "/api/school/questions/:id",
        false,
        Some("edit_posts"),
    ),
    ("GET", "/api/school/courses/:id/exams", false, None),
    (
        "POST",
        "/api/school/courses/:id/exams",
        false,
        Some("edit_posts"),
    ),
    ("GET", "/api/school/exams/:id", false, None),
    ("PUT", "/api/school/exams/:id", false, Some("edit_posts")),
    ("DELETE", "/api/school/exams/:id", false, Some("edit_posts")),
    ("POST", "/api/school/exams/:id/start", true, None),
    (
        "GET",
        "/api/school/exams/:id/attempts",
        false,
        Some("edit_posts"),
    ),
    ("GET", "/api/school/exams/:id/leaderboard", false, None),
    ("GET", "/api/school/attempts/:id", true, None),
    ("POST", "/api/school/attempts/:id/answers", true, None),
    ("POST", "/api/school/attempts/:id/submit", true, None),
    ("GET", "/api/school/attempts/:id/result", true, None),
    ("GET", "/api/school/results", true, None),
    ("GET", "/api/school/results/:id", true, None),
    (
        "GET",
        "/api/school/grading-queue",
        false,
        Some("edit_posts"),
    ),
    (
        "POST",
        "/api/school/answers/:id/grade",
        false,
        Some("edit_posts"),
    ),
    ("GET", "/api/school/certificates", true, None),
    ("GET", "/api/school/certificates/:code", false, None),
    ("GET", "/school", false, None),
    ("GET", "/school/courses/:slug", false, None),
    ("GET", "/school/certificates/:code", false, None),
];

/// P10 SEO routes (moved to `wordless-seo-worker`).
const SEO_ROUTES: &[ProxyRoute] = &[
    ("GET", "/api/posts/:id/seo", false, Some("edit_posts")),
    ("PUT", "/api/posts/:id/seo", false, Some("edit_posts")),
    ("GET", "/api/posts/:id/seo/analysis", false, Some("edit_posts")),
    ("GET", "/api/seo/robots", false, Some("manage_options")),
    ("PUT", "/api/seo/robots", false, Some("manage_options")),
    ("GET", "/api/posts/:id/seo/json-ld", false, Some("edit_posts")),
    ("GET", "/api/posts/:id/seo/faq", false, Some("edit_posts")),
    ("PUT", "/api/posts/:id/seo/faq", false, Some("edit_posts")),
    ("GET", "/api/seo/cwv-budgets", false, Some("manage_options")),
    ("PUT", "/api/seo/cwv-budgets", false, Some("manage_options")),
    ("GET", "/api/seo/audit-runs", false, Some("manage_options")),
    ("POST", "/api/seo/audit-runs", false, Some("manage_options")),
    ("GET", "/api/seo/audit-runs/:id", false, Some("manage_options")),
    ("GET", "/api/seo/sitemap", false, Some("manage_options")),
    ("GET", "/api/seo/templates", false, Some("edit_posts")),
    ("POST", "/api/seo/templates/:slug/create", false, Some("edit_posts")),
    ("GET", "/api/seo/calendar", true, None),
    ("POST", "/api/seo/import", false, Some("manage_options")),
    ("GET", "/api/seo/redirects", false, Some("manage_options")),
    ("POST", "/api/seo/redirects", false, Some("manage_options")),
    ("PUT", "/api/seo/redirects/:id", false, Some("manage_options")),
    ("DELETE", "/api/seo/redirects/:id", false, Some("manage_options")),
    ("GET", "/api/seo/404s", false, Some("manage_options")),
    ("DELETE", "/api/seo/404s", false, Some("manage_options")),
    ("GET", "/sitemap.xml", false, None),
    ("GET", "/sitemap-index.xml", false, None),
    ("GET", "/sitemap-images.xml", false, None),
    ("GET", "/sitemap-videos.xml", false, None),
    ("GET", "/robots.txt", false, None),
    ("GET", "/llms.txt", false, None),
    ("GET", "/llms-full.txt", false, None),
];

/// The social Worker is not configured (no `WORDLESS_SOCIAL_URL`
/// var, no `SOCIAL` service binding): fail honestly, never silently.
async fn proxy_to_social(ctx: &mut Ctx) -> HandlerResult {
    match &ctx.social {
        Some(target) => Ok(forward(target, ctx).await),
        None => Ok(RouteResponse::json_error(
            "internal_error",
            503,
            "Social service is not configured.",
        )),
    }
}

/// The WooCommerce Worker is not configured (no `WORDLESS_WOO_URL`
/// var, no `WOO` service binding): fail honestly, never silently.
async fn proxy_to_woo(ctx: &mut Ctx) -> HandlerResult {
    match &ctx.woo {
        Some(target) => Ok(forward(target, ctx).await),
        None => Ok(RouteResponse::json_error(
            "internal_error",
            503,
            "WooCommerce service is not configured.",
        )),
    }
}

/// The school Worker is not configured (no `WORDLESS_SCHOOL_URL`
/// var, no `SCHOOL` service binding): fail honestly, never silently.
async fn proxy_to_school(ctx: &mut Ctx) -> HandlerResult {
    match &ctx.school {
        Some(target) => Ok(forward(target, ctx).await),
        None => Ok(RouteResponse::json_error(
            "internal_error",
            503,
            "School service is not configured.",
        )),
    }
}

/// The SEO Worker is not configured (no `WORDLESS_SEO_URL` var, no
/// `SEO` service binding): fail honestly, never silently.
async fn proxy_to_seo(ctx: &mut Ctx) -> HandlerResult {
    match &ctx.seo {
        Some(target) => {
            let mut resp = forward(target, ctx).await;
            // Drain the purge side-channel: the SEO Worker reports
            // cache-purge URLs (redirect creates/updates) as
            // `X-Wordless-Purge` response headers; they belong in
            // this Worker's purge list (purged after routing, see
            // lib.rs), never in the client response.
            let mut i = 0;
            while i < resp.extra_headers.len() {
                if resp.extra_headers[i]
                    .0
                    .eq_ignore_ascii_case("X-Wordless-Purge")
                {
                    let (_, url) = resp.extra_headers.remove(i);
                    ctx.purge_urls.push(url);
                } else {
                    i += 1;
                }
            }
            Ok(resp)
        }
        None => Ok(RouteResponse::json_error(
            "internal_error",
            503,
            "SEO service is not configured.",
        )),
    }
}

/// Register every moved route on the router built by `routes.rs`.
pub fn register_proxy_routes(router: &mut Router<Ctx>) {
    for (method, path, require_auth, capability) in SOCIAL_ROUTES {
        router.add(
            method,
            path,
            |ctx| Box::pin(async move { proxy_to_social(ctx).await }),
            RouteOptions {
                require_auth: *require_auth,
                capability: *capability,
            },
        );
    }
    for (method, path, require_auth, capability) in WOO_ROUTES {
        router.add(
            method,
            path,
            |ctx| Box::pin(async move { proxy_to_woo(ctx).await }),
            RouteOptions {
                require_auth: *require_auth,
                capability: *capability,
            },
        );
    }
    for (method, path, require_auth, capability) in SCHOOL_ROUTES {
        router.add(
            method,
            path,
            |ctx| Box::pin(async move { proxy_to_school(ctx).await }),
            RouteOptions {
                require_auth: *require_auth,
                capability: *capability,
            },
        );
    }
    for (method, path, require_auth, capability) in SEO_ROUTES {
        router.add(
            method,
            path,
            |ctx| Box::pin(async move { proxy_to_seo(ctx).await }),
            RouteOptions {
                require_auth: *require_auth,
                capability: *capability,
            },
        );
    }
}
