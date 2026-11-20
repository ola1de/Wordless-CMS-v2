//! Platform-neutral interface to the WooCommerce Worker (size remedy,
//! 2026-11-20).
//!
//! The WooCommerce compatibility subset (`wordless-woocommerce`) no
//! longer lives in the main Worker: the combined WASM binary outgrew
//! the Cloudflare free-tier 3 MiB limit (P8: 3,241,322 bytes,
//! 103.0%). The subset runs in its own Worker
//! (`wordless-woocommerce`) with its own 3 MiB budget, bound to the
//! main Worker as a service binding (production) or reached over
//! plain HTTP at `WORDLESS_WOO_URL` (local dev — the P6 `dev_url`
//! pattern).
//!
//! The kernel defines the trait; the worker entry implements it over
//! the real transport. Services only ever see this interface, so the
//! transport stays replaceable and the logic stays testable — the same
//! shape as [`crate::converter::ConverterClient`].
//!
//! Contract (all methods speak JSON — the Woo Worker returns the
//! exact serde shapes of the WooCommerce crate's structs, so
//! responses are byte-identical to the old in-process calls):
//!
//! - [`WooClient::resolve_variation`] — the variation id for the
//!   shopper's `attr_<slug>` form choices, or `None` when no
//!   attributes were chosen; a 400 when the choices match no
//!   variation.
//! - [`WooClient::get_product`] — the `WooProduct` JSON for a
//!   variable product (or `Null` when it has no Woo detail), for the
//!   storefront product page.
//! - [`WooClient::zones`] — the shipping zones with their methods,
//!   for the cart checkout form's shipping-method selector.
//! - [`WooClient::totals`] — the cart totals (coupon / tax /
//!   shipping) for a totals input; strict mode makes a bad coupon a
//!   400, like the storefront checkout pre-check.
//! - [`WooClient::shop_checkout`] — the whole storefront checkout
//!   (totals pre-check, provider resolution, gateway, order
//!   creation) in one hop; returns `{ providerId, order,
//!   checkoutUrl, reference, totals }`.
//!
//! An application-level failure from the Woo Worker carries the
//! `WordlessError` code + status it raised, so the main Worker
//! re-raises the exact error the shopper would have seen before the
//! split (a bad coupon is still a 400, `payment_not_configured`
//! still a 503). A transport failure (Worker unreachable, non-JSON
//! answer) is a [`WooError`] with code `internal_error`, status 500.

use async_trait::async_trait;
use serde_json::Value;

/// What went wrong calling the WooCommerce service.
#[derive(Debug, Clone)]
pub struct WooError {
    /// Human-readable message (surfaced in API errors).
    pub message: String,
    /// The `WordlessError` code the Woo Worker raised (or
    /// `internal_error` for a transport failure).
    pub code: String,
    /// The HTTP status the Woo Worker raised (or 500 for a transport
    /// failure).
    pub status: u16,
}

impl WooError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: "internal_error".to_string(),
            status: 500,
        }
    }

    pub fn with_code(message: impl Into<String>, code: impl Into<String>, status: u16) -> Self {
        Self {
            message: message.into(),
            code: code.into(),
            status,
        }
    }

    /// The caller-facing error, with the Woo Worker's exact code +
    /// status (a bad coupon stays a 400, `payment_not_configured`
    /// stays a 503).
    pub fn to_wordless(&self) -> crate::error::WordlessError {
        crate::error::WordlessError::new(&self.code, self.status, &self.message)
    }
}

impl std::fmt::Display for WooError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for WooError {}

/// WooCommerce-service surface. `?Send`, like the `Db` trait and
/// `ObjectStore`: Workers futures are `!Send`, and the runtime is
/// single-threaded.
#[async_trait(?Send)]
pub trait WooClient {
    /// The variation id for the shopper's `attr_<slug>` form choices
    /// (`form` is the raw `/shop/cart/add` form map as JSON), or
    /// `None` when no attributes were chosen. A 400 when the choices
    /// match no variation.
    async fn resolve_variation(
        &self,
        product_id: i64,
        form: &Value,
    ) -> Result<Option<i64>, WooError>;

    /// The `WooProduct` JSON for a variable product, or `Null` when
    /// it has no Woo detail (the storefront then falls back to the
    /// plain add-to-cart form, same as before the split).
    async fn get_product(&self, product_id: i64) -> Result<Value, WooError>;

    /// The shipping zones with their methods as JSON
    /// (`[{ "zone", "methods" }]`), for the cart checkout form.
    async fn zones(&self) -> Result<Value, WooError>;

    /// The cart totals for a totals input (`{ couponCode, country,
    /// state, shippingMethodId }`). Strict mode makes a bad coupon a
    /// 400.
    async fn totals(
        &self,
        cart_token: &str,
        totals_input: &Value,
        strict: bool,
    ) -> Result<Value, WooError>;

    /// The whole storefront checkout in one hop. Returns
    /// `{ providerId, order, checkoutUrl, reference, totals }`.
    async fn shop_checkout(
        &self,
        cart_token: &str,
        customer_name: &Value,
        customer_email: &Value,
        totals_input: &Value,
        base: &str,
        success_url: &str,
        cancel_url: &str,
    ) -> Result<Value, WooError>;
}
