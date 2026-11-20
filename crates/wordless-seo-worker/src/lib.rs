//! WORDLESS v2 SEO Worker entry (2026-11-20 SEO split).
//!
//! The `#[event(fetch)]` entrypoint: builds the request context
//! (session user first, exactly like the main Worker's `fetch()`),
//! serves the SEO route table, and puts the security headers on
//! every response. No cron trigger: SEO has no sweep.
//!
//! Internal service — the endpoints are NOT part of the public API
//! surface on their own: the main Worker proxies `/api/posts/:id/seo*`,
//! `/api/seo/*` and the public sitemap/robots/llms routes here
//! (service binding in production, plain HTTP in local dev).
//! Reachability is the access control; auth is enforced by the
//! router from the session cookie, exactly like the main Worker.
//!
//! Endpoints:
//! - `GET /__seo/health` → `{ "ok": true, "service":
//!   "wordless-seo", "version" }` (readiness probe for
//!   `scripts/dev-split.sh` and the smoke scripts).
//! - every SEO route from `routes_seo.rs` (same paths, methods,
//!   guards, envelope as before the split).
//!
//! `origin`: the main Worker's origin, forwarded by the proxy as
//! the `X-Wordless-Origin` header — the redirect create/update
//! handlers build cache-purge URLs from it, and the cached entries
//! live under the main Worker's origin. Falls back to this
//! request's own URL origin for direct calls in local dev.

pub mod ctx;
pub mod helpers;
pub mod routes_seo;

use std::collections::HashMap;
use std::rc::Rc;

use serde_json::json;
use wordless_auth::{parse_session_cookie, user_from_session};
use wordless_db::d1::D1Db;
use wordless_db::Db;
use wordless_kernel::envelope::RouteResponse;
use wordless_kernel::router::Router;
use wordless_kernel::security::security_headers;
use worker::{event, Context, Env, Request, Response, Result};

use ctx::Ctx;

/// Convert a platform-neutral `RouteResponse` into a `worker::Response`,
/// adding the security headers (same rule as the main Worker: v1
/// applies them to EVERY response).
fn to_worker_response(resp: RouteResponse) -> Result<Response> {
    let mut response = if resp.body.is_empty() && resp.content_type.is_empty() {
        Response::empty()?.with_status(resp.status)
    } else {
        Response::from_bytes(resp.body)?.with_status(resp.status)
    };
    if !resp.content_type.is_empty() {
        response
            .headers_mut()
            .set("Content-Type", &resp.content_type)?;
    }
    for (name, value) in &resp.extra_headers {
        response.headers_mut().set(name, value)?;
    }
    for (name, value) in security_headers() {
        response.headers_mut().set(&name, &value)?;
    }
    Ok(response)
}

#[event(fetch)]
pub async fn main(mut req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let path = url.path().to_string();

    if req.method() == worker::Method::Get && path == "/__seo/health" {
        let body = json!({
            "ok": true,
            "service": "wordless-seo",
            "version": env!("CARGO_PKG_VERSION"),
        });
        let mut resp = Response::from_json(&body)?;
        resp.headers_mut().set("Content-Type", "application/json")?;
        return Ok(resp);
    }

    // Bindings: DB is required. This Worker shares the main Worker's
    // D1 database (same database_id); the main Worker runs the
    // migrations — this Worker never applies them.
    let db = match env.d1("DB") {
        Ok(d) => Rc::new(D1Db::new(d)) as Rc<dyn Db>,
        Err(e) => {
            worker::console_error!("D1 binding missing: {e}");
            let resp = RouteResponse::json_error(
                "internal_error",
                500,
                "Database binding is not configured.",
            );
            return to_worker_response(resp);
        }
    };

    // Session user first, exactly like the main Worker's fetch().
    let cookie = req.headers().get("Cookie").ok().flatten();
    let token = parse_session_cookie(cookie.as_deref());
    let user = match user_from_session(db.as_ref(), token.as_deref()).await {
        Ok(u) => u.map(|u| u.to_auth_user()),
        Err(e) => {
            worker::console_error!("session lookup failed: {e}");
            None
        }
    };

    let mut query = HashMap::new();
    for (k, v) in url.query_pairs() {
        query
            .entry(k.into_owned())
            .or_insert_with(|| v.into_owned());
    }
    let mut headers = HashMap::new();
    for (name, value) in req.headers().entries() {
        headers.entry(name.to_ascii_lowercase()).or_insert(value);
    }
    let body = if req.method() == worker::Method::Post
        || req.method() == worker::Method::Put
        || req.method() == worker::Method::Patch
        || req.method() == worker::Method::Delete
    {
        req.bytes().await.unwrap_or_default()
    } else {
        Vec::new()
    };

    // Origin: the main Worker's origin when proxied (the proxy sets
    // `X-Wordless-Origin` — see the main Worker's `proxy.rs`), else
    // this request's own origin (direct calls in local dev).
    let origin = headers
        .get("x-wordless-origin")
        .cloned()
        .unwrap_or_else(|| url.origin().ascii_serialization());

    // P12: the site comes from the `X-Wordless-Site` header the main
    // Worker forwards (it already resolved and validated the site).
    // Validate again here so a direct call to this internal Worker
    // cannot spoof another site's sitemap with a forged header.
    let site_id = match headers
        .get("x-wordless-site")
        .and_then(|v| v.parse::<i64>().ok())
    {
        Some(id) if id >= 1 => {
            match wordless_multisite::get_site_by_id(db.as_ref(), id).await {
                Ok(Some(site)) if site.status == "public" || site.status == "archived" => id,
                _ => 1,
            }
        }
        _ => 1,
    };

    let mut ctx = Ctx {
        method: req.method().as_ref().to_string(),
        path,
        query,
        headers,
        body,
        params: HashMap::new(),
        user,
        db,
        origin,
        site_id,
    };

    let mut router: Router<Ctx> = Router::new();
    routes_seo::register_seo_routes(&mut router);
    routes_seo::register_seo_public_routes(&mut router);
    let resp = router.handle(&mut ctx).await;
    to_worker_response(resp)
}
