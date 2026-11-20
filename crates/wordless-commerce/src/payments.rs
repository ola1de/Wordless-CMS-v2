//! Payment providers (port of v1 `src/modules/commerce/payments/`).
//!
//! Every provider implements the same three operations:
//! - create checkout: start a payment for a pending order → a URL the
//!   customer is sent to + a reference stored on the order.
//! - verify payment: ask the provider what happened to a reference.
//! - verify webhook: prove a webhook really came from the provider.
//!   The signature IS the auth for webhooks — there is no session.
//!
//! Honesty rules: a provider with no secret key reports configured:
//! false and checkout refuses; nothing here ever marks an order paid
//! without a verified signal. The mock provider is test-mode only and
//! says so in every message.
//!
//! Differences from v1 (same behaviour, Rust-shaped):
//! - HMAC is synchronous pure Rust (`hmac` crate), not WebCrypto, so
//!   the verifiers are plain `fn`s, not `async fn`s. The outputs are
//!   identical (checked against the v1 test vectors, themselves
//!   cross-checked against Python's `hmac` module).
//! - Webhook headers arrive as `&[(&str, &str)]` pairs (the worker
//!   converts its `Headers`); the Stripe verifier takes the current
//!   Unix time explicitly so the 300 s tolerance is testable.
//! - Secrets arrive as a `PaymentSecrets` struct the worker fills from
//!   env/secrets bindings. This crate never reads `Env` directly.
//! - The real providers' checkout HTTP calls need `fetch`, which only
//!   exists in the Worker. `CheckoutGateway` is the seam: this crate
//!   implements it for the mock provider; the worker implements it for
//!   Paystack / Flutterwave / Stripe with `worker::Fetch`.

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use serde::Serialize;
use serde_json::Value;
use sha2::{Sha256, Sha512};
use wordless_db::options::{get_option, set_option};
use wordless_db::row::row_int;
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, WordlessError};

use crate::orders::{create_order_from_cart, set_order_payment};
use crate::types::Order;

/// Provider ids (v1 `PAYMENT_PROVIDER_IDS`).
pub const PAYMENT_PROVIDER_PAYSTACK: &str = "paystack";
/// Provider ids (v1 `PAYMENT_PROVIDER_IDS`).
pub const PAYMENT_PROVIDER_FLUTTERWAVE: &str = "flutterwave";
/// Provider ids (v1 `PAYMENT_PROVIDER_IDS`).
pub const PAYMENT_PROVIDER_STRIPE: &str = "stripe";
/// Provider ids (v1 `PAYMENT_PROVIDER_IDS`).
pub const PAYMENT_PROVIDER_MOCK: &str = "mock";

/// `wp_options` key for the selected provider.
pub const PAYMENT_PROVIDER_OPTION: &str = "wl_payment_provider";
/// `wp_options` key for the payment mode.
pub const PAYMENT_MODE_OPTION: &str = "wl_payment_mode";
/// Test mode.
pub const PAYMENT_MODE_TEST: &str = "test";
/// Live mode.
pub const PAYMENT_MODE_LIVE: &str = "live";

/// Stripe webhook timestamp tolerance, in seconds (v1
/// `SIGNATURE_TOLERANCE_SECONDS`).
pub const STRIPE_SIGNATURE_TOLERANCE_SECONDS: i64 = 300;

/// True when `value` names a known payment provider.
pub fn is_payment_provider_id(value: &str) -> bool {
    matches!(
        value,
        PAYMENT_PROVIDER_PAYSTACK
            | PAYMENT_PROVIDER_FLUTTERWAVE
            | PAYMENT_PROVIDER_STRIPE
            | PAYMENT_PROVIDER_MOCK
    )
}

/// Secrets for the payment providers, filled by the worker from
/// env/secrets bindings. Never stored in D1, never logged, never put
/// in an error message.
#[derive(Debug, Clone, Default)]
pub struct PaymentSecrets {
    pub paystack_secret_key: Option<String>,
    pub flutterwave_secret_key: Option<String>,
    pub flutterwave_secret_hash: Option<String>,
    pub stripe_secret_key: Option<String>,
    pub stripe_webhook_secret: Option<String>,
    pub mock_webhook_secret: Option<String>,
}

impl PaymentSecrets {
    /// All secrets unset (nothing configured).
    pub fn empty() -> Self {
        Self::default()
    }
}

/// Error type for "no payment provider is configured" (checkout → 503,
/// never a fake success). Port of v1 `paymentNotConfigured`.
pub fn payment_not_configured(message: &str) -> WordlessError {
    WordlessError::new("payment_not_configured", 503, message)
}

// ---------------------------------------------------------------- HMAC

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// HMAC-SHA256 hex digest. Port of v1 `hmacHex("SHA-256", …)`.
pub fn hmac_hex_sha256(key: &str, data: &str) -> String {
    // HMAC accepts keys of any length, so `new_from_slice` cannot fail.
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC accepts keys of any length");
    mac.update(data.as_bytes());
    hex_encode(&mac.finalize().into_bytes())
}

/// HMAC-SHA512 hex digest. Port of v1 `hmacHex("SHA-512", …)`.
pub fn hmac_hex_sha512(key: &str, data: &str) -> String {
    // HMAC accepts keys of any length, so `new_from_slice` cannot fail.
    let mut mac =
        Hmac::<Sha512>::new_from_slice(key.as_bytes()).expect("HMAC accepts keys of any length");
    mac.update(data.as_bytes());
    hex_encode(&mac.finalize().into_bytes())
}

/// Constant-time string comparison (same length required, no early exit
/// on content). Port of v1 `timingSafeEqual`.
pub fn timing_safe_equal(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

// ---------------------------------------------------------------- webhook verification

/// The result of verifying a provider webhook.
#[derive(Debug, Clone, PartialEq)]
pub struct WebhookResult {
    /// The signature checked out.
    pub valid: bool,
    /// Provider event name (`""` when the body is not parseable).
    pub event: String,
    /// Order/payment reference from the payload, if any.
    pub reference: Option<String>,
    /// True only when the provider says the payment succeeded.
    pub paid: bool,
    /// Plain-language explanation for the webhook log.
    pub detail: String,
}

fn header<'a>(headers: &'a [(&'a str, &'a str)], name: &str) -> &'a str {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| *v)
        .unwrap_or("")
}

fn parse_body(raw_body: &str) -> Value {
    serde_json::from_str::<Value>(raw_body).unwrap_or(Value::Object(Default::default()))
}

fn body_event(body: &Value, key: &str) -> String {
    body.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn body_data(body: &Value) -> &Value {
    body.get("data").unwrap_or(&Value::Null)
}

fn data_str(data: &Value, key: &str) -> Option<String> {
    data.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn invalid(detail: &str) -> WebhookResult {
    WebhookResult {
        valid: false,
        event: String::new(),
        reference: None,
        paid: false,
        detail: detail.to_string(),
    }
}

/// Paystack webhook: HMAC-SHA512 of the raw request body with the
/// secret key, sent as the `x-paystack-signature` header; event
/// `charge.success` with `data.status` `"success"` means paid.
pub fn verify_paystack_webhook(
    secrets: &PaymentSecrets,
    raw_body: &str,
    headers: &[(&str, &str)],
) -> WebhookResult {
    let Some(key) = secrets
        .paystack_secret_key
        .as_deref()
        .filter(|k| !k.is_empty())
    else {
        return invalid("Paystack is not configured (no secret key).");
    };
    let signature = header(headers, "x-paystack-signature");
    let expected = hmac_hex_sha512(key, raw_body);
    if signature.is_empty() || !timing_safe_equal(signature, &expected) {
        return invalid("Paystack webhook signature does not match.");
    }
    let body = parse_body(raw_body);
    let event = body_event(&body, "event");
    let data = body_data(&body);
    let reference = data_str(data, "reference");
    let paid = event == "charge.success"
        && data_str(data, "status").as_deref() == Some("success")
        && reference.is_some();
    WebhookResult {
        valid: true,
        event: event.clone(),
        reference,
        paid,
        detail: if paid {
            "Paystack charge succeeded.".to_string()
        } else {
            format!(
                "Paystack event \"{event}\" (status {}) is not a successful charge.",
                data.get("status")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".to_string())
            )
        },
    }
}

/// Flutterwave webhook: Flutterwave sends the secret hash set in the
/// dashboard as the `verif-hash` header — verification is comparing
/// that header to the configured `FLUTTERWAVE_SECRET_HASH` (this is
/// the scheme in Flutterwave's webhook documentation; it is not an
/// HMAC). Event `charge.completed` with `data.status` `"successful"`
/// means paid.
pub fn verify_flutterwave_webhook(
    secrets: &PaymentSecrets,
    raw_body: &str,
    headers: &[(&str, &str)],
) -> WebhookResult {
    let Some(hash) = secrets
        .flutterwave_secret_hash
        .as_deref()
        .filter(|k| !k.is_empty())
    else {
        return invalid(
            "Flutterwave webhook hash is not configured (set FLUTTERWAVE_SECRET_HASH — the same hash set in the Flutterwave dashboard).",
        );
    };
    let sent = header(headers, "verif-hash");
    if sent.is_empty() || !timing_safe_equal(sent, hash) {
        return invalid("Flutterwave verif-hash does not match the configured secret hash.");
    }
    let body = parse_body(raw_body);
    let event = body_event(&body, "event");
    let data = body_data(&body);
    let reference = data_str(data, "tx_ref");
    let paid = event == "charge.completed"
        && data_str(data, "status").as_deref() == Some("successful")
        && reference.is_some();
    WebhookResult {
        valid: true,
        event: event.clone(),
        reference,
        paid,
        detail: if paid {
            "Flutterwave charge completed successfully.".to_string()
        } else {
            format!(
                "Flutterwave event \"{event}\" (status {}) is not a successful charge.",
                data.get("status")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".to_string())
            )
        },
    }
}

/// Stripe webhook: the `stripe-signature` header carries
/// `t=<timestamp>, v1=<hmac>`; the signature is HMAC-SHA256 of
/// `${t}.${rawBody}` with the webhook secret, and the timestamp must
/// be within 300 seconds of now (replay protection). Event
/// `checkout.session.completed` with `payment_status` `"paid"` means
/// paid.
pub fn verify_stripe_webhook(
    secrets: &PaymentSecrets,
    raw_body: &str,
    headers: &[(&str, &str)],
    now_unix_secs: i64,
) -> WebhookResult {
    let Some(secret) = secrets
        .stripe_webhook_secret
        .as_deref()
        .filter(|k| !k.is_empty())
    else {
        return invalid("Stripe webhook secret is not configured (set STRIPE_WEBHOOK_SECRET).");
    };
    let sig_header = header(headers, "stripe-signature");
    let mut timestamp = "";
    let mut signature = "";
    for part in sig_header.split(',') {
        if let Some((k, v)) = part.split_once('=') {
            match k {
                "t" => timestamp = v,
                "v1" => signature = v,
                _ => {}
            }
        }
    }
    if timestamp.is_empty() || signature.is_empty() {
        return invalid("Stripe signature header is missing its timestamp or v1 signature.");
    }
    let ts: i64 = match timestamp.parse() {
        Ok(n) => n,
        Err(_) => {
            return invalid(
                "Stripe webhook timestamp is outside the 300-second tolerance (possible replay).",
            );
        }
    };
    if (now_unix_secs - ts).abs() > STRIPE_SIGNATURE_TOLERANCE_SECONDS {
        return invalid(
            "Stripe webhook timestamp is outside the 300-second tolerance (possible replay).",
        );
    }
    let expected = hmac_hex_sha256(secret, &format!("{timestamp}.{raw_body}"));
    if !timing_safe_equal(signature, &expected) {
        return invalid("Stripe webhook signature does not match.");
    }
    let body = parse_body(raw_body);
    let event = body_event(&body, "type");
    let data = body_data(&body);
    let object = data.get("object").unwrap_or(&Value::Null);
    let reference = data_str(object, "id");
    let paid = event == "checkout.session.completed"
        && data_str(object, "payment_status").as_deref() == Some("paid")
        && reference.is_some();
    WebhookResult {
        valid: true,
        event: event.clone(),
        reference,
        paid,
        detail: if paid {
            "Stripe checkout session paid.".to_string()
        } else {
            format!(
                "Stripe event \"{event}\" (payment_status {}) is not a completed payment.",
                object
                    .get("payment_status")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".to_string())
            )
        },
    }
}

/// Mock webhook (test mode only): HMAC-SHA256 of the raw body with
/// `MOCK_PAYMENT_WEBHOOK_SECRET`, sent as `x-mock-signature`. When
/// the secret is unset, every mock webhook is rejected — there is no
/// signature bypass anywhere. Event `payment.paid` means paid.
pub fn verify_mock_webhook(
    secrets: &PaymentSecrets,
    raw_body: &str,
    headers: &[(&str, &str)],
) -> WebhookResult {
    let Some(secret) = secrets
        .mock_webhook_secret
        .as_deref()
        .filter(|k| !k.is_empty())
    else {
        return invalid(
            "Mock webhook secret is not configured (set MOCK_PAYMENT_WEBHOOK_SECRET). No mock webhook is accepted without it.",
        );
    };
    let signature = header(headers, "x-mock-signature");
    let expected = hmac_hex_sha256(secret, raw_body);
    if signature.is_empty() || !timing_safe_equal(signature, &expected) {
        return invalid("Mock webhook signature does not match.");
    }
    let body = parse_body(raw_body);
    let event = body_event(&body, "event");
    let reference = body
        .get("reference")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let paid = event == "payment.paid" && reference.is_some();
    WebhookResult {
        valid: true,
        event: event.clone(),
        reference,
        paid,
        detail: if paid {
            "Mock test payment confirmed.".to_string()
        } else {
            format!("Mock webhook event \"{event}\" is not a payment confirmation.")
        },
    }
}

/// Verify a webhook for the named provider. Unknown provider ids are a
/// 400 (`Unknown payment provider "…"`) — the same error v1 throws.
pub fn verify_webhook(
    provider_id: &str,
    secrets: &PaymentSecrets,
    raw_body: &str,
    headers: &[(&str, &str)],
    now_unix_secs: i64,
) -> Result<WebhookResult, WordlessError> {
    match provider_id {
        PAYMENT_PROVIDER_PAYSTACK => Ok(verify_paystack_webhook(secrets, raw_body, headers)),
        PAYMENT_PROVIDER_FLUTTERWAVE => Ok(verify_flutterwave_webhook(secrets, raw_body, headers)),
        PAYMENT_PROVIDER_STRIPE => Ok(verify_stripe_webhook(
            secrets,
            raw_body,
            headers,
            now_unix_secs,
        )),
        PAYMENT_PROVIDER_MOCK => Ok(verify_mock_webhook(secrets, raw_body, headers)),
        other => Err(bad_request(&format!(
            "Unknown payment provider \"{other}\"."
        ))),
    }
}

// ---------------------------------------------------------------- provider registry

/// One provider's public status (no secrets).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub requires_key: bool,
    pub configured: bool,
    pub configured_detail: String,
}

fn provider_meta(id: &str) -> (&'static str, &'static str, bool) {
    match id {
        PAYMENT_PROVIDER_PAYSTACK => (
            "Paystack",
            "Paystack — cards, bank transfer and mobile money (NGN and more). Needs PAYSTACK_SECRET_KEY.",
            true,
        ),
        PAYMENT_PROVIDER_FLUTTERWAVE => (
            "Flutterwave",
            "Flutterwave — cards, bank transfer and mobile money across Africa. Needs FLUTTERWAVE_SECRET_KEY and FLUTTERWAVE_SECRET_HASH.",
            true,
        ),
        PAYMENT_PROVIDER_STRIPE => (
            "Stripe",
            "Stripe — cards and wallets worldwide. Needs STRIPE_SECRET_KEY and STRIPE_WEBHOOK_SECRET.",
            true,
        ),
        _ => (
            "Mock (test mode only)",
            "Local development and tests only — no money moves. Checkout opens a test page on this site; the webhook is HMAC-verified with MOCK_PAYMENT_WEBHOOK_SECRET.",
            false,
        ),
    }
}

/// True only when a checkout call could succeed right now. The mock is
/// always configured; the real providers need their secret key.
pub fn is_configured(provider_id: &str, secrets: &PaymentSecrets) -> bool {
    match provider_id {
        PAYMENT_PROVIDER_PAYSTACK => secrets
            .paystack_secret_key
            .as_deref()
            .is_some_and(|k| !k.is_empty()),
        PAYMENT_PROVIDER_FLUTTERWAVE => secrets
            .flutterwave_secret_key
            .as_deref()
            .is_some_and(|k| !k.is_empty()),
        PAYMENT_PROVIDER_STRIPE => secrets
            .stripe_secret_key
            .as_deref()
            .is_some_and(|k| !k.is_empty()),
        PAYMENT_PROVIDER_MOCK => true,
        _ => false,
    }
}

/// Plain-language configured state (never includes a secret).
pub fn configured_detail(provider_id: &str, secrets: &PaymentSecrets) -> String {
    if is_configured(provider_id, secrets) {
        match provider_id {
            PAYMENT_PROVIDER_PAYSTACK => {
                "Secret key is set (wrangler secret). Webhooks are verified with HMAC-SHA512."
                    .to_string()
            }
            PAYMENT_PROVIDER_FLUTTERWAVE => {
                "Secret key is set (wrangler secret). Webhooks are verified against the FLUTTERWAVE_SECRET_HASH you set in the Flutterwave dashboard."
                    .to_string()
            }
            PAYMENT_PROVIDER_STRIPE => {
                "Secret key is set (wrangler secret). Webhooks are verified with HMAC-SHA256 against STRIPE_WEBHOOK_SECRET (300 s tolerance)."
                    .to_string()
            }
            _ => "Always available in local development and tests. No real payment is ever processed."
                .to_string(),
        }
    } else {
        match provider_id {
            PAYMENT_PROVIDER_PAYSTACK => {
                "Not configured: set PAYSTACK_SECRET_KEY with `wrangler secret put PAYSTACK_SECRET_KEY`."
                    .to_string()
            }
            PAYMENT_PROVIDER_FLUTTERWAVE => {
                "Not configured: set FLUTTERWAVE_SECRET_KEY and FLUTTERWAVE_SECRET_HASH with `wrangler secret put`."
                    .to_string()
            }
            PAYMENT_PROVIDER_STRIPE => {
                "Not configured: set STRIPE_SECRET_KEY and STRIPE_WEBHOOK_SECRET with `wrangler secret put`."
                    .to_string()
            }
            _ => "Mock is always configured.".to_string(),
        }
    }
}

/// All four providers with their configured state (admin view).
pub fn list_payment_providers(secrets: &PaymentSecrets) -> Vec<ProviderInfo> {
    [
        PAYMENT_PROVIDER_PAYSTACK,
        PAYMENT_PROVIDER_FLUTTERWAVE,
        PAYMENT_PROVIDER_STRIPE,
        PAYMENT_PROVIDER_MOCK,
    ]
    .iter()
    .map(|id| {
        let (name, description, requires_key) = provider_meta(id);
        ProviderInfo {
            id: id.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            requires_key,
            configured: is_configured(id, secrets),
            configured_detail: configured_detail(id, secrets),
        }
    })
    .collect()
}

// ---------------------------------------------------------------- selection

/// The selected payment provider id, or `None` when none is selected
/// (or the stored value is not a known provider).
pub async fn get_selected_provider_id(db: &dyn Db) -> Result<Option<String>, WordlessError> {
    let selected: String = get_option(db, PAYMENT_PROVIDER_OPTION, String::new()).await?;
    if is_payment_provider_id(&selected) {
        Ok(Some(selected))
    } else {
        Ok(None)
    }
}

/// Select a provider (`None` / empty clears the selection). Returns the
/// stored selection.
pub async fn set_selected_provider_id(
    db: &dyn Db,
    value: Option<&str>,
) -> Result<Option<String>, WordlessError> {
    match value {
        None | Some("") => {
            set_option(db, PAYMENT_PROVIDER_OPTION, &Option::<String>::None).await?;
            Ok(None)
        }
        Some(v) if is_payment_provider_id(v) => {
            set_option(db, PAYMENT_PROVIDER_OPTION, &Some(v.to_string())).await?;
            Ok(Some(v.to_string()))
        }
        Some(_) => Err(bad_request(
            "Field \"provider\" must be one of: paystack, flutterwave, stripe, mock (or empty to clear).",
        )),
    }
}

/// The payment mode: `"live"` only when explicitly set, else `"test"`.
pub async fn get_payment_mode(db: &dyn Db) -> Result<String, WordlessError> {
    let mode: String = get_option(db, PAYMENT_MODE_OPTION, PAYMENT_MODE_TEST.to_string()).await?;
    Ok(if mode == PAYMENT_MODE_LIVE {
        PAYMENT_MODE_LIVE.to_string()
    } else {
        PAYMENT_MODE_TEST.to_string()
    })
}

/// Set the payment mode (`"test"` or `"live"`).
pub async fn set_payment_mode(db: &dyn Db, value: &str) -> Result<String, WordlessError> {
    if value != PAYMENT_MODE_TEST && value != PAYMENT_MODE_LIVE {
        return Err(bad_request("Field \"mode\" must be \"test\" or \"live\"."));
    }
    set_option(db, PAYMENT_MODE_OPTION, &value.to_string()).await?;
    Ok(value.to_string())
}

/// The selected provider, or a 503 that says plainly none is selected.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedProvider {
    pub provider_id: String,
    pub mode: String,
}

pub async fn resolve_payment_provider(
    db: &dyn Db,
    secrets: &PaymentSecrets,
) -> Result<ResolvedProvider, WordlessError> {
    let selected = get_selected_provider_id(db).await?;
    let Some(selected) = selected else {
        return Err(payment_not_configured(
            "No payment provider is selected. Choose one in Commerce → Payments (a test-mode provider works for local development; no order is ever marked paid without a configured provider).",
        ));
    };
    if !is_configured(&selected, secrets) {
        let (name, _, _) = provider_meta(&selected);
        return Err(payment_not_configured(&format!(
            "{name} is selected but not configured: {}",
            configured_detail(&selected, secrets)
        )));
    }
    Ok(ResolvedProvider {
        provider_id: selected,
        mode: get_payment_mode(db).await?,
    })
}

// ---------------------------------------------------------------- checkout

/// Input for starting a payment (v1 `PaymentCheckoutInput`).
#[derive(Debug, Clone)]
pub struct PaymentCheckoutInput {
    pub order_id: String,
    pub customer_name: String,
    pub customer_email: String,
    pub total_minor: i64,
    pub currency: String,
    pub site_url: String,
    /// Where the provider sends the customer after a successful payment.
    pub success_url: String,
    /// Where the provider sends the customer when they abandon payment.
    pub cancel_url: String,
}

impl PaymentCheckoutInput {
    /// Build checkout input from a pending order and the redirect URLs.
    pub fn from_order(order: &Order, site_url: &str, success_url: &str, cancel_url: &str) -> Self {
        Self {
            order_id: order.id.clone(),
            customer_name: order.customer_name.clone(),
            customer_email: order.customer_email.clone(),
            total_minor: order.total_minor,
            currency: order.currency.clone(),
            site_url: site_url.to_string(),
            success_url: success_url.to_string(),
            cancel_url: cancel_url.to_string(),
        }
    }
}

/// A started payment (v1 `PaymentCheckout`): the URL the customer is
/// redirected to, and the reference stored on the order.
#[derive(Debug, Clone, PartialEq)]
pub struct PaymentCheckout {
    pub checkout_url: String,
    pub reference: String,
}

/// What a provider reports about a reference (v1 `PaymentVerification`).
#[derive(Debug, Clone, PartialEq)]
pub struct PaymentVerification {
    pub status: String,
    /// Provider's own status string, for the webhook log.
    pub raw_status: Option<String>,
}

/// The seam for starting a real payment. The mock gateway is
/// implemented in this crate (pure, no network); the worker implements
/// this trait for Paystack / Flutterwave / Stripe with `worker::Fetch`
/// (initialise / create-payment / Checkout Session calls). The trait
/// is `?Send` because Worker futures are `!Send` (same rule as the
/// `Db` trait).
#[async_trait(?Send)]
pub trait CheckoutGateway {
    /// The provider this gateway starts payments for.
    fn provider_id(&self) -> &'static str;
    /// Start a payment for a pending order.
    async fn create_checkout(
        &self,
        input: &PaymentCheckoutInput,
    ) -> Result<PaymentCheckout, WordlessError>;
}

/// Mock payment provider (port of v1 `payments/mock.ts`) — test mode
/// only. Deterministic, no network, no money. `create_checkout`
/// returns a test-mode page on this same site
/// (`/shop/checkout/test`) — never a fake external checkout URL.
pub struct MockCheckoutGateway;

#[async_trait(?Send)]
impl CheckoutGateway for MockCheckoutGateway {
    fn provider_id(&self) -> &'static str {
        PAYMENT_PROVIDER_MOCK
    }

    async fn create_checkout(
        &self,
        input: &PaymentCheckoutInput,
    ) -> Result<PaymentCheckout, WordlessError> {
        let reference = format!("mock_{}", input.order_id);
        // Order ids are UUIDs, so no URL-encoding is needed here.
        let checkout_url = format!(
            "{}/shop/checkout/test?reference={}&order={}",
            input.site_url, reference, input.order_id
        );
        Ok(PaymentCheckout {
            checkout_url,
            reference,
        })
    }
}

/// The mock's source of truth is the webhook log: paid only when a
/// verified mock webhook for this reference was accepted. Port of v1
/// mock `verifyPayment`.
pub async fn mock_verify_payment(
    db: &dyn Db,
    reference: &str,
) -> Result<PaymentVerification, WordlessError> {
    let row: Option<Row> = db
        .prepare(
            "SELECT COUNT(*) AS n FROM wl_webhook_log WHERE provider = 'mock' AND reference = ? AND result = 'accepted'",
        )
        .bind(vec![Param::Text(reference.to_string())])
        .first()
        .await?;
    let n = row.as_ref().map(|r| row_int(r, "n")).unwrap_or(0);
    if n > 0 {
        Ok(PaymentVerification {
            status: "paid".to_string(),
            raw_status: Some("accepted".to_string()),
        })
    } else {
        Ok(PaymentVerification {
            status: "pending".to_string(),
            raw_status: Some("no-webhook-yet".to_string()),
        })
    }
}

/// The result of a successful checkout: the order (now carrying its
/// provider + reference) and where to send the customer.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutOutcome {
    pub order: Order,
    pub checkout_url: String,
    pub reference: String,
}

/// Append `?order=<id>` (or `&order=<id>` when the URL already has
/// a query string) to a provider return URL.
fn append_order_query(url: &str, order_id: &str) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}order={order_id}")
}

/// Start a checkout: create a pending order from the cart, resolve the
/// selected provider, start the payment through `gateway`, and record
/// the provider + reference on the order.
///
/// - With no provider selected, or the selected provider
///   unconfigured, this returns the typed `payment_not_configured`
///   error (HTTP 503) — never a fake success. The pending order stays
///   pending with an empty `payment_reference`, exactly as in v1.
/// - `gateway` must be the gateway for the selected provider; a
///   mismatch is a wiring bug and returns a 500.
/// - A gateway failure after the order is created leaves the pending
///   order in place (same as v1: the route throws, the order stays).
#[allow(clippy::too_many_arguments)]
pub async fn start_checkout(
    db: &dyn Db,
    secrets: &PaymentSecrets,
    gateway: &dyn CheckoutGateway,
    cart_token: &str,
    customer_name: &Value,
    customer_email: &Value,
    site_url: &str,
    success_url: &str,
    cancel_url: &str,
) -> Result<CheckoutOutcome, WordlessError> {
    let order = create_order_from_cart(db, cart_token, customer_name, customer_email).await?;
    let resolved = resolve_payment_provider(db, secrets).await?;
    if gateway.provider_id() != resolved.provider_id {
        return Err(WordlessError::new(
            "internal_error",
            500,
            "Checkout gateway does not match the selected payment provider.",
        ));
    }
    // v1 appends `?order=<id>` to both return URLs so the
    // success/cancel pages can load the order afterwards.
    let success_url = append_order_query(success_url, &order.id);
    let cancel_url = append_order_query(cancel_url, &order.id);
    let input = PaymentCheckoutInput::from_order(&order, site_url, &success_url, &cancel_url);
    let checkout = gateway.create_checkout(&input).await?;
    let updated =
        set_order_payment(db, &order.id, &resolved.provider_id, &checkout.reference).await?;
    Ok(CheckoutOutcome {
        order: updated,
        checkout_url: checkout.checkout_url,
        reference: checkout.reference,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cart::{add_to_cart, new_cart_token};
    use crate::products::create_product;
    use crate::test_support::SqliteDb;
    use crate::types::ProductInput;
    use futures_executor::block_on;
    use serde_json::json;

    // Ground-truth HMAC values, computed independently with Python's
    // `hmac` module (not with this crate, not with Node):
    //   hmac.new(b"test-key", b"the quick brown fox", hashlib.sha256).hexdigest()
    //   hmac.new(b"test-key", b"the quick brown fox", hashlib.sha512).hexdigest()
    const SHA256_FOX: &str = "c7ff94e639a1cd39b85aeb409029791600506736fe59570fdab1a3470db19769";
    const SHA512_FOX: &str = "913e8470436d460893318bbf66d3582f201bb914b62bf1315df89c4a243ad079f94140048c5eda9bc27ab7447532eb5802f431ef02245208e64fb9d96fba52a5";
    // Paystack body (exact v1 `payments.test.ts` payload, JSON.stringify
    // order) + its Python-computed HMAC-SHA512 with sk_test_paystack_secret.
    const PAYSTACK_BODY: &str = r#"{"event":"charge.success","data":{"reference":"order-1","status":"success","amount":125000}}"#;
    const PAYSTACK_SIG: &str = "191e39107bb8aabf3994e8844ef66c93be4d8eb434c7625d026d446f4fc9a9fa3e469a59ccb5c84c04abf375c0969ed07fe0aafa798234da794cc84c514e0d7f";
    // Mock body (exact v1 payload) + Python-computed HMAC-SHA256 with s3cret.
    const MOCK_BODY: &str = r#"{"event":"payment.paid","reference":"mock_order-1"}"#;
    const MOCK_SIG: &str = "dedd3d479b23b1388ef7032ef63ec401c7188d8fc7653e0028c129283d961ad7";
    // Stripe body (exact v1 payload); the signed payload
    // `1700000000.<body>` HMAC-SHA256 with whsec_test, Python-computed.
    const STRIPE_BODY: &str = r#"{"type":"checkout.session.completed","data":{"object":{"id":"cs_test_123","payment_status":"paid"}}}"#;
    const STRIPE_SIGNED_PAYLOAD_SIG: &str =
        "ae8e185e65efd0ef539b044d7073db1605c23d6ea3dd597666ffae90eca31ff7";

    fn paystack_secrets() -> PaymentSecrets {
        PaymentSecrets {
            paystack_secret_key: Some("sk_test_paystack_secret".to_string()),
            ..Default::default()
        }
    }

    fn flutterwave_secrets() -> PaymentSecrets {
        PaymentSecrets {
            flutterwave_secret_key: Some("FLWSECK_TEST-xxx".to_string()),
            flutterwave_secret_hash: Some("flw-secret-hash".to_string()),
            ..Default::default()
        }
    }

    fn stripe_secrets() -> PaymentSecrets {
        PaymentSecrets {
            stripe_secret_key: Some("sk_test_stripe".to_string()),
            stripe_webhook_secret: Some("whsec_test".to_string()),
            ..Default::default()
        }
    }

    fn mock_secrets() -> PaymentSecrets {
        PaymentSecrets {
            mock_webhook_secret: Some("s3cret".to_string()),
            ..Default::default()
        }
    }

    fn stripe_headers(body: &str, timestamp: i64) -> Vec<(String, String)> {
        let sig = hmac_hex_sha256("whsec_test", &format!("{timestamp}.{body}"));
        vec![(
            "stripe-signature".to_string(),
            format!("t={timestamp},v1={sig}"),
        )]
    }

    fn header_refs(headers: &[(String, String)]) -> Vec<(&str, &str)> {
        headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }

    #[test]
    #[test]
    fn append_order_query_adds_order_param() {
        assert_eq!(
            append_order_query("https://example.com/shop/checkout/success", "abc"),
            "https://example.com/shop/checkout/success?order=abc"
        );
        assert_eq!(
            append_order_query("https://example.com/success?x=1", "abc"),
            "https://example.com/success?x=1&order=abc"
        );
    }

    fn hmac_matches_independent_vectors_for_sha256_and_sha512() {
        assert_eq!(
            hmac_hex_sha256("test-key", "the quick brown fox"),
            SHA256_FOX
        );
        assert_eq!(
            hmac_hex_sha512("test-key", "the quick brown fox"),
            SHA512_FOX
        );
    }

    #[test]
    fn timing_safe_equal_compares_equal_strings_only() {
        assert!(timing_safe_equal("abc", "abc"));
        assert!(!timing_safe_equal("abc", "abd"));
        assert!(!timing_safe_equal("abc", "abcd"));
    }

    #[test]
    fn paystack_verifies_correct_hmac_sha512_signature() {
        let result = verify_paystack_webhook(
            &paystack_secrets(),
            PAYSTACK_BODY,
            &[("x-paystack-signature", PAYSTACK_SIG)],
        );
        assert_eq!(
            result,
            WebhookResult {
                valid: true,
                event: "charge.success".to_string(),
                reference: Some("order-1".to_string()),
                paid: true,
                detail: "Paystack charge succeeded.".to_string(),
            }
        );
    }

    #[test]
    fn paystack_rejects_wrong_and_missing_signatures() {
        let secrets = paystack_secrets();
        assert!(
            !verify_paystack_webhook(
                &secrets,
                PAYSTACK_BODY,
                &[("x-paystack-signature", "deadbeef")]
            )
            .valid
        );
        assert!(!verify_paystack_webhook(&secrets, PAYSTACK_BODY, &[]).valid);
    }

    #[test]
    fn paystack_accepts_signature_but_does_not_mark_failed_charge_paid() {
        let failed = r#"{"event":"charge.success","data":{"reference":"order-1","status":"failed","amount":125000}}"#;
        let sig = hmac_hex_sha512("sk_test_paystack_secret", failed);
        let result = verify_paystack_webhook(
            &paystack_secrets(),
            failed,
            &[("x-paystack-signature", sig.as_str())],
        );
        assert!(result.valid);
        assert!(!result.paid);
    }

    #[test]
    fn paystack_reports_not_configured_without_secret() {
        let result = verify_paystack_webhook(&PaymentSecrets::empty(), PAYSTACK_BODY, &[]);
        assert!(!result.valid);
        assert!(result.detail.contains("not configured"));
        assert!(!is_configured(
            PAYMENT_PROVIDER_PAYSTACK,
            &PaymentSecrets::empty()
        ));
    }

    #[test]
    fn flutterwave_verifies_matching_verif_hash() {
        let body = r#"{"event":"charge.completed","data":{"tx_ref":"order-2","status":"successful","amount":1250}}"#;
        let result = verify_flutterwave_webhook(
            &flutterwave_secrets(),
            body,
            &[("verif-hash", "flw-secret-hash")],
        );
        assert!(result.valid);
        assert_eq!(result.event, "charge.completed");
        assert_eq!(result.reference.as_deref(), Some("order-2"));
        assert!(result.paid);
    }

    #[test]
    fn flutterwave_rejects_mismatched_hash_and_unpaid_charge() {
        let secrets = flutterwave_secrets();
        let body = r#"{"event":"charge.completed","data":{"tx_ref":"order-2","status":"successful","amount":1250}}"#;
        assert!(
            !verify_flutterwave_webhook(&secrets, body, &[("verif-hash", "someone-else")]).valid
        );
        let failed = r#"{"event":"charge.completed","data":{"tx_ref":"order-2","status":"failed","amount":1250}}"#;
        let result =
            verify_flutterwave_webhook(&secrets, failed, &[("verif-hash", "flw-secret-hash")]);
        assert!(result.valid);
        assert!(!result.paid);
    }

    #[test]
    fn stripe_signed_payload_matches_independent_vector() {
        // The exact string Stripe signs: `<timestamp>.<rawBody>`.
        assert_eq!(
            hmac_hex_sha256("whsec_test", &format!("1700000000.{STRIPE_BODY}")),
            STRIPE_SIGNED_PAYLOAD_SIG
        );
    }

    #[test]
    fn stripe_verifies_fresh_correctly_signed_webhook() {
        let now = 1_795_200_000i64; // fixed "now" for the test
        let headers = stripe_headers(STRIPE_BODY, now);
        let refs = header_refs(&headers);
        let result = verify_stripe_webhook(&stripe_secrets(), STRIPE_BODY, &refs, now);
        assert!(result.valid);
        assert_eq!(result.event, "checkout.session.completed");
        assert_eq!(result.reference.as_deref(), Some("cs_test_123"));
        assert!(result.paid);
    }

    #[test]
    fn stripe_rejects_tampered_signature() {
        let now = 1_795_200_000i64;
        let result = verify_stripe_webhook(
            &stripe_secrets(),
            STRIPE_BODY,
            &[("stripe-signature", &format!("t={now},v1=deadbeef"))],
            now,
        );
        assert!(!result.valid);
    }

    #[test]
    fn stripe_rejects_stale_timestamp_replay() {
        let now = 1_795_200_000i64;
        let stale = now - 3600;
        let headers = stripe_headers(STRIPE_BODY, stale);
        let refs = header_refs(&headers);
        let result = verify_stripe_webhook(&stripe_secrets(), STRIPE_BODY, &refs, now);
        assert!(!result.valid);
        assert!(result.detail.contains("300-second"), "{}", result.detail);
    }

    #[test]
    fn stripe_does_not_mark_unpaid_session_paid() {
        let now = 1_795_200_000i64;
        let unpaid = r#"{"type":"checkout.session.completed","data":{"object":{"id":"cs_test_123","payment_status":"unpaid"}}}"#;
        let headers = stripe_headers(unpaid, now);
        let refs = header_refs(&headers);
        let result = verify_stripe_webhook(&stripe_secrets(), unpaid, &refs, now);
        assert!(result.valid);
        assert!(!result.paid);
    }

    #[test]
    fn mock_accepts_correctly_signed_test_webhook() {
        let result = verify_mock_webhook(
            &mock_secrets(),
            MOCK_BODY,
            &[("x-mock-signature", MOCK_SIG)],
        );
        assert!(result.valid);
        assert_eq!(result.event, "payment.paid");
        assert_eq!(result.reference.as_deref(), Some("mock_order-1"));
        assert!(result.paid);
    }

    #[test]
    fn mock_rejects_when_no_secret_configured() {
        let result = verify_mock_webhook(&PaymentSecrets::empty(), MOCK_BODY, &[]);
        assert!(!result.valid);
        assert!(result.detail.contains("not configured"));
    }

    #[test]
    fn provider_configured_state_is_honest() {
        let empty = PaymentSecrets::empty();
        assert!(is_configured(PAYMENT_PROVIDER_MOCK, &empty));
        assert!(!is_configured(PAYMENT_PROVIDER_STRIPE, &empty));
        assert!(is_configured(
            PAYMENT_PROVIDER_STRIPE,
            &PaymentSecrets {
                stripe_secret_key: Some("sk_test_x".to_string()),
                ..Default::default()
            }
        ));
        assert!(is_configured(
            PAYMENT_PROVIDER_FLUTTERWAVE,
            &flutterwave_secrets()
        ));
        assert!(is_configured(
            PAYMENT_PROVIDER_PAYSTACK,
            &paystack_secrets()
        ));
        assert!(verify_webhook("nope", &empty, "{}", &[], 0).is_err());
    }

    #[test]
    fn checkout_refuses_with_no_provider_selected_503() {
        let db = SqliteDb::new();
        let err = block_on(resolve_payment_provider(&db, &PaymentSecrets::empty())).unwrap_err();
        assert_eq!(err.code, "payment_not_configured");
        assert_eq!(err.status, 503);
    }

    #[test]
    fn checkout_resolves_mock_when_selected() {
        let db = SqliteDb::new();
        block_on(set_selected_provider_id(&db, Some("mock"))).unwrap();
        let resolved = block_on(resolve_payment_provider(&db, &mock_secrets())).unwrap();
        assert_eq!(resolved.provider_id, "mock");
        assert_eq!(resolved.mode, "test");
    }

    #[test]
    fn checkout_refuses_unconfigured_real_provider() {
        let db = SqliteDb::new();
        block_on(set_selected_provider_id(&db, Some("paystack"))).unwrap();
        let err = block_on(resolve_payment_provider(&db, &PaymentSecrets::empty())).unwrap_err();
        assert_eq!(err.status, 503);
        assert!(err.message.contains("not configured"), "{}", err.message);
    }

    #[test]
    fn mock_checkout_starts_payment_and_records_reference() {
        let db = SqliteDb::new();
        block_on(set_selected_provider_id(&db, Some("mock"))).unwrap();
        let product = block_on(create_product(
            &db,
            ProductInput {
                name: Some(json!("Pack Hoodie")),
                price_minor: Some(json!(125000)),
                stock: Some(json!(5)),
                status: Some(json!("published")),
                ..Default::default()
            },
        ))
        .unwrap();
        let token = new_cart_token().unwrap();
        block_on(add_to_cart(&db, &token, product.id, &json!(2))).unwrap();
        let gateway = MockCheckoutGateway;
        let outcome = block_on(start_checkout(
            &db,
            &mock_secrets(),
            &gateway,
            &token,
            &json!("Ada"),
            &json!("ada@example.test"),
            "http://localhost:8787",
            "http://localhost:8787/shop/checkout/success",
            "http://localhost:8787/shop/checkout/cancelled",
        ))
        .unwrap();
        assert_eq!(outcome.order.status, "pending");
        assert_eq!(outcome.order.payment_provider, "mock");
        assert_eq!(outcome.reference, format!("mock_{}", outcome.order.id));
        assert!(outcome
            .checkout_url
            .starts_with("http://localhost:8787/shop/checkout/test?"));
        assert!(outcome.checkout_url.contains(&outcome.reference));
    }

    #[test]
    fn start_checkout_503_leaves_pending_order_with_empty_reference() {
        let db = SqliteDb::new();
        let product = block_on(create_product(
            &db,
            ProductInput {
                name: Some(json!("Pack Hoodie")),
                price_minor: Some(json!(125000)),
                stock: Some(json!(5)),
                status: Some(json!("published")),
                ..Default::default()
            },
        ))
        .unwrap();
        let token = new_cart_token().unwrap();
        block_on(add_to_cart(&db, &token, product.id, &json!(1))).unwrap();
        let gateway = MockCheckoutGateway;
        let err = block_on(start_checkout(
            &db,
            &PaymentSecrets::empty(),
            &gateway,
            &token,
            &json!("Ada"),
            &json!("ada@example.test"),
            "http://localhost:8787",
            "http://localhost:8787/shop/checkout/success",
            "http://localhost:8787/shop/checkout/cancelled",
        ))
        .unwrap_err();
        assert_eq!(err.status, 503);
        assert_eq!(err.code, "payment_not_configured");
        // The order was created before resolution failed (v1 behaviour).
        let list = block_on(crate::orders::list_orders(&db, None, 1, 20)).unwrap();
        assert_eq!(list.total, 1);
        assert_eq!(list.orders[0].status, "pending");
        assert_eq!(list.orders[0].payment_reference, "");
        // …and an empty-reference webhook can never pay it.
        assert!(block_on(crate::orders::get_order_by_reference(&db, ""))
            .unwrap()
            .is_none());
    }

    #[test]
    fn mock_verify_payment_reads_the_webhook_log() {
        let db = SqliteDb::new();
        assert_eq!(
            block_on(mock_verify_payment(&db, "mock_x")).unwrap().status,
            "pending"
        );
    }
}
