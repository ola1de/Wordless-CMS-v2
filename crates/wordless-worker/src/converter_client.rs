//! Worker-side `ConverterClient` (split, 2026-11-20): reaches the
//! `wordless-converter` Worker over one of two transports, in order
//! (the P6 extension-resolver pattern):
//!
//! 1. `WORDLESS_CONVERTER_URL` — plain HTTP to a locally running
//!    converter Worker (local dev / smoke tests; default
//!    `http://127.0.0.1:8805`, see `scripts/dev-split.sh`).
//! 2. Service binding `CONVERTER` — the free production path: the
//!    converter is a normal account-level Worker, bound in
//!    `wrangler.toml` (`[[services]]`, per env).
//!
//! The client speaks the converter's JSON contract (see
//! `crates/wordless-converter/src/lib.rs`): every endpoint answers
//! `{ "ok": true, ... }` or `{ "ok": false, "error", "blocked"? }`.
//! A transport failure (unreachable, non-2xx, non-JSON) is a
//! `ConverterError` with no `blocked` list; an application failure
//! carries the converter's message (and violations for `convert_php`).

use async_trait::async_trait;
use serde_json::{json, Value};
use std::rc::Rc;
use wordless_kernel::converter::{ConverterClient, ConverterError};
use worker::{Fetch, Fetcher, Headers, Method, Request, RequestInit};

/// One converter transport: a service-binding `Fetcher` (production)
/// or a plain-HTTP base URL (local dev).
enum Transport {
    /// Service binding; requests go to the fake internal origin (the
    /// binding, not the URL, decides the destination — the P6 pattern).
    Fetcher(Fetcher),
    /// Local-dev base URL, e.g. `http://127.0.0.1:8805`.
    Url(String),
}

/// The worker-side converter client.
pub struct WorkerConverterClient {
    transport: Transport,
}

impl WorkerConverterClient {
    /// Service-binding transport (production).
    pub fn via_service(fetcher: Fetcher) -> Rc<dyn ConverterClient> {
        Rc::new(Self {
            transport: Transport::Fetcher(fetcher),
        }) as Rc<dyn ConverterClient>
    }

    /// Plain-HTTP transport (local dev).
    pub fn via_url(base_url: String) -> Rc<dyn ConverterClient> {
        Rc::new(Self {
            transport: Transport::Url(base_url.trim_end_matches('/').to_string()),
        }) as Rc<dyn ConverterClient>
    }

    fn url_for(&self, path: &str) -> String {
        match &self.transport {
            Transport::Fetcher(_) => format!("https://converter-wordless.internal{path}"),
            Transport::Url(base) => format!("{base}{path}"),
        }
    }

    /// POST JSON to one converter endpoint. Returns the response body
    /// (already checked for `"ok": true`); application failures
    /// become `ConverterError` with the converter's message.
    async fn post(&self, path: &str, body: Value) -> Result<Value, ConverterError> {
        let url = self.url_for(path);
        let mut init = RequestInit::new();
        init.with_method(Method::Post);
        let headers = Headers::new();
        headers
            .set("Content-Type", "application/json")
            .map_err(|e| {
                ConverterError::new(format!("Failed to build converter request headers: {e}"))
            })?;
        init.with_headers(headers);
        init.with_body(Some(wasm_bindgen::JsValue::from_str(
            &serde_json::to_string(&body).unwrap_or_default(),
        )));
        let req = Request::new_with_init(&url, &init)
            .map_err(|e| ConverterError::new(format!("Failed to build converter request: {e}")))?;
        let mut resp =
            match &self.transport {
                Transport::Fetcher(fetcher) => fetcher.fetch_request(req).await.map_err(|e| {
                    ConverterError::new(format!("Converter service fetch failed: {e}"))
                })?,
                Transport::Url(_) => Fetch::Request(req).send().await.map_err(|e| {
                    ConverterError::new(format!("Converter service fetch failed: {e}"))
                })?,
            };
        let status = resp.status_code();
        let text = resp
            .text()
            .await
            .map_err(|e| ConverterError::new(format!("Failed to read converter response: {e}")))?;
        if !(200..300).contains(&status) {
            return Err(ConverterError::new(format!(
                "Converter service answered HTTP {status} for {path}."
            )));
        }
        let parsed: Value = serde_json::from_str(&text).map_err(|e| {
            ConverterError::new(format!(
                "Converter service returned non-JSON for {path}: {e}"
            ))
        })?;
        if parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
            return Ok(parsed);
        }
        let message = parsed
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("Converter service failed without a message.")
            .to_string();
        let blocked = parsed.get("blocked").cloned();
        Err(match blocked {
            Some(b) => ConverterError::blocked(message, b),
            None => ConverterError::new(message),
        })
    }
}

#[async_trait(?Send)]
impl ConverterClient for WorkerConverterClient {
    async fn convert_php(
        &self,
        filename: &str,
        source: &str,
        slug: &str,
    ) -> Result<Value, ConverterError> {
        let body = self
            .post(
                "/__convert/convert-php",
                json!({ "filename": filename, "source": source, "slug": slug }),
            )
            .await?;
        body.get("report")
            .cloned()
            .ok_or_else(|| ConverterError::new("Converter convert-php answer has no report."))
    }

    async fn analyze_files(&self, files: &[(String, String)]) -> Result<Value, ConverterError> {
        let body = self
            .post(
                "/__convert/analyze-files",
                json!({
                    "files": files.iter().map(|(p, s)| json!({ "path": p, "source": s })).collect::<Vec<_>>(),
                }),
            )
            .await?;
        body.get("analyses")
            .cloned()
            .ok_or_else(|| ConverterError::new("Converter analyze-files answer has no analyses."))
    }

    async fn convert_files(
        &self,
        files: &[(String, String)],
        slug: &str,
        kind: &str,
    ) -> Result<Value, ConverterError> {
        let body = self
            .post(
                "/__convert/convert-files",
                json!({
                    "files": files.iter().map(|(p, s)| json!({ "path": p, "source": s })).collect::<Vec<_>>(),
                    "slug": slug,
                    "kind": kind,
                }),
            )
            .await?;
        Ok(json!({
            "generated": body.get("generated").cloned().unwrap_or(Value::Null),
            "report": body.get("report").cloned().unwrap_or(Value::Null),
            "defaults": body.get("defaults").cloned().unwrap_or(Value::Null),
        }))
    }

    async fn theme_settings(
        &self,
        theme_json: &Value,
        unsupported: &[String],
    ) -> Result<Value, ConverterError> {
        let body = self
            .post(
                "/__convert/theme-settings",
                json!({ "themeJson": theme_json, "unsupported": unsupported }),
            )
            .await?;
        body.get("settings")
            .cloned()
            .ok_or_else(|| ConverterError::new("Converter theme-settings answer has no settings."))
    }

    async fn render_elementor(&self, data: &Value) -> Result<String, ConverterError> {
        let body = self
            .post("/__convert/render-elementor", json!({ "data": data }))
            .await?;
        body.get("html")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| ConverterError::new("Converter render-elementor answer has no html."))
    }
}
