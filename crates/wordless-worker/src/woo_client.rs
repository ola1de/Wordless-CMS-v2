//! Worker-side `WooClient` (size remedy, 2026-11-20): reaches the
//! `wordless-woocommerce` Worker over one of two transports, in order
//! (the converter-client pattern):
//!
//! 1. `WORDLESS_WOO_URL` — plain HTTP to a locally running Woo Worker
//!    (local dev / smoke tests; default `http://127.0.0.1:8808`, see
//!    `scripts/dev-split.sh`).
//! 2. Service binding `WOO` — the free production path.
//!
//! The client speaks the Woo Worker's JSON contract (see
//! `crates/wordless-woocommerce-worker/src/service.rs`): every
//! endpoint answers `{ "ok": true, ... }` or `{ "ok": false,
//! "error", "code", "status" }`. An application failure carries the
//! Woo Worker's code + status, so the caller re-raises the exact
//! error the shopper would have seen before the split.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::rc::Rc;
use wordless_kernel::woo_client::{WooClient, WooError};
use worker::{Fetch, Fetcher, Headers, Method, Request, RequestInit};

/// One Woo transport: a service-binding `Fetcher` (production) or a
/// plain-HTTP base URL (local dev).
enum Transport {
    /// Service binding; requests go to the fake internal origin (the
    /// binding, not the URL, decides the destination — the P6 pattern).
    Fetcher(Fetcher),
    /// Local-dev base URL, e.g. `http://127.0.0.1:8808`.
    Url(String),
}

/// The worker-side Woo client.
pub struct WorkerWooClient {
    transport: Transport,
}

impl WorkerWooClient {
    /// Service-binding transport (production).
    pub fn via_service(fetcher: Fetcher) -> Rc<dyn WooClient> {
        Rc::new(Self {
            transport: Transport::Fetcher(fetcher),
        }) as Rc<dyn WooClient>
    }

    /// Plain-HTTP transport (local dev).
    pub fn via_url(base_url: String) -> Rc<dyn WooClient> {
        Rc::new(Self {
            transport: Transport::Url(base_url.trim_end_matches('/').to_string()),
        }) as Rc<dyn WooClient>
    }

    fn url_for(&self, path: &str) -> String {
        match &self.transport {
            Transport::Fetcher(_) => format!("https://woo-wordless.internal{path}"),
            Transport::Url(base) => format!("{base}{path}"),
        }
    }

    /// POST JSON to one Woo endpoint. Returns the response body
    /// (already checked for `"ok": true`); application failures
    /// become a `WooError` with the Woo Worker's code + status.
    async fn post(&self, path: &str, body: Value) -> Result<Value, WooError> {
        let url = self.url_for(path);
        let mut init = RequestInit::new();
        init.with_method(Method::Post);
        let headers = Headers::new();
        headers
            .set("Content-Type", "application/json")
            .map_err(|e| WooError::new(format!("Failed to build Woo request headers: {e}")))?;
        init.with_headers(headers);
        init.with_body(Some(wasm_bindgen::JsValue::from_str(
            &serde_json::to_string(&body).unwrap_or_default(),
        )));
        let req = Request::new_with_init(&url, &init)
            .map_err(|e| WooError::new(format!("Failed to build Woo request: {e}")))?;
        let mut resp = match &self.transport {
            Transport::Fetcher(fetcher) => fetcher
                .fetch_request(req)
                .await
                .map_err(|e| WooError::new(format!("Woo service fetch failed: {e}")))?,
            Transport::Url(_) => Fetch::Request(req)
                .send()
                .await
                .map_err(|e| WooError::new(format!("Woo service fetch failed: {e}")))?,
        };
        let status = resp.status_code();
        let text = resp
            .text()
            .await
            .map_err(|e| WooError::new(format!("Failed to read Woo response: {e}")))?;
        if !(200..300).contains(&status) {
            return Err(WooError::new(format!(
                "Woo service answered HTTP {status} for {path}."
            )));
        }
        let parsed: Value = serde_json::from_str(&text)
            .map_err(|e| WooError::new(format!("Woo service returned non-JSON for {path}: {e}")))?;
        if parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
            return Ok(parsed);
        }
        let message = parsed
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("Woo service failed without a message.")
            .to_string();
        let code = parsed
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("internal_error")
            .to_string();
        let status = parsed.get("status").and_then(|v| v.as_u64()).unwrap_or(500) as u16;
        Err(WooError::with_code(message, code, status))
    }
}

#[async_trait(?Send)]
impl WooClient for WorkerWooClient {
    async fn resolve_variation(
        &self,
        product_id: i64,
        form: &Value,
    ) -> Result<Option<i64>, WooError> {
        let body = self
            .post(
                "/__woo/resolve-variation",
                json!({ "productId": product_id, "form": form }),
            )
            .await?;
        Ok(body.get("variationId").and_then(|v| v.as_i64()))
    }

    async fn get_product(&self, product_id: i64) -> Result<Value, WooError> {
        let body = self
            .post("/__woo/get-product", json!({ "productId": product_id }))
            .await?;
        Ok(body.get("product").cloned().unwrap_or(Value::Null))
    }

    async fn zones(&self) -> Result<Value, WooError> {
        let body = self.post("/__woo/zones", json!({})).await?;
        Ok(body.get("zones").cloned().unwrap_or(Value::Null))
    }

    async fn totals(
        &self,
        cart_token: &str,
        totals_input: &Value,
        strict: bool,
    ) -> Result<Value, WooError> {
        let body = self
            .post(
                "/__woo/totals",
                json!({ "cartToken": cart_token, "totalsInput": totals_input, "strict": strict }),
            )
            .await?;
        body.get("totals")
            .cloned()
            .ok_or_else(|| WooError::new("Woo totals answer has no totals."))
    }

    async fn shop_checkout(
        &self,
        cart_token: &str,
        customer_name: &Value,
        customer_email: &Value,
        totals_input: &Value,
        base: &str,
        success_url: &str,
        cancel_url: &str,
    ) -> Result<Value, WooError> {
        self.post(
            "/__woo/shop-checkout",
            json!({
                "cartToken": cart_token,
                "customerName": customer_name,
                "customerEmail": customer_email,
                "totalsInput": totals_input,
                "base": base,
                "successUrl": success_url,
                "cancelUrl": cancel_url,
            }),
        )
        .await
    }
}
