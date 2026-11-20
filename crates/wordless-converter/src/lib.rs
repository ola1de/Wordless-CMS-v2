//! WORDLESS v2 converter Worker (split, 2026-11-20).
//!
//! The PHP → TypeScript conversion engine as a standalone stateless
//! Worker. The main Worker outgrew the free-tier 3 MiB WASM limit
//! (P7: 3,204,402 bytes, 101.9%), so the engine moved here, into its
//! own 3 MiB budget, and the main Worker calls it over HTTP.
//!
//! Internal service — the endpoints are NOT part of the public API
//! and carry no auth: reachability is the access control (service
//! binding in production, localhost-only in local dev). Every
//! endpoint speaks plain JSON:
//!
//! - `GET /__convert/health` → `{ "ok": true, "service":
//!   "wordless-converter", "version" }`
//! - `POST /__convert/convert-php` `{ filename, source, slug }` →
//!   `{ "ok": true, "report": ConversionReport }` or `{ "ok": false,
//!   "error", "blocked"? }` (blocked constructs / unparseable files
//!   are application failures, not transport errors — HTTP 200 with
//!   `ok: false`, so the caller can fail the job with the message).
//! - `POST /__convert/analyze-files` `{ files: [{ path, source }] }`
//!   → `{ "ok": true, "analyses": [{ "path", "analysis" | null,
//!   "error" | null }] }` (one round trip for a whole upload).
//! - `POST /__convert/convert-files` `{ files, slug, kind }` →
//!   `{ "ok": true, "generated": [[path, code], ...], "report":
//!   AutoConvertReport, "defaults": [OptionDefault, ...] }`.
//! - `POST /__convert/theme-settings` `{ themeJson, unsupported }` →
//!   `{ "ok": true, "settings": SettingsMigrationReport }`.
//! - `POST /__convert/render-elementor` `{ data }` → `{ "ok": true,
//!   "html" }`.
//!
//! Bad JSON / missing fields → 400 `{ "ok": false, "error" }`.
//! Unknown paths → 404. Nothing here touches D1/R2: the main Worker
//! stores converted output (R2) and reports (D1).

pub mod service;

use serde_json::{json, Value};
use worker::{event, Context, Env, Request, Response, Result};

/// Parse a request body as JSON (`Value::Null` when empty/invalid —
/// the service functions validate the fields they need).
async fn read_json(req: &mut Request) -> Value {
    match req.text().await {
        Ok(t) if !t.trim().is_empty() => serde_json::from_str(&t).unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

fn json_response(status: u16, body: &Value) -> Result<Response> {
    let bytes = serde_json::to_vec(body).unwrap_or_default();
    let mut resp = Response::from_bytes(bytes)?.with_status(status);
    resp.headers_mut().set("Content-Type", "application/json")?;
    Ok(resp)
}

#[event(fetch)]
pub async fn main(mut req: Request, _env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    let path = url.path().to_string();
    let method = req.method().as_ref().to_string();
    let (status, body) = match (method.as_str(), path.as_str()) {
        ("GET", "/__convert/health") => (200, service::health()),
        ("POST", "/__convert/convert-php") => {
            let body = read_json(&mut req).await;
            service::convert_php(&body)
        }
        ("POST", "/__convert/analyze-files") => {
            let body = read_json(&mut req).await;
            service::analyze_files(&body)
        }
        ("POST", "/__convert/convert-files") => {
            let body = read_json(&mut req).await;
            service::convert_files(&body)
        }
        ("POST", "/__convert/theme-settings") => {
            let body = read_json(&mut req).await;
            service::theme_settings(&body)
        }
        ("POST", "/__convert/render-elementor") => {
            let body = read_json(&mut req).await;
            service::render_elementor(&body)
        }
        _ => (
            404,
            json!({ "ok": false, "error": "Unknown converter endpoint." }),
        ),
    };
    json_response(status, &body)
}
