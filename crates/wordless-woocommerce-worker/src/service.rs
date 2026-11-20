//! Internal `__woo` service (size remedy, 2026-11-20).
//!
//! The main Worker keeps the storefront routes (`/shop/*`) and the
//! render code, but the WooCommerce logic lives here. These endpoints
//! let the main Worker call into the Woo logic over the service
//! binding (production) or plain HTTP (local dev) — the same pattern
//! as the converter Worker's `__convert` endpoints.
//!
//! Every endpoint speaks plain JSON and answers
//! `{ "ok": true, ... }` or `{ "ok": false, "error", "code",
//! "status" }`. Application failures carry the `WordlessError` code
//! + status so the main Worker can re-raise the exact error the
//! shopper would have seen before the split (e.g. a bad coupon is a
//! 400, not a 500). Bad JSON / missing fields → 400.
//! Unknown paths → 404. Nothing here is part of the public API.
//!
//! - `POST /__woo/resolve-variation` `{ productId, form }` → the
//!   variation id for the shopper's `attr_<slug>` choices, or
//!   `variationId: null` when no attributes were chosen; 400 when
//!   the choices match no variation. (`form` is the raw
//!   `/shop/cart/add` form map.)
//! - `POST /__woo/get-product` `{ productId }` → `{ product:
//!   WooProduct | null }` (for the storefront product page).
//! - `POST /__woo/zones` `{}` → `{ zones: [{ zone, methods }] }`
//!   (for the cart checkout form's shipping-method selector).
//! - `POST /__woo/totals` `{ cartToken, totalsInput, strict }` →
//!   `{ totals }` (strict = a bad coupon is a 400, like the
//!   storefront checkout pre-check).
//! - `POST /__woo/shop-checkout` `{ cartToken, customerName,
//!   customerEmail, totalsInput, base, successUrl, cancelUrl }` →
//!   `{ providerId, order, checkoutUrl, reference, totals }` — the
//!   whole storefront checkout (totals pre-check, provider
//!   resolution, gateway, order creation) in one hop.

use std::rc::Rc;

use serde_json::{json, Value};
use wordless_commerce::payments::{resolve_payment_provider, MockCheckoutGateway, PaymentSecrets};
use wordless_db::Db;
use wordless_gateways::gateway_for;
use wordless_kernel::error::{bad_request, WordlessError};
use wordless_woocommerce::attributes::list_attributes;
use wordless_woocommerce::checkout::start_woo_checkout;
use wordless_woocommerce::products::get_woo_product;
use wordless_woocommerce::shipping::list_zones_with_methods;
use wordless_woocommerce::totals::{compute_totals_for_cart, TotalsInput};
use wordless_woocommerce::variations::find_variation_by_attributes;

/// One endpoint's answer: HTTP status + JSON body.
pub type ServiceAnswer = (u16, Value);

/// What the service needs: the shared D1 + the payment secrets
/// (same secrets as the main Worker — the Woo checkout resolves the
/// provider and gateway here, in the Worker that owns the checkout).
pub struct Service {
    pub db: Rc<dyn Db>,
    pub payment_secrets: PaymentSecrets,
}

fn ok(body: Value) -> ServiceAnswer {
    (200, body)
}

fn fail(e: WordlessError) -> ServiceAnswer {
    (
        200,
        json!({ "ok": false, "error": e.message, "code": e.code, "status": e.status }),
    )
}

fn bad_input(message: &str) -> ServiceAnswer {
    (
        400,
        json!({ "ok": false, "error": message, "code": "bad_request", "status": 400 }),
    )
}

fn totals_input(body: &Value) -> Result<TotalsInput, ServiceAnswer> {
    let input = body.get("totalsInput").cloned().unwrap_or(Value::Null);
    let coupon_code = input
        .get("couponCode")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let country = input
        .get("country")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let state = input
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let shipping_method_id = input.get("shippingMethodId").and_then(|v| v.as_i64());
    Ok(TotalsInput {
        coupon_code,
        country,
        state,
        shipping_method_id,
    })
}

impl Service {
    pub async fn resolve_variation(&self, body: &Value) -> ServiceAnswer {
        let product_id = match body.get("productId").and_then(|v| v.as_i64()) {
            Some(id) if id >= 1 => id,
            _ => return bad_input("Field \"productId\" must be an integer."),
        };
        let form = body.get("form").cloned().unwrap_or(Value::Null);
        let result = async {
            let attrs = list_attributes(self.db.as_ref(), product_id).await?;
            let selections: Vec<(String, String)> = attrs
                .iter()
                .filter(|a| a.is_variation)
                .filter_map(|a| {
                    form.get(&format!("attr_{}", a.slug))
                        .and_then(|v| v.as_str())
                        .map(|v| (a.name.clone(), v.to_string()))
                })
                .collect();
            if selections.is_empty() {
                return Ok::<_, WordlessError>(None);
            }
            match find_variation_by_attributes(self.db.as_ref(), product_id, &selections).await? {
                Some(v) => Ok(Some(v.id)),
                None => Err(bad_request(
                    "No variation matches those options — please choose each option.",
                )),
            }
        }
        .await;
        match result {
            Ok(variation_id) => ok(json!({ "ok": true, "variationId": variation_id })),
            Err(e) => fail(e),
        }
    }

    pub async fn get_product(&self, body: &Value) -> ServiceAnswer {
        let product_id = match body.get("productId").and_then(|v| v.as_i64()) {
            Some(id) if id >= 1 => id,
            _ => return bad_input("Field \"productId\" must be an integer."),
        };
        match get_woo_product(self.db.as_ref(), product_id).await {
            Ok(product) => ok(json!({ "ok": true, "product": product })),
            Err(e) => fail(e),
        }
    }

    pub async fn zones(&self) -> ServiceAnswer {
        match list_zones_with_methods(self.db.as_ref()).await {
            Ok(zones) => {
                let zones: Vec<Value> = zones
                    .into_iter()
                    .map(|(zone, methods)| json!({ "zone": zone, "methods": methods }))
                    .collect();
                ok(json!({ "ok": true, "zones": zones }))
            }
            Err(e) => fail(e),
        }
    }

    pub async fn totals(&self, body: &Value) -> ServiceAnswer {
        let cart_token = match body.get("cartToken").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => t.to_string(),
            _ => return bad_input("Field \"cartToken\" is required."),
        };
        let input = match totals_input(body) {
            Ok(input) => input,
            Err(answer) => return answer,
        };
        let strict = body
            .get("strict")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        match compute_totals_for_cart(self.db.as_ref(), &cart_token, &input, strict).await {
            Ok(totals) => ok(json!({ "ok": true, "totals": totals })),
            Err(e) => fail(e),
        }
    }

    pub async fn shop_checkout(&self, body: &Value) -> ServiceAnswer {
        let cart_token = match body.get("cartToken").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => t.to_string(),
            _ => return bad_input("Field \"cartToken\" is required."),
        };
        let customer_name = body.get("customerName").cloned().unwrap_or(Value::Null);
        let customer_email = body.get("customerEmail").cloned().unwrap_or(Value::Null);
        let input = match totals_input(body) {
            Ok(input) => input,
            Err(answer) => return answer,
        };
        let base = body
            .get("base")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let success_url = body
            .get("successUrl")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let cancel_url = body
            .get("cancelUrl")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let result = async {
            // Same precedence as before the split: validate the Woo
            // totals (strict — a bad coupon is a 400) BEFORE resolving
            // the payment provider, so the shopper sees the coupon
            // error, not `payment_not_configured`.
            compute_totals_for_cart(self.db.as_ref(), &cart_token, &input, true).await?;
            let resolved =
                resolve_payment_provider(self.db.as_ref(), &self.payment_secrets).await?;
            let gateway =
                gateway_for(&resolved.provider_id, &self.payment_secrets).unwrap_or_else(|| {
                    Rc::new(MockCheckoutGateway)
                        as Rc<dyn wordless_commerce::payments::CheckoutGateway>
                });
            let outcome = start_woo_checkout(
                self.db.as_ref(),
                &self.payment_secrets,
                gateway.as_ref(),
                &cart_token,
                &customer_name,
                &customer_email,
                &input,
                &base,
                &success_url,
                &cancel_url,
            )
            .await?;
            Ok::<_, WordlessError>((resolved.provider_id, outcome))
        }
        .await;
        match result {
            Ok((provider_id, outcome)) => ok(json!({
                "ok": true,
                "providerId": provider_id,
                "order": outcome.order,
                "checkoutUrl": outcome.checkout_url,
                "reference": outcome.reference,
                "totals": outcome.totals,
            })),
            Err(e) => fail(e),
        }
    }
}
