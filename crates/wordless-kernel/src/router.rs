//! Method + pattern router with per-route auth/role guards.
//! (Port of v1 `src/kernel/router.ts` — same segment matching, same
//! 401/403/404/405 behaviour, same envelope.)
//!
//! The router is generic over the request context `C` so the kernel
//! stays free of Worker types: the worker entry defines the concrete
//! context (it holds the D1 handle, the session user, …) and implements
//! `RequestContext` for it.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use crate::capabilities::{user_can, AuthUser};
use crate::envelope::RouteResponse;
use crate::error::{bad_request, WordlessError};

/// What the router needs from a request. Implemented by the worker
/// entry's context type.
pub trait RequestContext {
    fn method(&self) -> &str;
    fn path(&self) -> &str;
    fn user(&self) -> Option<&AuthUser>;
    fn set_param(&mut self, name: String, value: String);
    fn param(&self, name: &str) -> Option<&str>;
    fn body(&self) -> &[u8];
    fn query(&self, name: &str) -> Option<&str>;
    fn header(&self, name: &str) -> Option<&str>;
}

pub type HandlerResult = Result<RouteResponse, WordlessError>;

/// A route handler: takes the context mutably, returns a future.
/// (Boxed this way so async handlers can live in a `Vec` — the same
/// shape `async-trait` generates, hand-written to keep the kernel free
/// of extra machinery.)
///
/// NOTE: the future is deliberately NOT `Send`. Handlers await D1/R2,
/// whose futures are `!Send` (`JsFuture` holds an `Rc`), and the
/// Workers runtime is single-threaded (`spawn_local`). Requiring
/// `Send` here would make every handler unimplementable.
pub type HandlerFn<C> =
    Box<dyn for<'a> Fn(&'a mut C) -> Pin<Box<dyn Future<Output = HandlerResult> + 'a>>>;

#[derive(Default, Clone, Copy)]
pub struct RouteOptions {
    /// Route requires a logged-in user.
    pub require_auth: bool,
    /// WordPress-style capability the user must hold (implies require_auth).
    pub capability: Option<&'static str>,
}

struct Route<C> {
    method: String,
    segments: Vec<String>,
    handler: HandlerFn<C>,
    require_auth: bool,
    capability: Option<&'static str>,
}

pub struct Router<C> {
    routes: Vec<Route<C>>,
}

fn split_path(path: &str) -> Vec<String> {
    path.trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn match_segments(route: &[String], path: &[String]) -> Option<HashMap<String, String>> {
    if route.len() != path.len() {
        return None;
    }
    let mut params = HashMap::new();
    for (r, p) in route.iter().zip(path.iter()) {
        if let Some(name) = r.strip_prefix(':') {
            let decoded = percent_decode(p);
            params.insert(name.to_string(), decoded);
        } else if r != p {
            return None;
        }
    }
    Some(params)
}

/// Minimal percent-decoding for `:param` segments (v1 uses
/// `decodeURIComponent`; this covers the common cases).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl<C: RequestContext> Router<C> {
    pub fn new() -> Self {
        Self { routes: Vec::new() }
    }

    pub fn add<F>(&mut self, method: &str, pattern: &str, handler: F, opts: RouteOptions)
    where
        F: for<'a> Fn(&'a mut C) -> Pin<Box<dyn Future<Output = HandlerResult> + 'a>> + 'static,
    {
        self.routes.push(Route {
            method: method.to_uppercase(),
            segments: split_path(pattern),
            handler: Box::new(handler),
            require_auth: opts.require_auth || opts.capability.is_some(),
            capability: opts.capability,
        });
    }

    pub fn get<F>(&mut self, pattern: &str, handler: F, opts: RouteOptions)
    where
        F: for<'a> Fn(&'a mut C) -> Pin<Box<dyn Future<Output = HandlerResult> + 'a>> + 'static,
    {
        self.add("GET", pattern, handler, opts);
    }

    pub fn post<F>(&mut self, pattern: &str, handler: F, opts: RouteOptions)
    where
        F: for<'a> Fn(&'a mut C) -> Pin<Box<dyn Future<Output = HandlerResult> + 'a>> + 'static,
    {
        self.add("POST", pattern, handler, opts);
    }

    pub fn put<F>(&mut self, pattern: &str, handler: F, opts: RouteOptions)
    where
        F: for<'a> Fn(&'a mut C) -> Pin<Box<dyn Future<Output = HandlerResult> + 'a>> + 'static,
    {
        self.add("PUT", pattern, handler, opts);
    }

    pub fn delete<F>(&mut self, pattern: &str, handler: F, opts: RouteOptions)
    where
        F: for<'a> Fn(&'a mut C) -> Pin<Box<dyn Future<Output = HandlerResult> + 'a>> + 'static,
    {
        self.add("DELETE", pattern, handler, opts);
    }

    pub async fn handle(&self, ctx: &mut C) -> RouteResponse {
        let segments = split_path(ctx.path());
        let method = ctx.method().to_uppercase();
        let mut path_matched = false;
        for route in &self.routes {
            let Some(params) = match_segments(&route.segments, &segments) else {
                continue;
            };
            path_matched = true;
            if route.method != method {
                continue;
            }
            if route.require_auth && ctx.user().is_none() {
                return RouteResponse::json_error("unauthorized", 401, "Authentication required.");
            }
            if let Some(cap) = route.capability {
                match ctx.user() {
                    Some(u) if user_can(u, cap) => {}
                    _ => {
                        return RouteResponse::json_error(
                            "forbidden",
                            403,
                            "You do not have permission to do that.",
                        )
                    }
                }
            }
            for (k, v) in params {
                ctx.set_param(k, v);
            }
            return match (route.handler)(ctx).await {
                Ok(resp) => resp,
                Err(e) => RouteResponse::json_error(&e.code, e.status, &e.message),
            };
        }
        if path_matched {
            return RouteResponse::json_error(
                "method_not_allowed",
                405,
                &format!("Method {method} is not allowed on this route."),
            );
        }
        RouteResponse::json_error("not_found", 404, "Route not found.")
    }
}

impl<C: RequestContext> Default for Router<C> {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a JSON request body, raising a 400 on invalid JSON.
/// (Port of v1 `readJsonBody`.)
pub fn read_json_body(ctx: &impl RequestContext) -> Result<serde_json::Value, WordlessError> {
    serde_json::from_slice(ctx.body()).map_err(|_| bad_request("Request body must be valid JSON."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct TestCtx {
        method: String,
        path: String,
        params: HashMap<String, String>,
        user: Option<AuthUser>,
        body: Vec<u8>,
    }

    impl TestCtx {
        fn new(method: &str, path: &str) -> Self {
            Self {
                method: method.into(),
                path: path.into(),
                params: HashMap::new(),
                user: None,
                body: Vec::new(),
            }
        }
    }

    impl RequestContext for TestCtx {
        fn method(&self) -> &str {
            &self.method
        }
        fn path(&self) -> &str {
            &self.path
        }
        fn user(&self) -> Option<&AuthUser> {
            self.user.as_ref()
        }
        fn set_param(&mut self, name: String, value: String) {
            self.params.insert(name, value);
        }
        fn param(&self, name: &str) -> Option<&str> {
            self.params.get(name).map(|s| s.as_str())
        }
        fn body(&self) -> &[u8] {
            &self.body
        }
        fn query(&self, _name: &str) -> Option<&str> {
            None
        }
        fn header(&self, _name: &str) -> Option<&str> {
            None
        }
    }

    fn admin() -> AuthUser {
        AuthUser {
            id: 1,
            login: "admin".into(),
            email: "a@example.com".into(),
            display_name: "Admin".into(),
            role: crate::capabilities::Role::Administrator,
        }
    }

    fn subscriber() -> AuthUser {
        AuthUser {
            id: 2,
            login: "sub".into(),
            email: "s@example.com".into(),
            display_name: "Sub".into(),
            role: crate::capabilities::Role::Subscriber,
        }
    }

    fn ok_json(data: serde_json::Value) -> HandlerResult {
        Ok(RouteResponse::json_ok(data, 200))
    }

    #[test]
    fn exact_and_param_routes_match() {
        let mut router: Router<TestCtx> = Router::new();
        router.get(
            "/api/health",
            |_ctx| Box::pin(async move { ok_json(json!({ "status": "ok" })) }),
            RouteOptions::default(),
        );
        router.get(
            "/api/posts/:id",
            |ctx| {
                let id = ctx.param("id").unwrap_or("").to_string();
                Box::pin(async move { ok_json(json!({ "id": id })) })
            },
            RouteOptions::default(),
        );

        let mut ctx = TestCtx::new("GET", "/api/health");
        let resp = futures_executor::block_on(router.handle(&mut ctx));
        assert_eq!(resp.status, 200);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&resp.body).unwrap()["data"]["status"],
            "ok"
        );

        let mut ctx = TestCtx::new("GET", "/api/posts/42");
        let resp = futures_executor::block_on(router.handle(&mut ctx));
        assert_eq!(resp.status, 200);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&resp.body).unwrap()["data"]["id"],
            "42"
        );
    }

    #[test]
    fn unknown_route_is_404_and_wrong_method_is_405() {
        let mut router: Router<TestCtx> = Router::new();
        router.get(
            "/api/health",
            |_ctx| Box::pin(async move { ok_json(json!({})) }),
            RouteOptions::default(),
        );

        let mut ctx = TestCtx::new("GET", "/api/nope");
        let resp = futures_executor::block_on(router.handle(&mut ctx));
        assert_eq!(resp.status, 404);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&resp.body).unwrap()["error"]["code"],
            "not_found"
        );

        let mut ctx = TestCtx::new("POST", "/api/health");
        let resp = futures_executor::block_on(router.handle(&mut ctx));
        assert_eq!(resp.status, 405);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&resp.body).unwrap()["error"]["code"],
            "method_not_allowed"
        );
    }

    #[test]
    fn auth_guard_returns_401_and_capability_guard_returns_403() {
        let mut router: Router<TestCtx> = Router::new();
        router.get(
            "/api/auth/me",
            |_ctx| Box::pin(async move { ok_json(json!({})) }),
            RouteOptions {
                require_auth: true,
                capability: None,
            },
        );
        router.get(
            "/api/settings",
            |_ctx| Box::pin(async move { ok_json(json!({})) }),
            RouteOptions {
                require_auth: false,
                capability: Some("manage_options"),
            },
        );

        // Anonymous -> 401 on the auth route.
        let mut ctx = TestCtx::new("GET", "/api/auth/me");
        let resp = futures_executor::block_on(router.handle(&mut ctx));
        assert_eq!(resp.status, 401);

        // Logged in -> 200.
        let mut ctx = TestCtx::new("GET", "/api/auth/me");
        ctx.user = Some(subscriber());
        let resp = futures_executor::block_on(router.handle(&mut ctx));
        assert_eq!(resp.status, 200);

        // Subscriber lacks manage_options -> 403.
        let mut ctx = TestCtx::new("GET", "/api/settings");
        ctx.user = Some(subscriber());
        let resp = futures_executor::block_on(router.handle(&mut ctx));
        assert_eq!(resp.status, 403);

        // Admin has it -> 200.
        let mut ctx = TestCtx::new("GET", "/api/settings");
        ctx.user = Some(admin());
        let resp = futures_executor::block_on(router.handle(&mut ctx));
        assert_eq!(resp.status, 200);
    }

    #[test]
    fn handler_error_becomes_envelope_error() {
        let mut router: Router<TestCtx> = Router::new();
        router.get(
            "/api/boom",
            |_ctx| {
                Box::pin(async move {
                    Err::<RouteResponse, WordlessError>(crate::error::bad_request("bad input"))
                })
            },
            RouteOptions::default(),
        );
        let mut ctx = TestCtx::new("GET", "/api/boom");
        let resp = futures_executor::block_on(router.handle(&mut ctx));
        assert_eq!(resp.status, 400);
        let v = serde_json::from_slice::<serde_json::Value>(&resp.body).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["code"], "bad_request");
        assert_eq!(v["data"], serde_json::Value::Null);
    }

    #[test]
    fn read_json_body_rejects_invalid_json() {
        let mut ctx = TestCtx::new("POST", "/api/x");
        ctx.body = b"{not json".to_vec();
        assert!(read_json_body(&ctx).is_err());
        ctx.body = b"{\"a\":1}".to_vec();
        assert_eq!(read_json_body(&ctx).unwrap()["a"], 1);
    }
}
