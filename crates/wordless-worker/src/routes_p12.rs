//! P12 route table — Multisite (v2-only routes, no v1 counterpart):
//! the sites API. One install, many sites: shared users and code,
//! per-site content/settings/themes.
//!
//! Registered from `routes::build_router` after the P11 table.
//!
//! All routes are administrator-only (`manage_options`): creating or
//! deleting a site wipes content, and per-site options are the
//! network's configuration surface. The site a request acts on is
//! resolved by the worker entry (`lib.rs`): the `X-Wordless-Site`
//! header for authenticated editors, else path prefix / subdomain /
//! site 1. These routes administer sites; they do not switch the
//! caller's context — the admin SPA sends the header itself.

use serde_json::{json, Value};
use wordless_kernel::error::{bad_request, not_found};
use wordless_kernel::router::{read_json_body, RequestContext, RouteOptions, Router};
use wordless_multisite::{
    create_site, delete_site, delete_site_option, get_site_by_id, get_site_option,
    is_site_scoped_key, list_sites, set_site_option, update_site,
};

use crate::context::Ctx;
use crate::routes::{ok, ok_json, opt_str, parse_id};

/// Register every P12 route on the router built by `routes.rs`.
pub fn register_p12_routes(router: &mut Router<Ctx>) {
    // ------------------------------------------------------- current site
    // Registered BEFORE `/api/sites/:id` so "current" is never parsed
    // as an id. Returns the site the caller's request resolved to.
    router.get(
        "/api/sites/current",
        |ctx| {
            Box::pin(async move {
                let site = get_site_by_id(ctx.db.as_ref(), ctx.site_id)
                    .await?
                    .ok_or_else(|| bad_request("Current site not found."))?;
                ok(site, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: Some("manage_options"),
        },
    );

    // ------------------------------------------------------- list sites
    router.get(
        "/api/sites",
        |ctx| {
            Box::pin(async move {
                let sites = list_sites(ctx.db.as_ref()).await?;
                ok(sites, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: Some("manage_options"),
        },
    );

    // ------------------------------------------------------- create site
    router.post(
        "/api/sites",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let slug = body
                    .get("slug")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let title = body
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let site = create_site(ctx.db.as_ref(), &slug, &title).await?;
                ok(site, 201)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: Some("manage_options"),
        },
    );

    // ------------------------------------------------------- one site
    router.get(
        "/api/sites/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "site id")?;
                let site = get_site_by_id(ctx.db.as_ref(), id)
                    .await?
                    .ok_or_else(|| not_found("Site not found."))?;
                ok(site, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: Some("manage_options"),
        },
    );

    // ------------------------------------------------------- update site
    router.put(
        "/api/sites/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "site id")?;
                let body = read_json_body(ctx)?;
                let site = update_site(
                    ctx.db.as_ref(),
                    id,
                    opt_str(&body, "title").as_deref(),
                    opt_str(&body, "status").as_deref(),
                )
                .await?;
                ok(site, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: Some("manage_options"),
        },
    );

    // ------------------------------------------------------- delete site
    // Wipes ALL of the site's content rows. The crate refuses site 1.
    router.delete(
        "/api/sites/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "site id")?;
                let deleted_posts = delete_site(ctx.db.as_ref(), id).await?;
                ok_json(json!({ "deleted": true, "deletedPosts": deleted_posts }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: Some("manage_options"),
        },
    );

    // ------------------------------------------------------- site options
    // Read one per-site option (falls back to the network default when
    // the site has no override). Any key reads; writes are restricted
    // to site-scoped keys below.
    router.get(
        "/api/sites/:id/options/:key",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "site id")?;
                let key = ctx.param("key").unwrap_or("").to_string();
                let value = get_site_option::<Value>(
                    ctx.db.as_ref(),
                    id,
                    &key,
                    Value::Null,
                )
                .await?;
                ok_json(json!({ "key": key, "value": value }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: Some("manage_options"),
        },
    );

    router.put(
        "/api/sites/:id/options/:key",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "site id")?;
                let key = ctx.param("key").unwrap_or("").to_string();
                if !is_site_scoped_key(&key) {
                    return Err(bad_request(&format!(
                        "Option \"{key}\" is not a per-site option."
                    )));
                }
                let body = read_json_body(ctx)?;
                let Some(value) = body.get("value").cloned() else {
                    return Err(bad_request("Request body must include a \"value\"."));
                };
                set_site_option(ctx.db.as_ref(), id, &key, &value).await?;
                ok_json(json!({ "key": key, "value": value }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: Some("manage_options"),
        },
    );

    router.delete(
        "/api/sites/:id/options/:key",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "site id")?;
                let key = ctx.param("key").unwrap_or("").to_string();
                let deleted = delete_site_option(ctx.db.as_ref(), id, &key).await?;
                ok_json(json!({ "key": key, "deleted": deleted }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: Some("manage_options"),
        },
    );
}
