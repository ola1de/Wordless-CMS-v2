//! Totals (P8) — the ONE quote/checkout computation.
//!
//! Both the quote endpoint (`POST /api/woo/quote`) and the Woo
//! checkout call `compute_totals` with the same cart, so the price a
//! customer sees is the price they pay. The breakdown:
//!
//! ```text
//! subtotal = Σ cart line totals (variation prices already resolved)
//! discount = coupon on the subtotal (0 when no/invalid coupon)
//! taxable  = subtotal − discount
//! tax      = rate × taxable, rounded half-up (0 when tax is off)
//! shipping = quoted method cost (0 when nothing applies)
//! total    = taxable + tax + shipping
//! ```
//!
//! All integer minor units. `strict` decides what an invalid coupon
//! does: the quote reports it (discount 0 + `coupon_note`), the
//! checkout refuses with a 400 — a customer who typed a bad code
//! must not silently pay full price and discover it later.

use wordless_commerce::cart::get_cart;
use wordless_commerce::types::Cart;
use wordless_db::Db;
use wordless_kernel::error::{bad_request, WordlessError};

use crate::coupons::validate_coupon;
use crate::shipping::quote_shipping;
use crate::taxes::{prices_include_tax, rate_for, tax_enabled, tax_on};
use crate::types::Totals;

/// Inputs for a totals computation.
#[derive(Debug, Clone, Default)]
pub struct TotalsInput {
    /// Coupon code typed by the customer ("" = none).
    pub coupon_code: String,
    /// ISO country code for tax + shipping ("" = unknown).
    pub country: String,
    /// State/region for tax ("" = unknown).
    pub state: String,
    /// Explicitly chosen shipping method id (None = cheapest).
    pub shipping_method_id: Option<i64>,
}

/// Compute the totals for a cart. `strict = true` (checkout) turns
/// an invalid supplied coupon into a 400; `strict = false` (quote)
/// reports it as discount 0 + `coupon_note`.
pub async fn compute_totals(
    db: &dyn Db,
    cart: &Cart,
    input: &TotalsInput,
    strict: bool,
) -> Result<Totals, WordlessError> {
    let subtotal = cart.total_minor;
    let currency = cart.currency.clone();

    // Coupon on the subtotal.
    let (discount, coupon_code, coupon_note) = if input.coupon_code.trim().is_empty() {
        (0, None, None)
    } else {
        let v = validate_coupon(db, &input.coupon_code, subtotal, &currency).await?;
        match (&v.coupon, &v.reason) {
            (Some(_), None) => (
                v.discount_minor,
                Some(input.coupon_code.trim().to_ascii_uppercase()),
                None,
            ),
            (_, Some(reason)) => {
                if strict {
                    return Err(bad_request(reason));
                }
                (
                    0,
                    Some(input.coupon_code.trim().to_ascii_uppercase()),
                    Some(reason.clone()),
                )
            }
            _ => (0, None, None),
        }
    };
    let taxable = (subtotal - discount).max(0);

    // Tax on (subtotal − discount), when enabled.
    let (tax, rate_bp, rate_name) = if tax_enabled(db).await? {
        match rate_for(db, &input.country, &input.state).await? {
            Some(rate) => (
                tax_on(taxable, rate.rate_bp, prices_include_tax(db).await?),
                rate.rate_bp,
                Some(rate.name),
            ),
            None => (0, 0, None),
        }
    } else {
        (0, 0, None)
    };

    // Shipping on the discounted subtotal.
    let quote = quote_shipping(
        db,
        &input.country,
        taxable,
        input.shipping_method_id,
        &currency,
    )
    .await?;
    let shipping = quote.cost_minor;

    Ok(Totals {
        subtotal_minor: subtotal,
        discount_minor: discount,
        coupon_code,
        coupon_note,
        tax_minor: tax,
        tax_rate_bp: rate_bp,
        tax_rate_name: rate_name,
        shipping_minor: shipping,
        shipping_method: quote.method.as_ref().map(|m| m.name.clone()),
        shipping_method_id: quote.method.as_ref().map(|m| m.id),
        total_minor: taxable + tax + shipping,
        currency,
    })
}

/// Convenience: load the cart for a token and compute its totals.
pub async fn compute_totals_for_cart(
    db: &dyn Db,
    cart_token: &str,
    input: &TotalsInput,
    strict: bool,
) -> Result<Totals, WordlessError> {
    let cart = get_cart(db, cart_token).await?;
    if cart.lines.is_empty() {
        return Err(bad_request("The cart is empty."));
    }
    if let Some(note) = &cart.currency_note {
        return Err(bad_request(note));
    }
    compute_totals(db, &cart, input, strict).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coupons::create_coupon;
    use crate::shipping::{create_method, create_zone};
    use crate::taxes::{create_tax_rate, set_tax_settings};
    use crate::test_support::TestDb;
    use crate::types::{CouponInput, ShippingMethodInput, ShippingZoneInput, TaxRateInput};
    use futures_executor::block_on;
    use serde_json::json;
    use wordless_commerce::cart::{add_to_cart, new_cart_token};
    use wordless_commerce::products::create_product;
    use wordless_commerce::types::ProductInput;

    fn setup_cart(db: &TestDb) -> String {
        let product = block_on(create_product(
            db,
            ProductInput {
                name: Some(json!("Pack Hoodie")),
                price_minor: Some(json!(100000)),
                stock: Some(json!(10)),
                status: Some(json!("published")),
                ..Default::default()
            },
        ))
        .unwrap();
        let token = new_cart_token().unwrap();
        block_on(add_to_cart(db, &token, product.id, &json!(2))).unwrap();
        token
    }

    #[test]
    fn totals_add_up_with_coupon_tax_and_shipping() {
        let db = TestDb::new();
        let token = setup_cart(&db); // subtotal 200,000
        block_on(create_coupon(
            &db,
            CouponInput {
                code: Some(json!("TENPCT")),
                r#type: Some(json!("percent")),
                percent_bp: Some(json!(1000)),
                ..Default::default()
            },
        ))
        .unwrap();
        block_on(set_tax_settings(&db, true, false)).unwrap();
        block_on(create_tax_rate(
            &db,
            TaxRateInput {
                name: Some(json!("Nigeria VAT")),
                country: Some(json!("NG")),
                rate_bp: Some(json!(750)),
                ..Default::default()
            },
        ))
        .unwrap();
        let zone = block_on(create_zone(
            &db,
            ShippingZoneInput {
                name: Some(json!("Nigeria")),
                countries: Some(json!(["NG"])),
            },
        ))
        .unwrap();
        block_on(create_method(
            &db,
            zone.id,
            ShippingMethodInput {
                name: Some(json!("Flat rate")),
                r#type: Some(json!("flat_rate")),
                cost_minor: Some(json!(250000)),
                ..Default::default()
            },
        ))
        .unwrap();
        let totals = block_on(compute_totals_for_cart(
            &db,
            &token,
            &TotalsInput {
                coupon_code: "TENPCT".to_string(),
                country: "NG".to_string(),
                ..Default::default()
            },
            true,
        ))
        .unwrap();
        // subtotal 200,000 − 10% (20,000) = 180,000 taxable;
        // tax 7.5% of 180,000 = 13,500; shipping 250,000;
        // total 180,000 + 13,500 + 250,000 = 443,500.
        assert_eq!(totals.subtotal_minor, 200000);
        assert_eq!(totals.discount_minor, 20000);
        assert_eq!(totals.tax_minor, 13500);
        assert_eq!(totals.shipping_minor, 250000);
        assert_eq!(totals.total_minor, 443500);
        assert_eq!(totals.currency, "NGN");
    }

    #[test]
    fn quote_reports_bad_coupon_but_checkout_refuses_it() {
        let db = TestDb::new();
        let token = setup_cart(&db);
        let input = TotalsInput {
            coupon_code: "NOPE".to_string(),
            ..Default::default()
        };
        let quote = block_on(compute_totals_for_cart(&db, &token, &input, false)).unwrap();
        assert_eq!(quote.discount_minor, 0);
        assert!(quote.coupon_note.is_some());
        let err = block_on(compute_totals_for_cart(&db, &token, &input, true)).unwrap_err();
        assert!(err.message.contains("does not exist"), "{}", err.message);
    }

    #[test]
    fn tax_off_by_default_so_plain_totals_match_p3() {
        let db = TestDb::new();
        let token = setup_cart(&db);
        let totals = block_on(compute_totals_for_cart(
            &db,
            &token,
            &TotalsInput::default(),
            true,
        ))
        .unwrap();
        assert_eq!(totals.total_minor, 200000);
        assert_eq!(totals.tax_minor, 0);
        assert_eq!(totals.shipping_minor, 0);
    }
}
