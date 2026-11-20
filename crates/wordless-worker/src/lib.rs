//! WORDLESS v2 worker entry.
//!
//! The `#[event(fetch)]` entrypoint: builds the request context
//! (session user first, exactly like v1's `fetch()`), serves `/admin`
//! from the ASSETS binding, routes everything else through the route
//! table, and puts the security headers on every response.
//!
//! The P2 route table is in `routes.rs` (all 104 P2 routes) and the
//! P3 table in `routes_p3.rs` (conversion, AI, commerce, integrations,
//! marketplace, security, backup, importer, GraphQL, outbound
//! webhooks, shop); public rendering falls back through
//! `handle_public_fallback` for paths the router does not match, and
//! anonymous public GETs go through the Cache API (60 s, v1 rules).

pub mod context;
pub mod converter_client;
pub mod health;
pub mod proxy;
pub mod routes;
pub mod routes_p3;
pub mod routes_p6;
pub mod routes_p11;
pub mod routes_p12;
pub mod routes_proxy;
pub mod woo_client;

use std::collections::HashMap;
use std::rc::Rc;

use wordless_auth::{parse_session_cookie, user_from_session};
use wordless_db::d1::D1Db;
use wordless_db::Db;
use wordless_kernel::envelope::RouteResponse;
use wordless_kernel::security::security_headers;
use wordless_render::cache::{is_cacheable_public_request, public_cache_control};
use wordless_seo_core::not_found::log_not_found;
use worker::{event, Cache, Context, Env, Request, Response, Result};

use context::{Bindings, Ctx};
use routes::{build_router, handle_public_fallback};
use wordless_cms::scheduling::publish_due_posts;
use wordless_gateways::WorkerHttpPost;
use wordless_kernel::time::now_ms;

/// Build the P3 AI environment from the Worker env (wasm32 only —
/// the platform backends are `#[cfg(target_arch = "wasm32")]` in
/// `wordless-ai`). Workers AI via the native `worker::Ai` binding;
/// OpenAI-compatible via `OPENAI_API_KEY` (+ optional
/// `OPENAI_BASE_URL` / `OPENAI_MODEL` vars) over Workers fetch.
/// Absent binding/key → the provider reports "not configured".
#[cfg(target_arch = "wasm32")]
fn build_ai_env(env: &Env) -> wordless_ai::AiEnv {
    use std::rc::Rc;
    use wordless_ai::platform::{WorkerAiBackend, WorkerFetchHttp};
    let mut ai_env = wordless_ai::AiEnv::empty();
    if let Ok(ai) = env.ai("AI") {
        ai_env = ai_env.with_workers_ai(Rc::new(WorkerAiBackend::new(ai)));
    }
    if let Ok(key) = env.secret("OPENAI_API_KEY") {
        let key = key.to_string();
        if !key.is_empty() {
            ai_env = ai_env
                .with_openai_key(key)
                .with_http(Rc::new(WorkerFetchHttp));
        }
    }
    if let Ok(base) = env.var("OPENAI_BASE_URL") {
        ai_env = ai_env.with_openai_base_url(base.to_string());
    }
    if let Ok(model) = env.var("OPENAI_MODEL") {
        ai_env = ai_env.with_openai_model(model.to_string());
    }
    ai_env
}

/// Read one optional secret (empty/missing → `None`). Secrets are
/// never logged.
/// Read a secret from the Worker env, falling back to a plain var of
/// the same name (local `wrangler dev --var` / `.dev.vars` values are
/// vars, not secrets — the fallback keeps local dev working without
/// changing production, where secrets stay secrets). Empty → `None`.
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

/// Split (2026-11-20): build the converter client from the Env
/// (wasm32 only). In order (the P6 resolver pattern):
/// 1. `WORDLESS_CONVERTER_URL` — plain HTTP to a locally running
///    converter Worker (local dev / smoke tests).
/// 2. Service binding `CONVERTER` — the free production path.
/// Neither → `None`: conversion jobs fail with a clear error and
/// public Elementor rendering falls back to block content.
#[cfg(target_arch = "wasm32")]
fn build_converter(env: &Env) -> Option<Rc<dyn wordless_kernel::converter::ConverterClient>> {
    if let Ok(url) = env.var("WORDLESS_CONVERTER_URL") {
        let url = url.to_string();
        if !url.is_empty() {
            return Some(crate::converter_client::WorkerConverterClient::via_url(url));
        }
    }
    if let Ok(fetcher) = env.service("CONVERTER") {
        return Some(crate::converter_client::WorkerConverterClient::via_service(
            fetcher,
        ));
    }
    None
}

/// Size remedy (2026-11-20): build a sub-worker proxy target
/// from the Env (wasm32 only). In order (the converter pattern):
/// 1. `WORDLESS_<NAME>_URL` — plain HTTP to a locally running
///    sub-worker (local dev / smoke tests).
/// 2. Service binding — the free production path.
/// Neither → `None`: proxied routes answer 503 with a clear error.
#[cfg(target_arch = "wasm32")]
fn build_proxy_target(
    env: &Env,
    url_var: &str,
    binding: &str,
    internal_host: &'static str,
) -> Option<crate::proxy::ProxyTarget> {
    if let Ok(url) = env.var(url_var) {
        let url = url.to_string();
        if !url.is_empty() {
            return Some(crate::proxy::ProxyTarget::via_url(url));
        }
    }
    if let Ok(fetcher) = env.service(binding) {
        return Some(crate::proxy::ProxyTarget::via_service(
            fetcher,
            internal_host,
        ));
    }
    None
}

/// Size remedy (2026-11-20): build the WooCommerce Worker client
/// from the Env (wasm32 only). Same transports as the proxy, in the
/// same order; `None` → shop handlers that need Woo data fail with a
/// clear error.
#[cfg(target_arch = "wasm32")]
fn build_woo_client(env: &Env) -> Option<Rc<dyn wordless_kernel::woo_client::WooClient>> {
    if let Ok(url) = env.var("WORDLESS_WOO_URL") {
        let url = url.to_string();
        if !url.is_empty() {
            return Some(crate::woo_client::WorkerWooClient::via_url(url));
        }
    }
    if let Ok(fetcher) = env.service("WOO") {
        return Some(crate::woo_client::WorkerWooClient::via_service(fetcher));
    }
    None
}

/// P6: build the extension resolver from the Env (wasm32 only).
/// Needs the host secret — without it no token can be derived and
/// nothing dispatches. Per instance, in order:
/// 1. `dev_url` — plain HTTP to a locally running extension Worker
///    (local dev / smoke tests).
/// 2. Service binding `EXT_<SLUG>` — the FREE production path: the
///    extension is a normal account-level Worker, bound statically.
///    The publish script adds the binding and redeploys the main
///    Worker when an extension is published.
/// 3. Dispatch namespace `EXT_DISPATCHER` — the dynamic path, but it
///    needs Workers for Platforms ($25/mo, no free tier — see doc §3
///    P6), so it is the design target, not the free-tier path.
#[cfg(target_arch = "wasm32")]
fn build_ext_resolver(env: &Env, secret: Option<String>) -> Option<crate::context::ExtResolver> {
    let secret = secret?;
    let env = env.clone();
    let dispatcher = env.dynamic_dispatcher("EXT_DISPATCHER").ok();
    Some(Rc::new(
        move |inst: &wordless_extensions::ExtensionInstance| {
            let token =
                wordless_extensions::derive_host_token(&secret, &inst.slug, inst.token_epoch);
            if let Some(dev_url) = &inst.dev_url {
                let exec: Rc<dyn wordless_extensions::ExtensionExecutor> =
                    Rc::new(wordless_extensions::HttpExecutor::new(
                        wordless_gateways::WorkerFetchTransport,
                        dev_url.clone(),
                    ));
                return Some((exec, token));
            }
            // Free production path: service binding EXT_<SLUG>
            // (hyphens become underscores, uppercased).
            let binding = format!("EXT_{}", inst.slug.to_uppercase().replace('-', "_"));
            if let Ok(fetcher) = env.service(&binding) {
                let exec: Rc<dyn wordless_extensions::ExtensionExecutor> =
                    Rc::new(wordless_extensions::HttpExecutor::new(
                        wordless_extensions::FetcherTransport::new(fetcher),
                        "https://ext-wordless.internal",
                    ));
                return Some((exec, token));
            }
            let dispatcher = dispatcher.as_ref()?;
            let fetcher = dispatcher.get(&inst.slug).ok()?;
            let exec: Rc<dyn wordless_extensions::ExtensionExecutor> =
                Rc::new(wordless_extensions::HttpExecutor::new(
                    wordless_extensions::FetcherTransport::new(fetcher),
                    "https://ext-wordless.internal",
                ));
            Some((exec, token))
        },
    ))
}

/// Convert a platform-neutral `RouteResponse` into a `worker::Response`,
/// adding the security headers (v1 applies them to EVERY response).
fn to_worker_response(resp: RouteResponse) -> Result<Response> {
    // NOTE (found by the P1 smoke test): `Response::from_bytes` defaults
    // to 200 — the route's status MUST be applied explicitly, or every
    // error envelope (401/404/405/…) comes back as HTTP 200.
    // P5 F1 (second half): `from_bytes(vec![])` makes workerd attach a
    // default `application/octet-stream` Content-Type, but v1's
    // redirects are `new Response(null, ...)` — no body, no content
    // type at all. An empty body with no content type uses
    // `Response::empty()` (a null body), which carries no Content-Type.
    let mut response = if resp.body.is_empty() && resp.content_type.is_empty() {
        Response::empty()?.with_status(resp.status)
    } else {
        Response::from_bytes(resp.body)?.with_status(resp.status)
    };
    // Empty content_type means "no Content-Type header at all" (P5 F1:
    // v1 redirects carry none). Never send a blank header value.
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

/// Serve `/admin` from the ASSETS binding (v1 `serveAdmin`).
/// Missing binding → 503 "Admin build not found"; missing asset file
/// falls back to the SPA entry point, except under `/admin/assets/`
/// where a 404 stays a 404 so broken bundles are visible.
async fn serve_admin(req: &Request, env: &Env) -> Result<Response> {
    let fetcher = match env.assets("ASSETS") {
        Ok(f) => f,
        Err(_) => {
            return Ok(Response::from_html(
                "<!DOCTYPE html><html><head><title>Wordless admin</title></head><body><h1>Admin build not found</h1><p>The admin SPA has not been built yet.</p></body></html>",
            )?
            .with_status(503));
        }
    };
    let url = req.url()?;
    let path = url.path();
    match fetcher.fetch_request(req.clone()?).await {
        Ok(asset) => {
            if asset.status_code() != 404 {
                return Ok(asset);
            }
            if path.starts_with("/admin/assets/") {
                return Ok(asset);
            }
            let mut index_url = url.clone();
            index_url.set_path("/admin/index.html");
            let index_req = Request::new(&index_url.to_string(), req.method())?;
            Ok(fetcher.fetch_request(index_req).await?)
        }
        Err(_) => Ok(Response::from_html(
            "<!DOCTYPE html><html><head><title>Wordless admin</title></head><body><h1>Admin unavailable</h1><p>The admin SPA could not be loaded.</p></body></html>",
        )?
        .with_status(503)),
    }
}

#[event(fetch)]
pub async fn main(mut req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let path = url.path().to_string();

    // /admin is served from Static Assets before any routing.
    if path == "/admin" || path.starts_with("/admin/") {
        let resp = serve_admin(&req, &env).await?;
        return with_headers(resp).await;
    }

    // Bindings: DB is required; MEDIA/CONFIG are reported honestly.
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
    let (media, media_present) = match env.bucket("MEDIA") {
        Ok(bucket) => (
            Some(Rc::new(R2Store::new(bucket)) as Rc<dyn wordless_kernel::storage::ObjectStore>),
            true,
        ),
        Err(_) => (None, false),
    };
    let config_present = env.kv("CONFIG").is_ok();
    let ai_present = env.ai("AI").is_ok();

    // P3 secrets + AI env (wasm32: real bindings; native test builds:
    // empty — providers report "not configured").
    #[cfg(target_arch = "wasm32")]
    let ai_env = build_ai_env(&env);
    #[cfg(not(target_arch = "wasm32"))]
    let ai_env = wordless_ai::AiEnv::empty();
    let payment_secrets = wordless_commerce::payments::PaymentSecrets {
        paystack_secret_key: opt_secret(&env, "PAYSTACK_SECRET_KEY"),
        flutterwave_secret_key: opt_secret(&env, "FLUTTERWAVE_SECRET_KEY"),
        flutterwave_secret_hash: opt_secret(&env, "FLUTTERWAVE_SECRET_HASH"),
        stripe_secret_key: opt_secret(&env, "STRIPE_SECRET_KEY"),
        stripe_webhook_secret: opt_secret(&env, "STRIPE_WEBHOOK_SECRET"),
        mock_webhook_secret: opt_secret(&env, "MOCK_PAYMENT_WEBHOOK_SECRET"),
    };
    let int_secrets = wordless_integrations::EnvSecrets {
        email_api_key: opt_secret(&env, "EMAIL_API_KEY"),
        paystack_secret_key: payment_secrets.paystack_secret_key.clone(),
        flutterwave_secret_key: payment_secrets.flutterwave_secret_key.clone(),
        stripe_secret_key: payment_secrets.stripe_secret_key.clone(),
    };
    let int_bindings = wordless_integrations::Bindings {
        d1: true,
        r2: media_present,
        kv: config_present,
        assets: true,
        ai: ai_present,
    };

    // P6: extension host secret + dispatch namespace. Without the
    // secret, extension loading refuses (fail closed) and the resolver
    // is absent, so nothing dispatches.
    let ext_secret = opt_secret(&env, "WORDLESS_EXT_HOST_SECRET");
    let ext_dispatcher_present = env.dynamic_dispatcher("EXT_DISPATCHER").is_ok();
    #[cfg(target_arch = "wasm32")]
    let ext_resolver = build_ext_resolver(&env, ext_secret.clone());
    #[cfg(not(target_arch = "wasm32"))]
    let ext_resolver = None;

    // Split (2026-11-20): converter Worker client (service binding in
    // production, WORDLESS_CONVERTER_URL in local dev).
    #[cfg(target_arch = "wasm32")]
    let converter = build_converter(&env);
    #[cfg(not(target_arch = "wasm32"))]
    let converter = None;

    // Size remedy (2026-11-20): social + WooCommerce sub-workers.
    #[cfg(target_arch = "wasm32")]
    let social = build_proxy_target(
        &env,
        "WORDLESS_SOCIAL_URL",
        "SOCIAL",
        "social-wordless.internal",
    );
    #[cfg(not(target_arch = "wasm32"))]
    let social = None;
    #[cfg(target_arch = "wasm32")]
    let woo = build_proxy_target(&env, "WORDLESS_WOO_URL", "WOO", "woo-wordless.internal");
    #[cfg(not(target_arch = "wasm32"))]
    let woo = None;
    // Size remedy (2026-11-20): school Worker sub-worker.
    #[cfg(target_arch = "wasm32")]
    let school = build_proxy_target(
        &env,
        "WORDLESS_SCHOOL_URL",
        "SCHOOL",
        "school-wordless.internal",
    );
    #[cfg(not(target_arch = "wasm32"))]
    let school = None;
    // SEO split (2026-11-20): SEO Worker sub-worker.
    #[cfg(target_arch = "wasm32")]
    let seo = build_proxy_target(&env, "WORDLESS_SEO_URL", "SEO", "seo-wordless.internal");
    #[cfg(not(target_arch = "wasm32"))]
    let seo = None;
    #[cfg(target_arch = "wasm32")]
    let woo_client = build_woo_client(&env);
    #[cfg(not(target_arch = "wasm32"))]
    let woo_client = None;

    // Session user first, exactly like v1's fetch().
    let cookie = req.headers().get("Cookie").ok().flatten();
    let token = parse_session_cookie(cookie.as_deref());
    let user = match user_from_session(db.as_ref(), token.as_deref()).await {
        Ok(u) => u.map(|u| u.to_auth_user()),
        Err(e) => {
            worker::console_error!("session lookup failed: {e}");
            None
        }
    };

    // P12: resolve the request to a site (header → subdomain →
    // path prefix → main site 1). The admin header only counts when
    // the caller is a logged-in user who can edit posts; a visitor
    // cannot switch sites by sending a header. On any resolution
    // error (e.g. a D1 that has not run migration 0018 yet) the
    // request falls back to the main site rather than failing.
    let network_domain = opt_secret(&env, "WORDLESS_NETWORK_DOMAIN").unwrap_or_default();
    let header_site = if user
        .as_ref()
        .map(|u| wordless_kernel::capabilities::user_can(u, "edit_posts"))
        .unwrap_or(false)
    {
        req.headers()
            .get("X-Wordless-Site")
            .ok()
            .flatten()
            .map(|v| v.to_string())
    } else {
        None
    };
    let host = url.host_str().unwrap_or_default().to_string();
    let resolution = wordless_multisite::resolve_site(
        db.as_ref(),
        &host,
        &path,
        &network_domain,
        header_site.as_deref(),
    )
    .await
    .unwrap_or_else(|e| {
        worker::console_error!("site resolution failed, using main site: {e}");
        wordless_multisite::SiteResolution {
            site_id: 1,
            slug: String::new(),
            status: "public".to_string(),
            stripped_path: path.clone(),
        }
    });
    let site_id = resolution.site_id;
    let site_slug = resolution.slug;
    let site_archived = resolution.status == "archived";
    // P12: archived sites refuse public rendering at the entry
    // (the API can still administer them, so /api/* passes through).
    if site_archived && !path.starts_with("/api") {
        let resp = RouteResponse::json_error(
            "not_found",
            404,
            "This site is archived.",
        );
        return to_worker_response(resp);
    }
    let path = resolution.stripped_path;

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
    // P12: forward the resolved site to sub-workers through the
    // proxy (proxy.rs forwards ctx.headers verbatim). Sub-workers
    // read it back into their own site context.
    headers.insert("x-wordless-site".to_string(), site_id.to_string());
    // Raw bytes, not text: P2 uploads (media, themes) are binary
    // multipart bodies and must survive byte-identical. JSON handlers
    // parse from bytes the same way they parsed from text.
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
        site_id,
        site_slug,
        site_archived,
        query,
        query_string: url.query().unwrap_or_default().to_string(),
        headers,
        body,
        params: HashMap::new(),
        user,
        db,
        media,
        bindings: Bindings {
            db: true,
            media: media_present,
            config: config_present,
        },
        origin: url.origin().ascii_serialization(),
        purge_urls: Vec::new(),
        ai_env,
        payment_secrets,
        int_secrets,
        int_bindings,
        http_post: Rc::new(WorkerHttpPost),
        ext_secret,
        ext_dispatcher_present,
        ext_resolver,
        converter,
        social,
        woo,
        school,
        seo,
        woo_client,
    };

    // Anonymous public GETs are cacheable; never cache API/admin/
    // logged-in traffic. Preview renders (?wl_preview=) are never
    // cached either (v1 `fetch()`).
    let cacheable = is_cacheable_public_request(
        ctx.method.as_str(),
        ctx.path.as_str(),
        ctx.query.contains_key("wl_preview"),
        token.is_some(),
    );
    if cacheable {
        let cache = Cache::default();
        if let Ok(Some(cached)) = cache.get(&req, false).await {
            return with_headers(cached).await;
        }
    }

    let router = build_router();
    let mut resp = router.handle(&mut ctx).await;

    // Public fallback: anything that is not an API/admin route is a
    // site URL (v1 `handlePublicGet` / `handlePublicCommentPost`).
    if resp.status == 404 && !ctx.path.starts_with("/api/") && !ctx.path.starts_with("/admin") {
        if ctx.method == "GET" || (ctx.method == "POST" && ctx.path.ends_with("/comments")) {
            resp = handle_public_fallback(&mut ctx).await;
        }
    }

    // Record public 404s (the log strips query strings; the home page
    // itself is never a 404 even when ?p= misses). Never breaks the
    // response.
    if resp.status == 404 && ctx.method == "GET" && ctx.path != "/" {
        let referrer = req
            .headers()
            .get("Referer")
            .ok()
            .flatten()
            .unwrap_or_default();
        log_not_found(ctx.db.as_ref(), ctx.site_id, &ctx.path, &referrer).await;
    }

    // Best-effort cache purge (post create/update, redirect creates).
    // A purge failure never fails the request — the entry expires
    // after 60 s on its own.
    if !ctx.purge_urls.is_empty() {
        let cache = Cache::default();
        for purge_url in &ctx.purge_urls {
            let _ = cache.delete(purge_url.as_str(), false).await;
        }
    }

    // Cacheable 200s are stored with a 60 s Cache-Control (+
    // stale-while-revalidate); the client response carries the same
    // header. Cache write failures must not break the response.
    if cacheable && resp.status == 200 {
        resp.extra_headers
            .push(("Cache-Control".to_string(), public_cache_control()));
        let mut cache_resp = match Response::from_bytes(resp.body.clone()) {
            Ok(r) => r.with_status(resp.status),
            Err(_) => return to_worker_response(resp),
        };
        if cache_resp
            .headers_mut()
            .set("Content-Type", &resp.content_type)
            .is_err()
        {
            return to_worker_response(resp);
        }
        for (name, value) in &resp.extra_headers {
            let _ = cache_resp.headers_mut().set(name, value);
        }
        for (name, value) in security_headers() {
            let _ = cache_resp.headers_mut().set(&name, &value);
        }
        let cache = Cache::default();
        let _ = cache.put(&req, cache_resp).await;
    }

    to_worker_response(resp)
}

/// Cron Trigger handler (P4 scheduling — the deploy-time half of the
/// mechanism; the other half is the lazy publish check on single-post
/// reads). Every 5 minutes (`[triggers] crons` in `wrangler.toml`) it
/// flips every due scheduled draft to `publish` via the same
/// `publish_due_posts` the lazy path calls. A failure is logged, not
/// retried here — the next tick picks the post up again, and the lazy
/// check covers reads in between. Local `wrangler dev` fires no real
/// cron, so this path is unit-tested (the service) and only observed
/// firing on a deployed Worker (P5).
///
/// The P9 exam sweep (closing expired attempts) does NOT live here
/// any more: it moved to the school Worker with the 2026-11-20
/// school split (its own 5-minute trigger — see
/// `crates/wordless-school-worker/src/lib.rs`).
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
    match publish_due_posts(db.as_ref(), now_ms()).await {
        Ok(0) => {}
        Ok(n) => {
            worker::console_log!("scheduled: published {n} due post(s)");
        }
        Err(e) => {
            worker::console_error!("scheduled: publish sweep failed: {e}");
        }
    }
    Ok(())
}

/// Add the security headers to a response that did not go through the
/// router (the admin path). Responses from the ASSETS fetcher have
/// IMMUTABLE headers (`TypeError: Can't modify immutable headers` if
/// you call `headers_mut().set()` on them — found by the P1 smoke test),
/// so the response is rebuilt into a fresh mutable one: status, body
/// and copied headers are preserved, then the security headers are set.
async fn with_headers(mut resp: Response) -> Result<Response> {
    let status = resp.status_code();
    let headers: Vec<(String, String)> = resp.headers().entries().collect();
    let body = resp.bytes().await?;
    let mut out = Response::from_bytes(body)?.with_status(status);
    for (name, value) in headers {
        out.headers_mut().set(&name, &value)?;
    }
    for (name, value) in security_headers() {
        out.headers_mut().set(&name, &value)?;
    }
    Ok(out)
}

/// R2 bucket behind the kernel's `ObjectStore` trait.
struct R2Store {
    bucket: worker::Bucket,
}

impl R2Store {
    fn new(bucket: worker::Bucket) -> Self {
        Self { bucket }
    }
}

#[async_trait::async_trait(?Send)]
impl wordless_kernel::storage::ObjectStore for R2Store {
    async fn count_objects(
        &self,
        limit: usize,
    ) -> std::result::Result<(usize, bool), wordless_kernel::WordlessError> {
        let listed = self
            .bucket
            .list()
            .limit(limit as u32)
            .execute()
            .await
            .map_err(|e| {
                wordless_kernel::WordlessError::new(
                    "internal_error",
                    500,
                    &format!("R2 list failed: {e}"),
                )
            })?;
        Ok((listed.objects().len(), listed.truncated()))
    }

    async fn get_object(
        &self,
        key: &str,
    ) -> std::result::Result<Option<(Vec<u8>, Option<String>)>, wordless_kernel::WordlessError>
    {
        let obj = self.bucket.get(key).execute().await.map_err(|e| {
            wordless_kernel::WordlessError::new(
                "internal_error",
                500,
                &format!("R2 get failed: {e}"),
            )
        })?;
        let Some(obj) = obj else { return Ok(None) };
        let content_type = obj.http_metadata().content_type;
        let Some(body) = obj.body() else {
            return Ok(Some((Vec::new(), content_type)));
        };
        let bytes = body.bytes().await.map_err(|e| {
            wordless_kernel::WordlessError::new(
                "internal_error",
                500,
                &format!("R2 read failed: {e}"),
            )
        })?;
        Ok(Some((bytes, content_type)))
    }

    async fn put_object(
        &self,
        key: &str,
        bytes: Vec<u8>,
        content_type: &str,
    ) -> std::result::Result<(), wordless_kernel::WordlessError> {
        self.bucket
            .put(key, bytes)
            .http_metadata(worker::HttpMetadata {
                content_type: Some(content_type.to_string()),
                content_language: None,
                content_disposition: None,
                content_encoding: None,
                cache_control: None,
                cache_expiry: None,
            })
            .execute()
            .await
            .map_err(|e| {
                wordless_kernel::WordlessError::new(
                    "internal_error",
                    500,
                    &format!("R2 put failed: {e}"),
                )
            })?;
        Ok(())
    }

    async fn delete_object(
        &self,
        key: &str,
    ) -> std::result::Result<(), wordless_kernel::WordlessError> {
        self.bucket.delete(key).await.map_err(|e| {
            wordless_kernel::WordlessError::new(
                "internal_error",
                500,
                &format!("R2 delete failed: {e}"),
            )
        })?;
        Ok(())
    }

    async fn head_object(
        &self,
        key: &str,
    ) -> std::result::Result<
        Option<wordless_kernel::storage::ObjectMeta>,
        wordless_kernel::WordlessError,
    > {
        let obj = self.bucket.head(key).await.map_err(|e| {
            wordless_kernel::WordlessError::new(
                "internal_error",
                500,
                &format!("R2 head failed: {e}"),
            )
        })?;
        let Some(obj) = obj else { return Ok(None) };
        Ok(Some(wordless_kernel::storage::ObjectMeta {
            key: obj.key(),
            size: obj.size(),
            content_type: obj.http_metadata().content_type,
            uploaded_at: Some(wordless_kernel::time::iso_from_ms(
                obj.uploaded().as_millis(),
            )),
        }))
    }

    async fn list_objects(
        &self,
        prefix: &str,
        limit: usize,
        cursor: Option<&str>,
    ) -> std::result::Result<wordless_kernel::storage::ListPage, wordless_kernel::WordlessError>
    {
        let mut list = self.bucket.list().prefix(prefix).limit(limit as u32);
        if let Some(cursor) = cursor {
            list = list.cursor(cursor);
        }
        let listed = list.execute().await.map_err(|e| {
            wordless_kernel::WordlessError::new(
                "internal_error",
                500,
                &format!("R2 list failed: {e}"),
            )
        })?;
        let objects = listed
            .objects()
            .into_iter()
            .map(|obj| wordless_kernel::storage::ObjectMeta {
                key: obj.key(),
                size: obj.size(),
                content_type: obj.http_metadata().content_type,
                uploaded_at: Some(wordless_kernel::time::iso_from_ms(
                    obj.uploaded().as_millis(),
                )),
            })
            .collect();
        Ok(wordless_kernel::storage::ListPage {
            objects,
            truncated: listed.truncated(),
            cursor: listed.cursor(),
        })
    }
}
