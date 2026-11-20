//! Sub-worker proxy (size remedy, 2026-11-20): forwards a request
//! to the social, WooCommerce, school or SEO Worker and returns its
//! response verbatim — same status, same body, same Content-Type,
//! plus the sub-worker's `Set-Cookie` when it sets one (the Woo cart
//! token) and its `X-Wordless-Purge` headers when it sets those (the
//! SEO Worker's redirect cache purges — drained by `proxy_to_seo`
//! into the main Worker's purge list, never shown to the client).
//! The proxy also tells the sub-worker the main Worker's origin
//! (`X-Wordless-Origin` request header): the SEO Worker builds
//! absolute purge URLs from it, and the cached entries live under
//! the main Worker's origin, not the sub-worker's internal address.
//!
//! The main Worker keeps the route registrations (same paths,
//! methods, auth guards — see `routes_proxy.rs`), so the public API
//! does not change; only the handler moves across the service
//! boundary. The sub-worker enforces the same guards itself from the
//! forwarded session cookie, so auth is checked twice and can never
//! drift.
//!
//! Transport (the P6 extension-resolver pattern, same as the
//! converter client):
//! 1. `WORDLESS_SOCIAL_URL` / `WORDLESS_WOO_URL` / `WORDLESS_SCHOOL_URL` /
//!    `WORDLESS_SEO_URL` — plain HTTP to a locally running sub-worker
//!    (local dev / smoke tests).
//! 2. Service binding `SOCIAL` / `WOO` / `SCHOOL` / `SEO` — the free production path.
//! Neither → the proxy answers 503 with a clear message (never a
//! silent empty response).

use worker::{Fetch, Fetcher, Headers, Method, Request, RequestInit};

use wordless_kernel::envelope::RouteResponse;

use crate::context::Ctx;

/// One proxy transport: a service-binding `Fetcher` (production) or
/// a plain-HTTP base URL (local dev).
pub enum ProxyTarget {
    /// Service binding; requests go to the fake internal origin (the
    /// binding, not the URL, decides the destination).
    Fetcher(Fetcher, &'static str),
    /// Local-dev base URL, e.g. `http://127.0.0.1:8807`.
    Url(String),
}

impl ProxyTarget {
    /// Service-binding transport (production). `internal_host` is the
    /// fake origin the request is addressed to (the binding decides
    /// the real destination).
    pub fn via_service(fetcher: Fetcher, internal_host: &'static str) -> Self {
        Self::Fetcher(fetcher, internal_host)
    }

    /// Plain-HTTP transport (local dev).
    pub fn via_url(base_url: String) -> Self {
        Self::Url(base_url.trim_end_matches('/').to_string())
    }

    fn url_for(&self, path: &str, query_string: &str) -> String {
        let query = if query_string.is_empty() {
            String::new()
        } else {
            format!("?{query_string}")
        };
        match self {
            ProxyTarget::Fetcher(_, host) => format!("https://{host}{path}{query}"),
            ProxyTarget::Url(base) => format!("{base}{path}{query}"),
        }
    }
}

/// Forward this request to the sub-worker and return its response
/// verbatim. A transport failure (unreachable, non-HTTP answer) is a
/// 500 with a clear message; an application answer from the
/// sub-worker — including its 401/403/404 envelopes — passes through
/// untouched, so callers see exactly what the sub-worker said.
pub async fn forward(target: &ProxyTarget, ctx: &Ctx) -> RouteResponse {
    let url = target.url_for(&ctx.path, &ctx.query_string);
    let method = match ctx.method.as_str() {
        "GET" => Method::Get,
        "POST" => Method::Post,
        "PUT" => Method::Put,
        "DELETE" => Method::Delete,
        "PATCH" => Method::Patch,
        _ => Method::Get,
    };
    let has_body = matches!(
        method,
        Method::Post | Method::Put | Method::Patch | Method::Delete
    );
    let mut init = RequestInit::new();
    init.with_method(method);
    let headers = Headers::new();
    for (name, value) in &ctx.headers {
        // `Host` and `Content-Length` belong to this hop, not the next.
        if name == "host" || name == "content-length" || name == "connection" {
            continue;
        }
        let _ = headers.set(name, value);
    }
    // The sub-worker needs the main Worker's origin for absolute
    // URLs it builds (today: the SEO Worker's redirect purge URLs —
    // the cached entries live under this origin, not the
    // sub-worker's internal address). Harmless to other sub-workers:
    // they ignore unknown headers.
    let _ = headers.set("X-Wordless-Origin", &ctx.origin);
    init.with_headers(headers);
    if !ctx.body.is_empty() && has_body {
        init.with_body(Some(wasm_bindgen::JsValue::from(js_sys::Uint8Array::from(
            ctx.body.as_slice(),
        ))));
    }
    let req = match Request::new_with_init(&url, &init) {
        Ok(req) => req,
        Err(e) => {
            return RouteResponse::json_error(
                "internal_error",
                500,
                &format!("Failed to build proxied request: {e}"),
            );
        }
    };
    let mut resp = match target {
        ProxyTarget::Fetcher(fetcher, _) => match fetcher.fetch_request(req).await {
            Ok(resp) => resp,
            Err(e) => {
                return RouteResponse::json_error(
                    "internal_error",
                    500,
                    &format!("Sub-worker service fetch failed: {e}"),
                );
            }
        },
        ProxyTarget::Url(_) => match Fetch::Request(req).send().await {
            Ok(resp) => resp,
            Err(e) => {
                return RouteResponse::json_error(
                    "internal_error",
                    500,
                    &format!("Sub-worker service fetch failed: {e}"),
                );
            }
        },
    };
    let status = resp.status_code();
    let content_type = resp
        .headers()
        .get("Content-Type")
        .ok()
        .flatten()
        .unwrap_or_default();
    // The sub-worker may set response headers the caller depends on —
    // today that is exactly two: the Woo Worker's cart-token
    // `Set-Cookie` (routes_woo.rs). Without it the client never learns
    // the cart token, so every later cart request starts a new empty
    // cart (caught by smoke-p8 on the split build: quote/checkout
    // 400s after a 200 cart add). Forward it; anything else the
    // sub-workers set in future (e.g. a redirect Location) must be
    // added here deliberately, not blindly — hop-by-hop headers must
    // never leak across the hop.
    let mut extra_headers = Vec::new();
    if let Ok(Some(cookie)) = resp.headers().get("Set-Cookie") {
        extra_headers.push(("Set-Cookie".to_string(), cookie));
    }
    // The SEO Worker reports cache-purge URLs as `X-Wordless-Purge`
    // (redirect creates/updates set exactly one). The caller
    // (`proxy_to_seo` in routes_proxy.rs) drains it into the main
    // Worker's purge list; it must never reach the client. Forwarded
    // here deliberately, like `Set-Cookie` above.
    if let Ok(Some(purge)) = resp.headers().get("X-Wordless-Purge") {
        extra_headers.push(("X-Wordless-Purge".to_string(), purge));
    }
    let body = resp.bytes().await.unwrap_or_default();
    RouteResponse {
        status,
        body,
        content_type,
        extra_headers,
    }
}
