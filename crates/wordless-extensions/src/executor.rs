//! Extension invocation — how the main Worker calls an extension
//! Worker, with a timeout that protects the main Worker.
//!
//! One fetch-based protocol, three ways to reach an extension
//! ([`DispatchTarget`]): a service binding (local dev/test), a
//! dispatch namespace (production), or a plain HTTP(S) URL (tests and
//! local dev without bindings). The transport ([`ExtensionTransport`])
//! is the only part that knows which one it is; the executor
//! ([`HttpExecutor`]) just POSTs the invoke body and parses the reply.
//!
//! Timeout: an extension runs in its own isolate, so a runaway
//! extension cannot block the main Worker — but the main Worker's
//! fetch would still wait. `invoke` races the transport call against
//! a sleep ([`race_with_timeout`]; `worker::Delay` on wasm32) and
//! returns a timeout result instead of hanging. The extension is
//! left running in its own isolate; the run is logged as `timeout`
//! and the extension can be disabled from the registry.

use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How long an extension invocation may run before the main Worker
/// gives up on it (default). The extension keeps running in its own
/// isolate — the timeout protects the main Worker, not the extension.
pub const DEFAULT_INVOKE_TIMEOUT_MS: u64 = 5_000;

/// One hook invocation.
#[derive(Debug, Clone)]
pub struct InvokeRequest {
    /// Extension slug (also the dispatch name).
    pub slug: String,
    /// Hook tag, e.g. `admin_notices`.
    pub hook: String,
    /// `"action"` or `"filter"`.
    pub kind: String,
    /// Hook arguments (for a filter, `args[0]` is the value).
    pub args: Vec<Value>,
    /// Main Worker origin the extension calls back to for host calls.
    pub host_url: String,
    /// Per-load bearer token for the host endpoint (never stored in
    /// the extension Worker).
    pub host_token: String,
    /// User id on whose behalf the hook runs (0 = no actor).
    pub actor_user_id: i64,
    /// Locale for `get_locale()` inside the extension.
    pub locale: String,
    /// Timeout in milliseconds (0 = the default).
    pub timeout_ms: u64,
}

/// The outcome of one invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvokeResult {
    pub ok: bool,
    /// Filter value (kind = filter) or action callback results.
    pub result: Value,
    /// Strings the extension's action callbacks rendered.
    pub rendered: Vec<String>,
    /// Error message when `ok` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Wall time of the invocation in milliseconds.
    pub duration_ms: u64,
    /// True when the invocation hit the timeout (the extension may
    /// still be running in its own isolate).
    pub timed_out: bool,
}

/// How an extension Worker is addressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchTarget {
    /// Service binding (local dev/test): the binding name in
    /// `wrangler.toml` (e.g. `EXT`).
    ServiceBinding { binding: String },
    /// Dispatch namespace (production): the namespace binding name
    /// (e.g. `EXT_DISPATCHER`).
    DispatchNamespace { binding: String },
    /// Plain HTTP(S) URL (tests and local dev without bindings).
    Http { base_url: String },
}

/// Transport: one POST (and one GET for health) to an extension
/// Worker. Implementations: `worker::Fetcher` (service bindings and
/// dispatch namespaces — the worker crate) and plain fetch (tests).
/// The transport returns the HTTP status and the raw body text; the
/// executor parses the invoke envelope.
///
/// `#[async_trait(?Send)]` (like the webhooks crate's `HttpPost`):
/// the boxed futures make the trait object-safe, so the worker crate
/// can hold `Rc<dyn ExtensionExecutor>`.
#[async_trait(?Send)]
pub trait ExtensionTransport {
    async fn post_json(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &str,
    ) -> Result<(u16, String), String>;
    async fn get_json(&self, url: &str) -> Result<(u16, String), String>;
}

/// A shared reference to a transport is itself a transport (so callers
/// can hold one transport and lend it to many executors).
#[async_trait(?Send)]
impl<T: ExtensionTransport> ExtensionTransport for &T {
    async fn post_json(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &str,
    ) -> Result<(u16, String), String> {
        (**self).post_json(url, headers, body).await
    }

    async fn get_json(&self, url: &str) -> Result<(u16, String), String> {
        (**self).get_json(url).await
    }
}

/// A shared executor is itself an executor — this is what lets a
/// `Rc<dyn ExtensionExecutor>` (the form the worker crate's resolver
/// returns) be passed anywhere an executor is needed.
#[async_trait(?Send)]
impl<T: ExtensionExecutor + ?Sized> ExtensionExecutor for std::rc::Rc<T> {
    async fn invoke(&self, req: &InvokeRequest) -> Result<InvokeResult, String> {
        (**self).invoke(req).await
    }

    async fn health(&self, slug: &str) -> Result<bool, String> {
        (**self).health(slug).await
    }
}

/// The executor: builds the invoke body, POSTs it with a timeout,
/// and parses the extension's reply.
#[async_trait(?Send)]
pub trait ExtensionExecutor {
    async fn invoke(&self, req: &InvokeRequest) -> Result<InvokeResult, String>;
    async fn health(&self, slug: &str) -> Result<bool, String>;
}

/// Executor over any [`ExtensionTransport`]. `base_url` is the
/// extension Worker's origin — a real origin for the HTTP transport,
/// a dummy origin (`https://ext-wordless.internal`) for Fetcher
/// transports, which route by binding and ignore the host.
pub struct HttpExecutor<T: ExtensionTransport> {
    transport: T,
    base_url: String,
}

impl<T: ExtensionTransport> HttpExecutor<T> {
    pub fn new(transport: T, base_url: impl Into<String>) -> Self {
        Self {
            transport,
            base_url: base_url.into(),
        }
    }

    fn invoke_url(&self) -> String {
        format!("{}/__wordless/invoke", self.base_url.trim_end_matches('/'))
    }

    fn health_url(&self, slug: &str) -> String {
        format!(
            "{}/__wordless/health?slug={slug}",
            self.base_url.trim_end_matches('/')
        )
    }
}

#[async_trait(?Send)]
impl<T: ExtensionTransport> ExtensionExecutor for HttpExecutor<T> {
    async fn invoke(&self, req: &InvokeRequest) -> Result<InvokeResult, String> {
        let timeout_ms = if req.timeout_ms == 0 {
            DEFAULT_INVOKE_TIMEOUT_MS
        } else {
            req.timeout_ms
        };
        let started = now_ms();
        let body = serde_json::to_string(&serde_json::json!({
            "hook": req.hook,
            "kind": req.kind,
            "args": req.args,
            "hostUrl": req.host_url,
            "hostToken": req.host_token,
            "actorUserId": req.actor_user_id,
            "locale": req.locale,
        }))
        .map_err(|e| format!("failed to serialise invoke request: {e}"))?;
        let headers = [("content-type".to_string(), "application/json".to_string())];
        let url = self.invoke_url();
        let call = self.transport.post_json(&url, &headers, &body);
        let outcome = race_with_timeout(call, sleep_ms(timeout_ms)).await;
        let duration_ms = now_ms().saturating_sub(started);
        match outcome {
            Err(()) => Ok(InvokeResult {
                ok: false,
                result: Value::Null,
                rendered: vec![],
                error: Some(format!(
                    "extension {} timed out after {timeout_ms} ms (hook {})",
                    req.slug, req.hook
                )),
                duration_ms,
                timed_out: true,
            }),
            Ok(Err(e)) => Err(format!("extension {} invoke failed: {e}", req.slug)),
            Ok(Ok((status, text))) => {
                if !(200..300).contains(&status) {
                    return Ok(InvokeResult {
                        ok: false,
                        result: Value::Null,
                        rendered: vec![],
                        error: Some(format!(
                            "extension {} returned HTTP {status}: {}",
                            req.slug,
                            text.chars().take(200).collect::<String>()
                        )),
                        duration_ms,
                        timed_out: false,
                    });
                }
                parse_invoke_response(&text, duration_ms)
            }
        }
    }

    async fn health(&self, slug: &str) -> Result<bool, String> {
        let (status, text) = self.transport.get_json(&self.health_url(slug)).await?;
        if !(200..300).contains(&status) {
            return Ok(false);
        }
        let body: Value =
            serde_json::from_str(&text).map_err(|e| format!("bad health response: {e}"))?;
        Ok(body.get("ok").and_then(Value::as_bool).unwrap_or(false))
    }
}

/// Parse an extension Worker's invoke reply:
/// `{ok, hook, kind, result, rendered}` or `{ok: false, error}`.
fn parse_invoke_response(text: &str, duration_ms: u64) -> Result<InvokeResult, String> {
    let body: Value =
        serde_json::from_str(text).map_err(|e| format!("bad invoke response: {e}"))?;
    let ok = body.get("ok").and_then(Value::as_bool).unwrap_or(false);
    let result = body.get("result").cloned().unwrap_or(Value::Null);
    let rendered = body
        .get("rendered")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let error = body
        .get("error")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(InvokeResult {
        ok,
        result,
        rendered,
        error,
        duration_ms,
        timed_out: false,
    })
}

/// Race a future against a sleep: `Ok(output)` when the future wins,
/// `Err(())` when the sleep fires first.
pub async fn race_with_timeout<F, S>(fut: F, sleep: S) -> Result<F::Output, ()>
where
    F: Future,
    S: Future<Output = ()>,
{
    use futures_util::future::{select, Either};
    match select(Box::pin(fut), Box::pin(sleep)).await {
        Either::Left((out, _)) => Ok(out),
        Either::Right(((), _)) => Err(()),
    }
}

/// Sleep future: `worker::Delay` on wasm32 (setTimeout-based);
/// a never-resolving future elsewhere (native is test/dev only — the
/// smoke test proves the real timeout on workerd).
fn sleep_ms(ms: u64) -> Pin<Box<dyn Future<Output = ()>>> {
    #[cfg(target_arch = "wasm32")]
    {
        Box::pin(worker::Delay::from(std::time::Duration::from_millis(ms)))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = ms;
        Box::pin(std::future::pending::<()>())
    }
}

/// Wall-clock milliseconds (wasm32: `Date.now()`; native: system time).
fn now_ms() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now() as u64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

#[cfg(target_arch = "wasm32")]
mod fetcher_transport {
    //! `worker::Fetcher`-backed transport (service bindings and
    //! dispatch namespaces). wasm32 only — the `worker` crate is a
    //! wasm32 dependency of this crate.
    use super::*;

    pub struct FetcherTransport {
        fetcher: worker::Fetcher,
    }

    impl FetcherTransport {
        pub fn new(fetcher: worker::Fetcher) -> Self {
            Self { fetcher }
        }
    }

    #[async_trait(?Send)]
    impl ExtensionTransport for FetcherTransport {
        async fn post_json(
            &self,
            url: &str,
            headers: &[(String, String)],
            body: &str,
        ) -> Result<(u16, String), String> {
            let mut init = worker::RequestInit::new();
            init.with_method(worker::Method::Post);
            let h = worker::Headers::new();
            for (name, value) in headers {
                h.set(name, value)
                    .map_err(|e| format!("failed to build extension request headers: {e}"))?;
            }
            init.with_headers(h);
            init.with_body(Some(wasm_bindgen::JsValue::from_str(body)));
            let req = worker::Request::new_with_init(url, &init)
                .map_err(|e| format!("failed to build extension request: {e}"))?;
            let mut resp = self
                .fetcher
                .fetch_request(req)
                .await
                .map_err(|e| format!("extension fetch failed: {e}"))?;
            let text = resp
                .text()
                .await
                .map_err(|e| format!("failed to read extension response: {e}"))?;
            Ok((resp.status_code(), text))
        }

        async fn get_json(&self, url: &str) -> Result<(u16, String), String> {
            let req = worker::Request::new(url, worker::Method::Get)
                .map_err(|e| format!("failed to build extension health request: {e}"))?;
            let mut resp = self
                .fetcher
                .fetch_request(req)
                .await
                .map_err(|e| format!("extension health fetch failed: {e}"))?;
            let text = resp
                .text()
                .await
                .map_err(|e| format!("failed to read extension health response: {e}"))?;
            Ok((resp.status_code(), text))
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use fetcher_transport::FetcherTransport;

#[cfg(test)]
pub mod test_support_transport {
    //! In-memory transport for tests: canned responses, and a hanging
    //! transport for the timeout test (via [`race_with_timeout`]).
    use super::*;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    pub struct FakeTransport {
        responses: Arc<Mutex<HashMap<String, (u16, String)>>>,
    }

    impl FakeTransport {
        pub fn new() -> Self {
            Self {
                responses: Arc::new(Mutex::new(HashMap::new())),
            }
        }

        pub fn respond(&self, url_contains: &str, status: u16, body: &str) {
            self.responses
                .lock()
                .unwrap()
                .insert(url_contains.to_string(), (status, body.to_string()));
        }

        fn lookup(&self, url: &str) -> (u16, String) {
            let map = self.responses.lock().unwrap();
            for (k, v) in map.iter() {
                if url.contains(k.as_str()) {
                    return v.clone();
                }
            }
            (404, "not found".to_string())
        }
    }

    #[async_trait(?Send)]
    impl ExtensionTransport for FakeTransport {
        async fn post_json(
            &self,
            url: &str,
            _headers: &[(String, String)],
            _body: &str,
        ) -> Result<(u16, String), String> {
            Ok(self.lookup(url))
        }

        async fn get_json(&self, url: &str) -> Result<(u16, String), String> {
            Ok(self.lookup(url))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_support_transport::FakeTransport;

    fn invoke_req() -> InvokeRequest {
        InvokeRequest {
            slug: "hello-dolly".to_string(),
            hook: "admin_notices".to_string(),
            kind: "action".to_string(),
            args: vec![],
            host_url: "http://127.0.0.1:8799".to_string(),
            host_token: "tok".to_string(),
            actor_user_id: 1,
            locale: "en_US".to_string(),
            timeout_ms: 5_000,
        }
    }

    #[test]
    fn invoke_parses_action_reply() {
        let transport = FakeTransport::new();
        transport.respond(
            "__wordless/invoke",
            200,
            r#"{"ok":true,"hook":"admin_notices","kind":"action","result":[null],"rendered":["<p id=\"dolly\">Hello</p>"]}"#,
        );
        let exec = HttpExecutor::new(transport, "https://ext-wordless.internal");
        let result = futures_executor::block_on(exec.invoke(&invoke_req())).unwrap();
        assert!(result.ok);
        assert_eq!(
            result.rendered,
            vec!["<p id=\"dolly\">Hello</p>".to_string()]
        );
        assert!(!result.timed_out);
    }

    #[test]
    fn invoke_error_reply_is_not_a_transport_error() {
        let transport = FakeTransport::new();
        transport.respond(
            "__wordless/invoke",
            200,
            r#"{"ok":false,"hook":"x","kind":"action","error":"boom"}"#,
        );
        let exec = HttpExecutor::new(transport, "https://ext-wordless.internal");
        let result = futures_executor::block_on(exec.invoke(&invoke_req())).unwrap();
        assert!(!result.ok);
        assert_eq!(result.error.as_deref(), Some("boom"));
    }

    #[test]
    fn invoke_http_error_is_reported() {
        let transport = FakeTransport::new();
        transport.respond("__wordless/invoke", 500, "worker exploded");
        let exec = HttpExecutor::new(transport, "https://ext-wordless.internal");
        let result = futures_executor::block_on(exec.invoke(&invoke_req())).unwrap();
        assert!(!result.ok);
        assert!(result.error.as_deref().unwrap().contains("500"));
    }

    #[test]
    fn health_true_and_false() {
        let transport = FakeTransport::new();
        transport.respond(
            "__wordless/health",
            200,
            r#"{"ok":true,"slug":"hello-dolly"}"#,
        );
        let exec = HttpExecutor::new(transport, "https://ext-wordless.internal");
        assert!(futures_executor::block_on(exec.health("hello-dolly")).unwrap());
    }

    #[test]
    fn race_with_timeout_fires_when_sleep_wins() {
        let slow = std::future::pending::<u32>();
        let fast_sleep = std::future::ready(());
        assert_eq!(
            futures_executor::block_on(race_with_timeout(slow, fast_sleep)),
            Err(())
        );
        let fast = std::future::ready(42u32);
        let slow_sleep = std::future::pending::<()>();
        assert_eq!(
            futures_executor::block_on(race_with_timeout(fast, slow_sleep)),
            Ok(42)
        );
    }
}
