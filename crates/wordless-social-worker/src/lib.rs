//! WORDLESS v2 social Worker entry (size remedy, 2026-11-20).
//!
//! The `#[event(fetch)]` entrypoint: builds the request context
//! (session user first, exactly like the main Worker's `fetch()`),
//! serves the social route table, and puts the security headers on
//! every response.
//!
//! Internal service — the endpoints are NOT part of the public API
//! surface on their own: the main Worker proxies `/api/social/*` and
//! the public `/members`, `/groups`, `/activity` pages here (service
//! binding in production, plain HTTP in local dev). Reachability is
//! the access control; auth is enforced by the router from the
//! session cookie, exactly like the main Worker.
//!
//! Endpoints:
//! - `GET /__social/health` → `{ "ok": true, "service":
//!   "wordless-social", "version" }` (readiness probe for
//!   `scripts/dev-split.sh` and the smoke scripts).
//! - every P7 route from `routes_social.rs` (same paths, methods,
//!   guards, envelope as before the split).

pub mod ctx;
pub mod helpers;
pub mod routes_social;

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

/// Read one optional secret (empty/missing → `None`). Secrets are
/// never logged.
fn opt_secret(env: &Env, name: &str) -> Option<String> {
    env.secret(name)
        .ok()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            env.var(name)
                .ok()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
        })
}

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

    if req.method() == worker::Method::Get && path == "/__social/health" {
        let body = json!({
            "ok": true,
            "service": "wordless-social",
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
    let int_secrets = wordless_integrations::EnvSecrets {
        email_api_key: opt_secret(&env, "EMAIL_API_KEY"),
        paystack_secret_key: opt_secret(&env, "PAYSTACK_SECRET_KEY"),
        flutterwave_secret_key: opt_secret(&env, "FLUTTERWAVE_SECRET_KEY"),
        stripe_secret_key: opt_secret(&env, "STRIPE_SECRET_KEY"),
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

    let mut ctx = Ctx {
        method: req.method().as_ref().to_string(),
        path,
        query,
        headers,
        body,
        params: HashMap::new(),
        user,
        db,
        int_secrets,
    };

    let mut router: Router<Ctx> = Router::new();
    routes_social::register_social_routes(&mut router);
    let resp = router.handle(&mut ctx).await;
    to_worker_response(resp)
}
