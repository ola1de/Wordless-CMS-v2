//! WORDLESS v2 school Worker entry (size remedy, 2026-11-20).
//!
//! The `#[event(fetch)]` entrypoint: builds the request context
//! (session user first, exactly like the main Worker's `fetch()`),
//! serves the school route table, and puts the security headers on
//! every response. The `#[event(scheduled)]` handler closes expired
//! exam attempts every 5 minutes (the cron half of the server-side
//! timing mechanism; the lazy close on attempt read is the other
//! half, and the locally verifiable one).
//!
//! Internal service — the endpoints are NOT part of the public API
//! surface on their own: the main Worker proxies `/api/school/*` and
//! the public `/school` pages here (service binding in production,
//! plain HTTP in local dev). Reachability is the access control;
//! auth is enforced by the router from the session cookie, exactly
//! like the main Worker.
//!
//! Endpoints:
//! - `GET /__school/health` → `{ "ok": true, "service":
//!   "wordless-school", "version" }` (readiness probe for
//!   `scripts/dev-split.sh` and the smoke scripts).
//! - every P9 route from `routes_school.rs` (same paths, methods,
//!   guards, envelope as before the split).

pub mod ctx;
pub mod helpers;
pub mod routes_school;

use std::collections::HashMap;
use std::rc::Rc;

use serde_json::json;
use wordless_auth::{parse_session_cookie, user_from_session};
use wordless_db::d1::D1Db;
use wordless_db::Db;
use wordless_kernel::envelope::RouteResponse;
use wordless_kernel::router::Router;
use wordless_kernel::security::security_headers;
use wordless_kernel::time::now_ms;
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

    if req.method() == worker::Method::Get && path == "/__school/health" {
        let body = json!({
            "ok": true,
            "service": "wordless-school",
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

    // P12: site from the forwarded `X-Wordless-Site` header,
    // validated against wl_sites (1 when absent or unknown).
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
        site_id,
    };

    let mut router: Router<Ctx> = Router::new();
    routes_school::register_school_routes(&mut router);
    let resp = router.handle(&mut ctx).await;
    to_worker_response(resp)
}

/// Cron Trigger handler (P9 server-side timing — the deploy-time half
/// of the mechanism; the other half is the lazy close on attempt
/// read). Every 5 minutes (`[triggers] crons` in this Worker's
/// `wrangler.toml`) it closes every expired exam attempt via the
/// same `close_expired_attempts` the lazy path calls. A failure is
/// logged, not retried here — the next tick picks the attempt up
/// again, and the lazy check covers reads in between. Local
/// `wrangler dev` fires no real cron, so this path is unit-tested
/// (the service) and only observed firing on a deployed Worker.
#[event(scheduled)]
pub async fn scheduled(
    _event: worker::ScheduledEvent,
    env: Env,
    _ctx: worker::ScheduleContext,
) -> Result<()> {
    let db = match env.d1("DB") {
        Ok(d) => Rc::new(D1Db::new(d)) as Rc<dyn Db>,
        Err(e) => {
            worker::console_error!("scheduled: D1 binding missing: {e}");
            return Ok(());
        }
    };
    match crate::routes_school::sweep_expired_attempts(db.as_ref(), now_ms()).await {
        Ok(0) => Ok(()),
        Ok(n) => {
            worker::console_log!("scheduled: closed {n} expired exam attempt(s)");
            Ok(())
        }
        Err(e) => {
            worker::console_error!("scheduled: exam sweep failed: {e}");
            Ok(())
        }
    }
}
