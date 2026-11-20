//! The JSON envelope `{ ok, data, error }` and the platform-neutral
//! response type handlers return. The worker entry converts a
//! `RouteResponse` into a real `worker::Response`.
//! (Port of v1 `src/kernel/router.ts` `jsonOk` / `jsonError`.)

use serde_json::{json, Value};

/// A response produced by a route handler, before it is turned into a
/// platform `Response` by the worker entry.
#[derive(Debug, Clone)]
pub struct RouteResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_type: String,
    /// Extra headers (e.g. `Set-Cookie`). Security headers are added by
    /// the worker entry, not here.
    pub extra_headers: Vec<(String, String)>,
}

impl RouteResponse {
    pub fn json_ok(data: Value, status: u16) -> Self {
        let body = serde_json::to_vec(&json!({ "ok": true, "data": data, "error": Value::Null }))
            .expect("envelope JSON must serialize");
        Self {
            status,
            body,
            content_type: "application/json; charset=utf-8".to_string(),
            extra_headers: Vec::new(),
        }
    }

    pub fn json_error(code: &str, status: u16, message: &str) -> Self {
        let body = serde_json::to_vec(&json!({
            "ok": false,
            "data": Value::Null,
            "error": { "code": code, "message": message },
        }))
        .expect("envelope JSON must serialize");
        Self {
            status,
            body,
            content_type: "application/json; charset=utf-8".to_string(),
            extra_headers: Vec::new(),
        }
    }

    pub fn html(body: String, status: u16) -> Self {
        Self {
            status,
            body: body.into_bytes(),
            content_type: "text/html; charset=utf-8".to_string(),
            extra_headers: Vec::new(),
        }
    }

    /// Empty body with NO `Content-Type` header (P5 F1): v1 redirects
    /// are `new Response(null, {status, headers: {Location}})` — no
    /// content type at all. `to_worker_response` skips the header when
    /// `content_type` is empty.
    pub fn empty(status: u16) -> Self {
        Self {
            status,
            body: Vec::new(),
            content_type: String::new(),
            extra_headers: Vec::new(),
        }
    }

    /// Raw bytes (P3: backup ZIP download). The caller sets the
    /// content type and any `Content-Disposition` via `with_header`.
    pub fn bytes(body: Vec<u8>, content_type: &str, status: u16) -> Self {
        Self {
            status,
            body,
            content_type: content_type.to_string(),
            extra_headers: Vec::new(),
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.extra_headers
            .push((name.to_string(), value.to_string()));
        self
    }
}
