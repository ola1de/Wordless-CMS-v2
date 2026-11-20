//! Worker-side HTTP implementations (P3): outbound webhook POSTs
//! and real payment checkouts (Paystack / Flutterwave / Stripe) via
//! `worker::Fetch`. They live in this crate (moved out of
//! wordless-worker in the 2026-11-20 size remedy, so the
//! wordless-woocommerce-worker can resolve payment gateways too);
//! the service crates stay platform-neutral and testable on native
//! targets.
//!
//! The mock gateway lives in `wordless-commerce` (pure, no network).
//! Real providers are implemented from their public API docs and
//! have NEVER been verified live (no keys connected, by design):
//! - Paystack: POST https://api.paystack.co/transaction/initialize
//!   { email, amount (kobo), reference, callback_url }, Bearer secret
//!   → `data.authorization_url`.
//! - Flutterwave: POST https://api.flutterwave.com/v3/payments
//!   { tx_ref, amount (major units), currency, redirect_url, customer,
//!   customizations }, Bearer secret → `data.link`.
//! - Stripe: POST https://api.stripe.com/v1/checkout/sessions,
//!   form-encoded, Basic auth, mode=payment, one line item in minor
//!   units → `url` + `id`.
//!
//! No client-side timeout is applied (pinned `worker` 0.9.0's
//! `AbortSignal` has no timeout constructor); the request is bounded
//! by the Workers runtime limits, same as the AI OpenAI path.

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::Value;
use std::rc::Rc;
use wordless_commerce::payments::{
    CheckoutGateway, PaymentCheckout, PaymentCheckoutInput, PaymentSecrets,
    PAYMENT_PROVIDER_FLUTTERWAVE, PAYMENT_PROVIDER_MOCK, PAYMENT_PROVIDER_PAYSTACK,
    PAYMENT_PROVIDER_STRIPE,
};
use wordless_kernel::error::{internal_error, WordlessError};
use wordless_webhooks::HttpPost;
use worker::{Fetch, Headers, Method, Request, RequestInit};

/// Outbound webhook delivery via Workers `fetch`: POST with the
/// caller-supplied headers (the HMAC signature is set by the
/// webhooks crate) and the JSON body. Returns the HTTP status; any
/// transport failure is an `Err` string (recorded as a failed
/// delivery, never propagated).
pub struct WorkerHttpPost;

/// P6: plain-HTTP transport for extension Workers, used when an
/// instance carries a local-dev `dev_url` (a locally running
/// extension Worker on its own port). The dispatch-namespace path
/// uses the extensions crate's `FetcherTransport` instead.
pub struct WorkerFetchTransport;

#[async_trait(?Send)]
impl wordless_extensions::ExtensionTransport for WorkerFetchTransport {
    async fn post_json(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &str,
    ) -> Result<(u16, String), String> {
        let mut init = RequestInit::new();
        init.with_method(Method::Post);
        let h = Headers::new();
        for (name, value) in headers {
            h.set(name, value)
                .map_err(|e| format!("Failed to build extension request headers: {e}"))?;
        }
        init.with_headers(h);
        init.with_body(Some(wasm_bindgen::JsValue::from_str(body)));
        let req = Request::new_with_init(url, &init)
            .map_err(|e| format!("Failed to build extension request: {e}"))?;
        let mut resp = Fetch::Request(req)
            .send()
            .await
            .map_err(|e| format!("Extension fetch failed: {e}"))?;
        let status = resp.status_code();
        let text = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read extension response: {e}"))?;
        Ok((status, text))
    }

    async fn get_json(&self, url: &str) -> Result<(u16, String), String> {
        let mut init = RequestInit::new();
        init.with_method(Method::Get);
        let req = Request::new_with_init(url, &init)
            .map_err(|e| format!("Failed to build extension health request: {e}"))?;
        let mut resp = Fetch::Request(req)
            .send()
            .await
            .map_err(|e| format!("Extension health fetch failed: {e}"))?;
        let status = resp.status_code();
        let text = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read extension health response: {e}"))?;
        Ok((status, text))
    }
}

#[async_trait(?Send)]
impl HttpPost for WorkerHttpPost {
    async fn post(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<u16, String> {
        let mut init = RequestInit::new();
        init.with_method(Method::Post);
        let h = Headers::new();
        for (name, value) in headers {
            h.set(name, value)
                .map_err(|e| format!("Failed to build webhook request headers: {e}"))?;
        }
        init.with_headers(h);
        init.with_body(Some(wasm_bindgen::JsValue::from(js_sys::Uint8Array::from(
            body,
        ))));
        let req = Request::new_with_init(url, &init)
            .map_err(|e| format!("Failed to build webhook request: {e}"))?;
        let mut resp = Fetch::Request(req)
            .send()
            .await
            .map_err(|e| format!("Webhook delivery failed: {e}"))?;
        // Drain the body so the connection is released.
        let _ = resp.text().await;
        Ok(resp.status_code())
    }
}

/// POST JSON, parse the JSON response. `auth` is the full
/// `Authorization` header value, when the provider needs one.
async fn post_json(
    url: &str,
    auth: Option<&str>,
    content_type: &str,
    body: &str,
) -> Result<(u16, Value), WordlessError> {
    let mut init = RequestInit::new();
    init.with_method(Method::Post);
    let headers = Headers::new();
    headers
        .set("Content-Type", content_type)
        .map_err(|e| internal_error(&format!("Failed to build request headers: {e}")))?;
    if let Some(auth) = auth {
        headers
            .set("Authorization", auth)
            .map_err(|e| internal_error(&format!("Failed to build request headers: {e}")))?;
    }
    init.with_headers(headers);
    init.with_body(Some(wasm_bindgen::JsValue::from_str(body)));
    let req = Request::new_with_init(url, &init)
        .map_err(|e| internal_error(&format!("Failed to build checkout request: {e}")))?;
    let mut resp = Fetch::Request(req)
        .send()
        .await
        .map_err(|e| internal_error(&format!("Checkout request failed: {e}")))?;
    let status = resp.status_code();
    let text = resp
        .text()
        .await
        .map_err(|e| internal_error(&format!("Failed to read checkout response: {e}")))?;
    let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok((status, parsed))
}

fn str_field(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Paystack checkout (v1 `payments/paystack.ts`).
pub struct PaystackGateway {
    secret_key: String,
}

impl PaystackGateway {
    pub fn new(secrets: &PaymentSecrets) -> Option<Self> {
        secrets
            .paystack_secret_key
            .clone()
            .filter(|k| !k.is_empty())
            .map(|secret_key| Self { secret_key })
    }
}

#[async_trait(?Send)]
impl CheckoutGateway for PaystackGateway {
    fn provider_id(&self) -> &'static str {
        PAYMENT_PROVIDER_PAYSTACK
    }

    async fn create_checkout(
        &self,
        input: &PaymentCheckoutInput,
    ) -> Result<PaymentCheckout, WordlessError> {
        let body = serde_json::json!({
            "email": input.customer_email,
            "amount": input.total_minor,
            "reference": input.order_id,
            "callback_url": input.success_url,
        });
        let (status, parsed) = post_json(
            "https://api.paystack.co/transaction/initialize",
            Some(&format!("Bearer {}", self.secret_key)),
            "application/json",
            &serde_json::to_string(&body).unwrap_or_default(),
        )
        .await?;
        let ok = parsed
            .get("status")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !(200..300).contains(&status) || !ok {
            let msg = str_field(&parsed, "message");
            return Err(internal_error(&format!(
                "Paystack request failed (HTTP {status}): {}.",
                if msg.is_empty() {
                    "unknown error"
                } else {
                    &msg
                }
            )));
        }
        let data = parsed.get("data").cloned().unwrap_or(Value::Null);
        let url = str_field(&data, "authorization_url");
        if url.is_empty() {
            return Err(internal_error("Paystack did not return a checkout URL."));
        }
        let reference = str_field(&data, "reference");
        Ok(PaymentCheckout {
            checkout_url: url,
            reference: if reference.is_empty() {
                input.order_id.clone()
            } else {
                reference
            },
        })
    }
}

/// Flutterwave checkout (v1 `payments/flutterwave.ts`). Amounts are
/// major units here (Flutterwave takes 1250.00, not 125000); amounts
/// stored in Wordless stay in minor units.
pub struct FlutterwaveGateway {
    secret_key: String,
}

impl FlutterwaveGateway {
    pub fn new(secrets: &PaymentSecrets) -> Option<Self> {
        secrets
            .flutterwave_secret_key
            .clone()
            .filter(|k| !k.is_empty())
            .map(|secret_key| Self { secret_key })
    }
}

#[async_trait(?Send)]
impl CheckoutGateway for FlutterwaveGateway {
    fn provider_id(&self) -> &'static str {
        PAYMENT_PROVIDER_FLUTTERWAVE
    }

    async fn create_checkout(
        &self,
        input: &PaymentCheckoutInput,
    ) -> Result<PaymentCheckout, WordlessError> {
        let body = serde_json::json!({
            "tx_ref": input.order_id,
            "amount": input.total_minor as f64 / 100.0,
            "currency": input.currency,
            "redirect_url": input.success_url,
            "customer": { "email": input.customer_email, "name": input.customer_name },
            "customizations": { "title": "Wordless store" },
        });
        let (status, parsed) = post_json(
            "https://api.flutterwave.com/v3/payments",
            Some(&format!("Bearer {}", self.secret_key)),
            "application/json",
            &serde_json::to_string(&body).unwrap_or_default(),
        )
        .await?;
        let ok = parsed
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s == "success")
            .unwrap_or(false);
        if !(200..300).contains(&status) || !ok {
            let msg = str_field(&parsed, "message");
            return Err(internal_error(&format!(
                "Flutterwave request failed (HTTP {status}): {}.",
                if msg.is_empty() {
                    "unknown error"
                } else {
                    &msg
                }
            )));
        }
        let data = parsed.get("data").cloned().unwrap_or(Value::Null);
        let url = str_field(&data, "link");
        if url.is_empty() {
            return Err(internal_error("Flutterwave did not return a payment link."));
        }
        let reference = str_field(&data, "tx_ref");
        Ok(PaymentCheckout {
            checkout_url: url,
            reference: if reference.is_empty() {
                input.order_id.clone()
            } else {
                reference
            },
        })
    }
}

/// Stripe checkout (v1 `payments/stripe.ts`): form-encoded session,
/// Basic auth, mode=payment, one line item in minor units.
pub struct StripeGateway {
    secret_key: String,
}

impl StripeGateway {
    pub fn new(secrets: &PaymentSecrets) -> Option<Self> {
        secrets
            .stripe_secret_key
            .clone()
            .filter(|k| !k.is_empty())
            .map(|secret_key| Self { secret_key })
    }
}

#[async_trait(?Send)]
impl CheckoutGateway for StripeGateway {
    fn provider_id(&self) -> &'static str {
        PAYMENT_PROVIDER_STRIPE
    }

    async fn create_checkout(
        &self,
        input: &PaymentCheckoutInput,
    ) -> Result<PaymentCheckout, WordlessError> {
        let form = [
            ("mode", "payment".to_string()),
            ("success_url", input.success_url.clone()),
            ("cancel_url", input.cancel_url.clone()),
            ("client_reference_id", input.order_id.clone()),
            (
                "line_items[0][price_data][currency]",
                input.currency.to_lowercase(),
            ),
            (
                "line_items[0][price_data][product_data][name]",
                format!("Order {}", input.order_id),
            ),
            (
                "line_items[0][price_data][unit_amount]",
                input.total_minor.to_string(),
            ),
            ("line_items[0][quantity]", "1".to_string()),
        ]
        .iter()
        .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
        let basic = BASE64.encode(format!("{}:", self.secret_key));
        let (status, parsed) = post_json(
            "https://api.stripe.com/v1/checkout/sessions",
            Some(&format!("Basic {basic}")),
            "application/x-www-form-urlencoded",
            &form,
        )
        .await?;
        if !(200..300).contains(&status) {
            let msg = parsed
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            return Err(internal_error(&format!(
                "Stripe request failed (HTTP {status}): {msg}."
            )));
        }
        let url = str_field(&parsed, "url");
        let id = str_field(&parsed, "id");
        if url.is_empty() || id.is_empty() {
            return Err(internal_error("Stripe did not return a checkout URL."));
        }
        Ok(PaymentCheckout {
            checkout_url: url,
            reference: id,
        })
    }
}

pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The gateway for the selected provider id, or `None` when the
/// provider is unknown or not configured. The mock gateway is the
/// pure one from `wordless-commerce` (no network, no money).
pub fn gateway_for(provider_id: &str, secrets: &PaymentSecrets) -> Option<Rc<dyn CheckoutGateway>> {
    match provider_id {
        PAYMENT_PROVIDER_PAYSTACK => {
            PaystackGateway::new(secrets).map(|g| Rc::new(g) as Rc<dyn CheckoutGateway>)
        }
        PAYMENT_PROVIDER_FLUTTERWAVE => {
            FlutterwaveGateway::new(secrets).map(|g| Rc::new(g) as Rc<dyn CheckoutGateway>)
        }
        PAYMENT_PROVIDER_STRIPE => {
            StripeGateway::new(secrets).map(|g| Rc::new(g) as Rc<dyn CheckoutGateway>)
        }
        PAYMENT_PROVIDER_MOCK => {
            Some(Rc::new(wordless_commerce::payments::MockCheckoutGateway)
                as Rc<dyn CheckoutGateway>)
        }
        _ => None,
    }
}

/// GET a URL and return its text body. Used for the pinned upstream
/// conversion fixture (and only that, today). Non-2xx is an error;
/// transport failures are errors too — the caller maps them to a 502.
pub async fn fetch_text(url: &str) -> Result<String, String> {
    let mut init = RequestInit::new();
    init.with_method(Method::Get);
    let req =
        Request::new_with_init(url, &init).map_err(|e| format!("Failed to build request: {e}"))?;
    let mut resp = Fetch::Request(req)
        .send()
        .await
        .map_err(|e| format!("Fetch failed: {e}"))?;
    let status = resp.status_code();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("Failed to read response: {e}"))?;
    if !(200..300).contains(&status) {
        return Err(format!("Upstream answered HTTP {status}."));
    }
    Ok(text)
}

/// Fetch WXR attachment files into R2 (the importer's
/// `AttachmentFetcher`, backed by Workers `fetch`). A failed fetch is
/// `None` — the importer records that attachment `skipped`, exactly
/// like the no-fetcher path.
pub struct WorkerAttachmentFetcher;

#[async_trait(?Send)]
impl wordless_importer::service::AttachmentFetcher for WorkerAttachmentFetcher {
    async fn fetch(&self, url: &str) -> Option<(Vec<u8>, String)> {
        let mut init = RequestInit::new();
        init.with_method(Method::Get);
        let req = Request::new_with_init(url, &init).ok()?;
        let mut resp = Fetch::Request(req).send().await.ok()?;
        if !(200..300).contains(&resp.status_code()) {
            return None;
        }
        let content_type = resp
            .headers()
            .get("content-type")
            .ok()
            .flatten()
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let bytes = resp.bytes().await.ok()?;
        Some((bytes, content_type))
    }
}
