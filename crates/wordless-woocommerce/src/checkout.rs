//! Woo checkout (P8) — order + provider + gateway.
//!
//! This mirrors `wordless-commerce`'s `start_checkout` step for step
//! (same provider resolution, same gateway match check, same
//! `set_order_payment`), with one difference: the order is created
//! by the Woo path (`create_order_from_cart_with` + `compute_totals`),
//! so coupons, tax and shipping land on the order before any money
//! moves. With no provider selected — or the selected provider
//! unconfigured — this returns the same typed 503
//! (`payment_not_configured`) as P3: never a fake success. The
//! pending order stays pending with an empty `payment_reference`,
//! exactly as in v1/P3.

use serde::Serialize;
use serde_json::Value;
use wordless_commerce::cart::get_cart;
use wordless_commerce::orders::{create_order_from_cart_with, set_order_payment, OrderTotals};
use wordless_commerce::payments::{
    resolve_payment_provider, CheckoutGateway, PaymentCheckoutInput, PaymentSecrets,
};
use wordless_commerce::types::Order;
use wordless_db::Db;
use wordless_kernel::error::{bad_request, WordlessError};

use crate::customers::{record_order_for_customer, split_name, upsert_customer};
use crate::totals::{compute_totals, TotalsInput};
use crate::types::Totals;

/// The result of a Woo checkout: the order (now carrying its
/// provider + reference and the Woo totals), where to send the
/// customer, and the totals breakdown the customer already saw.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WooCheckoutOutcome {
    pub order: Order,
    pub checkout_url: String,
    pub reference: String,
    pub totals: Totals,
}

/// Append `?order=<id>` (or `&order=<id>`) to a provider return URL
/// — the same rule as P3's `start_checkout`.
fn append_order_query(url: &str, order_id: &str) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}order={order_id}")
}

/// Start a Woo checkout: compute totals (strict — a bad coupon is a
/// 400), create the order from the cart, record the customer, then
/// run the P3 payment flow (resolve provider → gateway → reference).
pub async fn start_woo_checkout(
    db: &dyn Db,
    secrets: &PaymentSecrets,
    gateway: &dyn CheckoutGateway,
    cart_token: &str,
    customer_name: &Value,
    customer_email: &Value,
    totals_input: &TotalsInput,
    site_url: &str,
    success_url: &str,
    cancel_url: &str,
) -> Result<WooCheckoutOutcome, WordlessError> {
    let cart = get_cart(db, cart_token).await?;
    if cart.lines.is_empty() {
        return Err(bad_request("The cart is empty."));
    }
    if let Some(note) = &cart.currency_note {
        return Err(bad_request(note));
    }
    let totals = compute_totals(db, &cart, totals_input, true).await?;
    // The order is a Woo order only when Woo totals actually apply
    // (a coupon, a discount, tax, or shipping). A plain checkout
    // through this path — e.g. the v1 storefront /shop/checkout with
    // no coupon, tax off, no shipping configured — stays a plain
    // 'wordless' order, so its API shape is identical to v1 (fixed
    // 2026-11-20: every storefront order was stamped 'woo', which
    // surfaced as a parity DIFF on GET /api/commerce/orders).
    let source = if totals.coupon_code.is_some()
        || totals.discount_minor > 0
        || totals.tax_minor > 0
        || totals.shipping_minor > 0
        || totals.shipping_method.is_some()
    {
        "woo".to_string()
    } else {
        "wordless".to_string()
    };
    let order = create_order_from_cart_with(
        db,
        cart_token,
        customer_name,
        customer_email,
        Some(OrderTotals {
            coupon_code: totals.coupon_code.clone(),
            discount_minor: totals.discount_minor,
            tax_minor: totals.tax_minor,
            shipping_minor: totals.shipping_minor,
            shipping_method: totals.shipping_method.clone(),
            source,
        }),
    )
    .await?;
    // The customer record is store data, not a login: upsert by email
    // and count this order against them.
    let (first_name, last_name) = split_name(&order.customer_name);
    let _ = upsert_customer(
        db,
        &order.customer_email,
        &first_name,
        &last_name,
        &totals_input.country,
        &totals_input.state,
        "",
        "woo",
        "",
    )
    .await;
    let _ = record_order_for_customer(db, &order.customer_email, order.total_minor).await;
    let resolved = resolve_payment_provider(db, secrets).await?;
    if gateway.provider_id() != resolved.provider_id {
        return Err(WordlessError::new(
            "internal_error",
            500,
            "Checkout gateway does not match the selected payment provider.",
        ));
    }
    let success_url = append_order_query(success_url, &order.id);
    let cancel_url = append_order_query(cancel_url, &order.id);
    let input = PaymentCheckoutInput::from_order(&order, site_url, &success_url, &cancel_url);
    let checkout = gateway.create_checkout(&input).await?;
    let updated =
        set_order_payment(db, &order.id, &resolved.provider_id, &checkout.reference).await?;
    Ok(WooCheckoutOutcome {
        order: updated,
        checkout_url: checkout.checkout_url,
        reference: checkout.reference,
        totals,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coupons::create_coupon;
    use crate::test_support::TestDb;
    use crate::types::CouponInput;
    use futures_executor::block_on;
    use serde_json::json;
    use wordless_commerce::cart::{add_to_cart, new_cart_token};
    use wordless_commerce::payments::{
        set_selected_provider_id, MockCheckoutGateway, PaymentSecrets,
    };
    use wordless_commerce::products::create_product;
    use wordless_commerce::types::ProductInput;

    fn mock_secrets() -> PaymentSecrets {
        PaymentSecrets {
            mock_webhook_secret: Some("s3cret".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn woo_checkout_applies_coupon_and_records_customer() {
        let db = TestDb::new();
        block_on(set_selected_provider_id(&db, Some("mock"))).unwrap();
        block_on(create_coupon(
            &db,
            CouponInput {
                code: Some(json!("SAVE50")),
                amount_minor: Some(json!(50000)),
                ..Default::default()
            },
        ))
        .unwrap();
        let product = block_on(create_product(
            &db,
            ProductInput {
                name: Some(json!("Pack Hoodie")),
                price_minor: Some(json!(200000)),
                stock: Some(json!(5)),
                status: Some(json!("published")),
                ..Default::default()
            },
        ))
        .unwrap();
        let token = new_cart_token().unwrap();
        block_on(add_to_cart(&db, &token, product.id, &json!(1))).unwrap();
        let outcome = block_on(start_woo_checkout(
            &db,
            &mock_secrets(),
            &MockCheckoutGateway,
            &token,
            &json!("Ada Lovelace"),
            &json!("ada@example.test"),
            &TotalsInput {
                coupon_code: "SAVE50".to_string(),
                ..Default::default()
            },
            "http://localhost:8787",
            "http://localhost:8787/shop/checkout/success",
            "http://localhost:8787/shop/checkout/cancelled",
        ))
        .unwrap();
        assert_eq!(outcome.order.status, "pending");
        assert_eq!(outcome.order.total_minor, 150000); // 200,000 − 50,000
        assert_eq!(outcome.order.discount_minor, 50000);
        assert_eq!(outcome.order.coupon_code.as_deref(), Some("SAVE50"));
        assert_eq!(outcome.order.source.as_deref(), Some("woo"));
        assert_eq!(outcome.totals.total_minor, 150000);
        assert_eq!(outcome.reference, format!("mock_{}", outcome.order.id));
        // The customer was recorded.
        let customers = block_on(crate::customers::list_customers(&db, 1, 20)).unwrap();
        assert_eq!(customers.1, 1);
        assert_eq!(customers.0[0].email, "ada@example.test");
        assert_eq!(customers.0[0].total_orders, 1);
        assert_eq!(customers.0[0].total_spent_minor, 150000);
    }

    #[test]
    fn woo_checkout_503_with_no_provider_leaves_pending_order() {
        let db = TestDb::new();
        let product = block_on(create_product(
            &db,
            ProductInput {
                name: Some(json!("Pack Hoodie")),
                price_minor: Some(json!(200000)),
                stock: Some(json!(5)),
                status: Some(json!("published")),
                ..Default::default()
            },
        ))
        .unwrap();
        let token = new_cart_token().unwrap();
        block_on(add_to_cart(&db, &token, product.id, &json!(1))).unwrap();
        let err = block_on(start_woo_checkout(
            &db,
            &PaymentSecrets::empty(),
            &MockCheckoutGateway,
            &token,
            &json!("Ada"),
            &json!("ada@example.test"),
            &TotalsInput::default(),
            "http://localhost:8787",
            "http://localhost:8787/shop/checkout/success",
            "http://localhost:8787/shop/checkout/cancelled",
        ))
        .unwrap_err();
        assert_eq!(err.status, 503);
        assert_eq!(err.code, "payment_not_configured");
    }
}
