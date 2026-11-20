//! WooCommerce route table — WooCommerce compatibility subset (never
//! WooCommerce PHP): variable products (attributes + variations),
//! coupons, taxes, shipping, customers, CSV import, and the Woo
//! checkout (coupon / tax / shipping on top of the P3 pipeline).
//!
//! Moved here verbatim from the main Worker's `routes_p8.rs` in the
//! 2026-11-20 size remedy (only the `Ctx` / helper / gateway /
//! `site_url` imports changed). All routes use the v1 `{ok, data,
//! error}` envelope. Admin routes use
//! the same `manage_options` capability as the P3 commerce routes;
//! cart / quote / checkout / validate are public (cart-identified
//! by cookie, like P3). The P3 routes are NOT duplicated here:
//! orders live at `/api/commerce/orders` (they now carry the Woo
//! fields when set), payments at `/api/commerce/checkout` (P3
//! totals) and webhooks at the P3 webhook route.
//!
//! Decisions (doc §3 P8, recorded before any P8 code): import is
//! CSV only (Q12); tables are extended, not duplicated; money is
//! integer minor units; tax is off by default and prices are
//! exclusive by default, so the P3 checkout totals cannot change
//! until the owner enables tax.

use std::rc::Rc;

use serde_json::{json, Value};
use wordless_commerce::cart::{
    add_to_cart_variation, cart_cookie_header, clear_cart, get_cart, new_cart_token,
    parse_cart_cookie, set_cart_quantity_variation,
};
use wordless_commerce::payments::{resolve_payment_provider, CheckoutGateway, MockCheckoutGateway};
use wordless_commerce::products::{create_product, delete_product, update_product};
use wordless_commerce::types::ProductInput;
use wordless_kernel::error::{bad_request, not_found, WordlessError};
use wordless_kernel::router::{read_json_body, RequestContext, RouteOptions, Router};
use wordless_woocommerce::attributes::{
    create_attribute, delete_attribute, list_attributes, update_attribute,
};
use wordless_woocommerce::checkout::start_woo_checkout;
use wordless_woocommerce::coupons::{
    create_coupon, delete_coupon, get_coupon, get_coupon_by_code, list_coupons, update_coupon,
    validate_coupon,
};
use wordless_woocommerce::csv::import_csv;
use wordless_woocommerce::customers::{get_customer, list_customers};
use wordless_woocommerce::products::{
    delete_woo_product_extras, get_product_by_sku, get_variation_by_sku, get_woo_product,
    list_woo_products, set_product_terms,
};
use wordless_woocommerce::shipping::{
    create_method, create_zone, delete_method, delete_zone, list_methods, list_zones_with_methods,
    quote_shipping, update_method, update_zone,
};
use wordless_woocommerce::taxes::{
    create_tax_rate, delete_tax_rate, list_tax_rates, prices_include_tax, set_tax_settings,
    tax_enabled, update_tax_rate,
};
use wordless_woocommerce::totals::{compute_totals_for_cart, TotalsInput};
use wordless_woocommerce::types::{
    AttributeInput, CouponInput, ShippingMethodInput, ShippingZoneInput, TaxRateInput,
    VariationInput,
};
use wordless_woocommerce::variations::{
    create_variation, delete_variation, get_variation_for_product, list_variations,
    update_variation,
};

use crate::ctx::Ctx;
use crate::helpers::{
    log_action, ok, ok_json, opt_i64, opt_str, opt_str_array, page_param, parse_id, per_page_param,
    site_url,
};
use wordless_gateways::gateway_for;

/// The admin capability, same as the P3 commerce routes.
const ADMIN: RouteOptions = RouteOptions {
    require_auth: false,
    capability: Some("manage_options"),
};

/// `json_field` (main Worker routes_p3.rs) is private there; same rule here:
/// a present non-null field, else `None`.
fn json_field(body: &Value, key: &str) -> Option<Value> {
    body.get(key).filter(|v| !v.is_null()).cloned()
}

/// The cart token from the request cookie, minting a new one (and
/// its `Set-Cookie`) when the request has none — same as P3.
fn cart_token(ctx: &Ctx) -> (String, Option<String>) {
    match parse_cart_cookie(ctx.header("cookie")) {
        Some(token) => (token, None),
        None => match new_cart_token() {
            Ok(token) => {
                let cookie = cart_cookie_header(&token);
                (token, Some(cookie))
            }
            Err(_) => (String::new(), None),
        },
    }
}

/// Quote/checkout inputs from a JSON body.
fn totals_input(body: &Value) -> TotalsInput {
    TotalsInput {
        coupon_code: opt_str(body, "couponCode").unwrap_or_default(),
        country: opt_str(body, "country").unwrap_or_default(),
        state: opt_str(body, "state").unwrap_or_default(),
        shipping_method_id: opt_i64(body, "shippingMethodId"),
    }
}

/// A `ProductInput` from a JSON body (P3 fields + P8 `type`/`sku`).
fn product_input(body: &Value) -> ProductInput {
    ProductInput {
        name: json_field(body, "name"),
        slug: json_field(body, "slug"),
        description: json_field(body, "description"),
        price_minor: json_field(body, "priceMinor"),
        currency: json_field(body, "currency"),
        stock: json_field(body, "stock"),
        status: json_field(body, "status"),
        image_id: json_field(body, "imageId"),
        product_type: json_field(body, "type"),
        sku: json_field(body, "sku"),
    }
}

/// Register every P8 route on the router built by `routes.rs`.
pub fn register_woo_routes(router: &mut Router<Ctx>) {
    // ------------------------------------------------------- products
    router.get(
        "/api/woo/products",
        |ctx| {
            Box::pin(async move {
                let product_type = ctx.query("type").map(|s| s.to_string());
                let status = ctx.query("status").map(|s| s.to_string());
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                ok(
                    list_woo_products(
                        ctx.db.as_ref(),
                        product_type.as_deref(),
                        status.as_deref(),
                        page,
                        per_page,
                    )
                    .await?,
                    200,
                )
            })
        },
        ADMIN,
    );

    // Registered BEFORE `/api/woo/products/:id` so `sku` wins over
    // the id parameter.
    router.get(
        "/api/woo/products/sku/:sku",
        |ctx| {
            Box::pin(async move {
                let sku = ctx.param("sku").unwrap_or("").to_string();
                let product = get_product_by_sku(ctx.db.as_ref(), &sku)
                    .await?
                    .ok_or_else(|| not_found(&format!("Product with SKU \"{sku}\" not found.")))?;
                ok(product, 200)
            })
        },
        ADMIN,
    );

    router.get(
        "/api/woo/products/:id",
        |ctx| {
            Box::pin(async move {
                let raw = ctx.param("id").unwrap_or("").to_string();
                let id = raw
                    .parse::<i64>()
                    .map_err(|_| not_found(&format!("Product \"{raw}\" not found.")))?;
                let product = get_woo_product(ctx.db.as_ref(), id)
                    .await?
                    .ok_or_else(|| not_found(&format!("Product \"{raw}\" not found.")))?;
                ok(product, 200)
            })
        },
        RouteOptions::default(),
    );

    router.post(
        "/api/woo/products",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let product = create_product(ctx.db.as_ref(), product_input(&body)).await?;
                if let Some(attrs) = body.get("attributes").and_then(|v| v.as_array()) {
                    for attr in attrs {
                        let input: AttributeInput = serde_json::from_value(attr.clone())
                            .map_err(|_| bad_request("Invalid attribute data."))?;
                        create_attribute(ctx.db.as_ref(), product.id, input).await?;
                    }
                }
                if let Some(terms) = opt_str_array(&body, "categories") {
                    set_product_terms(ctx.db.as_ref(), product.id, "product_cat", &terms).await?;
                }
                if let Some(terms) = opt_str_array(&body, "tags") {
                    set_product_terms(ctx.db.as_ref(), product.id, "product_tag", &terms).await?;
                }
                log_action(ctx, "woo.product_create", "product", product.id).await;
                let detail = get_woo_product(ctx.db.as_ref(), product.id)
                    .await?
                    .ok_or_else(|| {
                        WordlessError::new("internal_error", 500, "Product was not created.")
                    })?;
                ok(detail, 201)
            })
        },
        ADMIN,
    );

    router.put(
        "/api/woo/products/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "product id")?;
                let body = read_json_body(ctx)?;
                update_product(ctx.db.as_ref(), id, product_input(&body)).await?;
                if let Some(terms) = opt_str_array(&body, "categories") {
                    set_product_terms(ctx.db.as_ref(), id, "product_cat", &terms).await?;
                }
                if let Some(terms) = opt_str_array(&body, "tags") {
                    set_product_terms(ctx.db.as_ref(), id, "product_tag", &terms).await?;
                }
                log_action(ctx, "woo.product_update", "product", id).await;
                let detail = get_woo_product(ctx.db.as_ref(), id)
                    .await?
                    .ok_or_else(|| not_found(&format!("Product {id} not found.")))?;
                ok(detail, 200)
            })
        },
        ADMIN,
    );

    router.delete(
        "/api/woo/products/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "product id")?;
                let deleted = delete_product(ctx.db.as_ref(), id).await?;
                if deleted {
                    delete_woo_product_extras(ctx.db.as_ref(), id).await?;
                }
                log_action(ctx, "woo.product_delete", "product", id).await;
                ok_json(json!({ "deleted": deleted }), 200)
            })
        },
        ADMIN,
    );

    // ----------------------------------------------------- attributes
    router.get(
        "/api/woo/products/:id/attributes",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "product id")?;
                ok(list_attributes(ctx.db.as_ref(), id).await?, 200)
            })
        },
        ADMIN,
    );

    router.post(
        "/api/woo/products/:id/attributes",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "product id")?;
                let body = read_json_body(ctx)?;
                let input: AttributeInput = serde_json::from_value(body)
                    .map_err(|_| bad_request("Invalid attribute data."))?;
                let attr = create_attribute(ctx.db.as_ref(), id, input).await?;
                log_action(ctx, "woo.attribute_create", "product", id).await;
                ok(attr, 201)
            })
        },
        ADMIN,
    );

    router.put(
        "/api/woo/products/:id/attributes/:attrId",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "product id")?;
                let attr_id = parse_id(ctx, "attrId", "attribute id")?;
                let body = read_json_body(ctx)?;
                let input: AttributeInput = serde_json::from_value(body)
                    .map_err(|_| bad_request("Invalid attribute data."))?;
                let attr = update_attribute(ctx.db.as_ref(), id, attr_id, input).await?;
                log_action(ctx, "woo.attribute_update", "product", id).await;
                ok(attr, 200)
            })
        },
        ADMIN,
    );

    router.delete(
        "/api/woo/products/:id/attributes/:attrId",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "product id")?;
                let attr_id = parse_id(ctx, "attrId", "attribute id")?;
                let deleted = delete_attribute(ctx.db.as_ref(), id, attr_id).await?;
                log_action(ctx, "woo.attribute_delete", "product", id).await;
                ok_json(json!({ "deleted": deleted }), 200)
            })
        },
        ADMIN,
    );

    // ----------------------------------------------------- variations
    router.get(
        "/api/woo/products/:id/variations",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "product id")?;
                ok(list_variations(ctx.db.as_ref(), id).await?, 200)
            })
        },
        RouteOptions::default(),
    );

    router.get(
        "/api/woo/products/:id/variations/:varId",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "product id")?;
                let var_id = parse_id(ctx, "varId", "variation id")?;
                let variation = get_variation_for_product(ctx.db.as_ref(), id, var_id)
                    .await?
                    .ok_or_else(|| not_found(&format!("Variation {var_id} not found.")))?;
                ok(variation, 200)
            })
        },
        RouteOptions::default(),
    );

    router.post(
        "/api/woo/products/:id/variations",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "product id")?;
                let body = read_json_body(ctx)?;
                let input: VariationInput = serde_json::from_value(body)
                    .map_err(|_| bad_request("Invalid variation data."))?;
                let variation = create_variation(ctx.db.as_ref(), id, input).await?;
                log_action(ctx, "woo.variation_create", "product", id).await;
                ok(variation, 201)
            })
        },
        ADMIN,
    );

    router.put(
        "/api/woo/products/:id/variations/:varId",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "product id")?;
                let var_id = parse_id(ctx, "varId", "variation id")?;
                let body = read_json_body(ctx)?;
                let input: VariationInput = serde_json::from_value(body)
                    .map_err(|_| bad_request("Invalid variation data."))?;
                let variation = update_variation(ctx.db.as_ref(), id, var_id, input).await?;
                log_action(ctx, "woo.variation_update", "product", id).await;
                ok(variation, 200)
            })
        },
        ADMIN,
    );

    router.delete(
        "/api/woo/products/:id/variations/:varId",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "product id")?;
                let var_id = parse_id(ctx, "varId", "variation id")?;
                let deleted = delete_variation(ctx.db.as_ref(), id, var_id).await?;
                log_action(ctx, "woo.variation_delete", "product", id).await;
                ok_json(json!({ "deleted": deleted }), 200)
            })
        },
        ADMIN,
    );

    // ----------------------------------------------------------- cart
    router.get(
        "/api/woo/cart",
        |ctx| {
            Box::pin(async move {
                let (token, set_cookie) = cart_token(ctx);
                let cart = get_cart(ctx.db.as_ref(), &token).await?;
                let mut resp = ok(cart, 200)?;
                if let Some(cookie) = set_cookie {
                    resp = resp.with_header("Set-Cookie", &cookie);
                }
                Ok(resp)
            })
        },
        RouteOptions::default(),
    );

    router.post(
        "/api/woo/cart",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let (token, set_cookie) = cart_token(ctx);
                let product_id = body["productId"]
                    .as_i64()
                    .ok_or_else(|| bad_request("Field \"productId\" must be an integer."))?;
                let cart = add_to_cart_variation(
                    ctx.db.as_ref(),
                    &token,
                    product_id,
                    &body["quantity"],
                    body["variationId"].as_i64(),
                )
                .await?;
                let mut resp = ok(cart, 200)?;
                if let Some(cookie) = set_cookie {
                    resp = resp.with_header("Set-Cookie", &cookie);
                }
                Ok(resp)
            })
        },
        RouteOptions::default(),
    );

    router.put(
        "/api/woo/cart",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let (token, _) = cart_token(ctx);
                let product_id = body["productId"]
                    .as_i64()
                    .ok_or_else(|| bad_request("Field \"productId\" must be an integer."))?;
                let cart = set_cart_quantity_variation(
                    ctx.db.as_ref(),
                    &token,
                    product_id,
                    &body["quantity"],
                    body["variationId"].as_i64(),
                )
                .await?;
                ok(cart, 200)
            })
        },
        RouteOptions::default(),
    );

    router.delete(
        "/api/woo/cart",
        |ctx| {
            Box::pin(async move {
                let (token, _) = cart_token(ctx);
                clear_cart(ctx.db.as_ref(), &token).await?;
                ok_json(json!({ "cleared": true }), 200)
            })
        },
        RouteOptions::default(),
    );

    // -------------------------------------------- quote and checkout
    router.post(
        "/api/woo/quote",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let (token, _) = cart_token(ctx);
                let totals =
                    compute_totals_for_cart(ctx.db.as_ref(), &token, &totals_input(&body), false)
                        .await?;
                ok(totals, 200)
            })
        },
        RouteOptions::default(),
    );

    router.post(
        "/api/woo/checkout",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let (token, _) = cart_token(ctx);
                // P8: validate the Woo totals (strict — a bad coupon is
                // a 400) BEFORE resolving the payment provider, so a
                // bad coupon is reported as a 400 even when no provider
                // is selected yet (fixed 2026-11-20: the route resolved
                // the provider first and answered 503
                // `payment_not_configured`). start_woo_checkout
                // recomputes the totals — one extra read, kept so the
                // order-creation path stays single-sourced.
                compute_totals_for_cart(ctx.db.as_ref(), &token, &totals_input(&body), true)
                    .await?;
                let resolved =
                    resolve_payment_provider(ctx.db.as_ref(), &ctx.payment_secrets).await?;
                let gateway = gateway_for(&resolved.provider_id, &ctx.payment_secrets)
                    .unwrap_or_else(|| Rc::new(MockCheckoutGateway) as Rc<dyn CheckoutGateway>);
                // P5 F2: checkout URLs come from the stored site URL,
                // never from the request host.
                let base = site_url(ctx.db.as_ref()).await?;
                let outcome = start_woo_checkout(
                    ctx.db.as_ref(),
                    &ctx.payment_secrets,
                    gateway.as_ref(),
                    &token,
                    &body["customerName"],
                    &body["customerEmail"],
                    &totals_input(&body),
                    &base,
                    &format!("{base}/shop/checkout/success"),
                    &format!("{base}/shop/checkout/cancelled"),
                )
                .await?;
                ok(outcome, 201)
            })
        },
        RouteOptions::default(),
    );

    router.post(
        "/api/woo/coupons/validate",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let code = opt_str(&body, "code")
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| bad_request("Field \"code\" is required."))?;
                let (token, _) = cart_token(ctx);
                let cart = get_cart(ctx.db.as_ref(), &token).await?;
                let subtotal = match body.get("subtotalMinor").and_then(|v| v.as_i64()) {
                    Some(v) => v,
                    None => cart.total_minor,
                };
                let currency = opt_str(&body, "currency").unwrap_or(cart.currency);
                let validation =
                    validate_coupon(ctx.db.as_ref(), &code, subtotal, &currency).await?;
                ok_json(
                    json!({
                        "code": code,
                        "valid": validation.reason.is_none(),
                        "coupon": validation.coupon,
                        "discountMinor": validation.discount_minor,
                        "reason": validation.reason,
                    }),
                    200,
                )
            })
        },
        RouteOptions::default(),
    );

    // ---------------------------------------------------------- coupons
    router.get(
        "/api/woo/coupons",
        |ctx| Box::pin(async move { ok(list_coupons(ctx.db.as_ref()).await?, 200) }),
        ADMIN,
    );

    router.post(
        "/api/woo/coupons",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let input: CouponInput = serde_json::from_value(body)
                    .map_err(|_| bad_request("Invalid coupon data."))?;
                let coupon = create_coupon(ctx.db.as_ref(), input).await?;
                log_action(ctx, "woo.coupon_create", "coupon", coupon.id).await;
                ok(coupon, 201)
            })
        },
        ADMIN,
    );

    router.get(
        "/api/woo/coupons/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "coupon id")?;
                let coupon = get_coupon(ctx.db.as_ref(), id)
                    .await?
                    .ok_or_else(|| not_found(&format!("Coupon {id} not found.")))?;
                ok(coupon, 200)
            })
        },
        ADMIN,
    );

    router.put(
        "/api/woo/coupons/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "coupon id")?;
                let body = read_json_body(ctx)?;
                let input: CouponInput = serde_json::from_value(body)
                    .map_err(|_| bad_request("Invalid coupon data."))?;
                let coupon = update_coupon(ctx.db.as_ref(), id, input).await?;
                log_action(ctx, "woo.coupon_update", "coupon", id).await;
                ok(coupon, 200)
            })
        },
        ADMIN,
    );

    router.delete(
        "/api/woo/coupons/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "coupon id")?;
                let deleted = delete_coupon(ctx.db.as_ref(), id).await?;
                log_action(ctx, "woo.coupon_delete", "coupon", id).await;
                ok_json(json!({ "deleted": deleted }), 200)
            })
        },
        ADMIN,
    );

    // ------------------------------------------------------------ taxes
    router.get(
        "/api/woo/tax-rates",
        |ctx| Box::pin(async move { ok(list_tax_rates(ctx.db.as_ref()).await?, 200) }),
        ADMIN,
    );

    router.post(
        "/api/woo/tax-rates",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let input: TaxRateInput = serde_json::from_value(body)
                    .map_err(|_| bad_request("Invalid tax rate data."))?;
                let rate = create_tax_rate(ctx.db.as_ref(), input).await?;
                log_action(ctx, "woo.tax_rate_create", "tax_rate", rate.id).await;
                ok(rate, 201)
            })
        },
        ADMIN,
    );

    router.put(
        "/api/woo/tax-rates/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "tax rate id")?;
                let body = read_json_body(ctx)?;
                let input: TaxRateInput = serde_json::from_value(body)
                    .map_err(|_| bad_request("Invalid tax rate data."))?;
                let rate = update_tax_rate(ctx.db.as_ref(), id, input).await?;
                log_action(ctx, "woo.tax_rate_update", "tax_rate", id).await;
                ok(rate, 200)
            })
        },
        ADMIN,
    );

    router.delete(
        "/api/woo/tax-rates/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "tax rate id")?;
                let deleted = delete_tax_rate(ctx.db.as_ref(), id).await?;
                log_action(ctx, "woo.tax_rate_delete", "tax_rate", id).await;
                ok_json(json!({ "deleted": deleted }), 200)
            })
        },
        ADMIN,
    );

    router.get(
        "/api/woo/tax-settings",
        |ctx| {
            Box::pin(async move {
                ok_json(
                    json!({
                        "taxEnabled": tax_enabled(ctx.db.as_ref()).await?,
                        "pricesIncludeTax": prices_include_tax(ctx.db.as_ref()).await?,
                    }),
                    200,
                )
            })
        },
        ADMIN,
    );

    router.put(
        "/api/woo/tax-settings",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let enabled = body
                    .get("taxEnabled")
                    .and_then(|v| v.as_bool())
                    .ok_or_else(|| bad_request("Field \"taxEnabled\" must be true or false."))?;
                let inclusive = body
                    .get("pricesIncludeTax")
                    .and_then(|v| v.as_bool())
                    .ok_or_else(|| {
                        bad_request("Field \"pricesIncludeTax\" must be true or false.")
                    })?;
                set_tax_settings(ctx.db.as_ref(), enabled, inclusive).await?;
                log_action(ctx, "woo.tax_settings_update", "option", 0).await;
                ok_json(
                    json!({ "taxEnabled": enabled, "pricesIncludeTax": inclusive }),
                    200,
                )
            })
        },
        ADMIN,
    );

    // --------------------------------------------------------- shipping
    router.get(
        "/api/woo/shipping-zones",
        |ctx| {
            Box::pin(async move {
                let zones = list_zones_with_methods(ctx.db.as_ref()).await?;
                let zones: Vec<Value> = zones
                    .into_iter()
                    .map(|(zone, methods)| json!({ "zone": zone, "methods": methods }))
                    .collect();
                ok(zones, 200)
            })
        },
        ADMIN,
    );

    router.post(
        "/api/woo/shipping-zones",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let input: ShippingZoneInput = serde_json::from_value(body)
                    .map_err(|_| bad_request("Invalid shipping zone data."))?;
                let zone = create_zone(ctx.db.as_ref(), input).await?;
                log_action(ctx, "woo.shipping_zone_create", "shipping_zone", zone.id).await;
                ok(zone, 201)
            })
        },
        ADMIN,
    );

    router.put(
        "/api/woo/shipping-zones/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "shipping zone id")?;
                let body = read_json_body(ctx)?;
                let input: ShippingZoneInput = serde_json::from_value(body)
                    .map_err(|_| bad_request("Invalid shipping zone data."))?;
                let zone = update_zone(ctx.db.as_ref(), id, input).await?;
                log_action(ctx, "woo.shipping_zone_update", "shipping_zone", id).await;
                ok(zone, 200)
            })
        },
        ADMIN,
    );

    router.delete(
        "/api/woo/shipping-zones/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "shipping zone id")?;
                let deleted = delete_zone(ctx.db.as_ref(), id).await?;
                log_action(ctx, "woo.shipping_zone_delete", "shipping_zone", id).await;
                ok_json(json!({ "deleted": deleted }), 200)
            })
        },
        ADMIN,
    );

    router.get(
        "/api/woo/shipping-zones/:id/methods",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "shipping zone id")?;
                ok(list_methods(ctx.db.as_ref(), id).await?, 200)
            })
        },
        ADMIN,
    );

    router.post(
        "/api/woo/shipping-zones/:id/methods",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "shipping zone id")?;
                let body = read_json_body(ctx)?;
                let input: ShippingMethodInput = serde_json::from_value(body)
                    .map_err(|_| bad_request("Invalid shipping method data."))?;
                let method = create_method(ctx.db.as_ref(), id, input).await?;
                log_action(ctx, "woo.shipping_method_create", "shipping_zone", id).await;
                ok(method, 201)
            })
        },
        ADMIN,
    );

    router.put(
        "/api/woo/shipping-methods/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "shipping method id")?;
                let body = read_json_body(ctx)?;
                let zone_id = body["zoneId"]
                    .as_i64()
                    .ok_or_else(|| bad_request("Field \"zoneId\" must be an integer."))?;
                let input: ShippingMethodInput = serde_json::from_value(body)
                    .map_err(|_| bad_request("Invalid shipping method data."))?;
                let method = update_method(ctx.db.as_ref(), zone_id, id, input).await?;
                log_action(ctx, "woo.shipping_method_update", "shipping_method", id).await;
                ok(method, 200)
            })
        },
        ADMIN,
    );

    router.delete(
        "/api/woo/shipping-methods/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "shipping method id")?;
                let zone_id = ctx
                    .query("zoneId")
                    .and_then(|s| s.parse::<i64>().ok())
                    .ok_or_else(|| bad_request("Query \"zoneId\" must be an integer."))?;
                let deleted = delete_method(ctx.db.as_ref(), zone_id, id).await?;
                log_action(ctx, "woo.shipping_method_delete", "shipping_method", id).await;
                ok_json(json!({ "deleted": deleted }), 200)
            })
        },
        ADMIN,
    );

    router.post(
        "/api/woo/shipping/quote",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let country = opt_str(&body, "country").unwrap_or_default();
                let (token, _) = cart_token(ctx);
                let cart = get_cart(ctx.db.as_ref(), &token).await?;
                let subtotal = match body.get("subtotalMinor").and_then(|v| v.as_i64()) {
                    Some(v) => v,
                    None => cart.total_minor,
                };
                let quote = quote_shipping(
                    ctx.db.as_ref(),
                    &country,
                    subtotal,
                    opt_i64(&body, "shippingMethodId"),
                    &cart.currency,
                )
                .await?;
                ok_json(
                    json!({
                        "method": quote.method,
                        "costMinor": quote.cost_minor,
                        "note": quote.note,
                    }),
                    200,
                )
            })
        },
        RouteOptions::default(),
    );

    // -------------------------------------------------------- customers
    router.get(
        "/api/woo/customers",
        |ctx| {
            Box::pin(async move {
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                let (customers, total) =
                    list_customers(ctx.db.as_ref(), page, per_page).await?;
                ok_json(
                    json!({ "customers": customers, "total": total, "page": page, "perPage": per_page }),
                    200,
                )
            })
        },
        ADMIN,
    );

    router.get(
        "/api/woo/customers/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "customer id")?;
                let customer = get_customer(ctx.db.as_ref(), id)
                    .await?
                    .ok_or_else(|| not_found(&format!("Customer {id} not found.")))?;
                ok(customer, 200)
            })
        },
        ADMIN,
    );

    // ------------------------------------------------------------ import
    router.post(
        "/api/woo/import",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let kind = opt_str(&body, "kind")
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| {
                        bad_request("Field \"kind\" is required (\"products\", \"orders\" or \"customers\").")
                    })?;
                let csv = opt_str(&body, "csv")
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| bad_request("Field \"csv\" is required."))?;
                let report = import_csv(ctx.db.as_ref(), &kind, &csv).await?;
                log_action(ctx, "woo.import", "import", 0).await;
                ok(report, 200)
            })
        },
        ADMIN,
    );

    // SKU lookups (admin): a variation by its SKU, for the admin UI
    // and the import report.
    router.get(
        "/api/woo/variations/sku/:sku",
        |ctx| {
            Box::pin(async move {
                let sku = ctx.param("sku").unwrap_or("").to_string();
                // get_variation_by_sku returns (product_id, variation_id)
                // — the route returns the full Variation object, not
                // the tuple (fixed 2026-11-20: smoke caught `data.stock`
                // missing from the tuple response).
                let (product_id, variation_id) = get_variation_by_sku(ctx.db.as_ref(), &sku)
                    .await?
                    .ok_or_else(|| {
                        not_found(&format!("Variation with SKU \"{sku}\" not found."))
                    })?;
                let variation =
                    get_variation_for_product(ctx.db.as_ref(), product_id, variation_id)
                        .await?
                        .ok_or_else(|| {
                            not_found(&format!("Variation with SKU \"{sku}\" not found."))
                        })?;
                ok(variation, 200)
            })
        },
        ADMIN,
    );

    // A coupon by code (admin): the same validation the storefront
    // uses, without a cart.
    router.get(
        "/api/woo/coupons/code/:code",
        |ctx| {
            Box::pin(async move {
                let code = ctx.param("code").unwrap_or("").to_string();
                let coupon = get_coupon_by_code(ctx.db.as_ref(), &code)
                    .await?
                    .ok_or_else(|| not_found(&format!("Coupon \"{code}\" not found.")))?;
                ok(coupon, 200)
            })
        },
        ADMIN,
    );
}
