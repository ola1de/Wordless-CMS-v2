//! Platform HTTP client — the only place this crate touches the
//! JS environment.
//!
//! `reqwest` does not work in Workers (no sockets), so outbound HTTP
//! goes through `worker::Fetch` only. All other code talks to the
//! [`HttpClient`] trait, so it stays testable on native targets with a
//! fake.
//!
//! Honest limit: v1's email provider used `AbortSignal.timeout(10000)`
//! (a 10 s timeout). This client does not reproduce that timeout —
//! the Workers platform bounds fetch duration itself, and no timer
//! API is wired here yet. A timeout wrapper is future work, not a
//! silent claim.

use async_trait::async_trait;
use serde_json::Value;

/// One HTTP response: status code plus the full body as text.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

impl HttpResponse {
    /// True for 2xx statuses (v1 treats any `res.ok` as delivered).
    pub fn is_ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// A transport failure (DNS, TLS, network, invalid URL, …) — as a
/// plain message, never a secret. Non-2xx HTTP statuses are NOT
/// errors here: they come back as [`HttpResponse`] so the caller can
/// record the outcome (v1 records `failed` in the mail log).
#[derive(Debug, Clone)]
pub struct HttpError(pub String);

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HttpError {}

/// Minimal outbound HTTP surface this crate needs: one JSON POST
/// with a Bearer token. `?Send` like the `Db` trait (the Workers
/// runtime is single-threaded).
#[async_trait(?Send)]
pub trait HttpClient {
    /// POST `body` as JSON to `url` with
    /// `Authorization: Bearer <bearer_token>` and
    /// `Content-Type: application/json`.
    async fn post_json(
        &self,
        url: &str,
        bearer_token: &str,
        body: &Value,
    ) -> Result<HttpResponse, HttpError>;
}

/// The real client: `worker::Fetch` on wasm32. On native targets it
/// compiles to a stub that always fails — native code must inject a
/// fake instead (see `test_support::FakeHttpClient`).
#[derive(Debug, Default)]
pub struct WorkerHttpClient;

#[cfg(target_arch = "wasm32")]
#[async_trait(?Send)]
impl HttpClient for WorkerHttpClient {
    async fn post_json(
        &self,
        url: &str,
        bearer_token: &str,
        body: &Value,
    ) -> Result<HttpResponse, HttpError> {
        use wasm_bindgen::JsValue;
        use worker::{Fetch, Headers, Method, Request, RequestInit};

        let headers = Headers::new();
        headers
            .set("Authorization", &format!("Bearer {bearer_token}"))
            .map_err(|e| HttpError(e.to_string()))?;
        headers
            .set("Content-Type", "application/json")
            .map_err(|e| HttpError(e.to_string()))?;
        let mut init = RequestInit::new();
        init.with_method(Method::Post)
            .with_headers(headers)
            .with_body(Some(JsValue::from_str(&body.to_string())));
        let req = Request::new_with_init(url, &init).map_err(|e| HttpError(e.to_string()))?;
        let mut res = Fetch::Request(req)
            .send()
            .await
            .map_err(|e| HttpError(e.to_string()))?;
        let status = res.status_code();
        let body_text = res.text().await.map_err(|e| HttpError(e.to_string()))?;
        Ok(HttpResponse {
            status,
            body: body_text,
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait(?Send)]
impl HttpClient for WorkerHttpClient {
    async fn post_json(
        &self,
        _url: &str,
        _bearer_token: &str,
        _body: &Value,
    ) -> Result<HttpResponse, HttpError> {
        Err(HttpError(
            "HTTP fetch is only available on wasm32 (Cloudflare Workers); inject a test HttpClient on native targets.".to_string(),
        ))
    }
}
