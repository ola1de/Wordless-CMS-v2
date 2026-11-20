//! Shop pages (P3) — port of v1 `src/modules/render/service.ts`
//! shop section (`renderShop`, `renderProductPage`,
//! `renderCartPage`, `renderTestCheckout`, `renderCheckoutResult`).
//!
//! The storefront is server-rendered forms (no JavaScript): add-to-cart
//! and checkout post to `/shop/*` routes, which redirect back here.
//! Prices are formatted from integer minor units
//! (`wordless-commerce` `format_money`). Shop pages render through
//! the active block theme as a synthetic "page" post, exactly like
//! v1's `shopPost` helper; the fallback M2 layout uses the same HTML
//! plus its own `<h1>`.

use wordless_commerce::money::format_money;
use wordless_commerce::products::list_products;
use wordless_commerce::{Cart, Order, Product};
use wordless_db::Db;
use wordless_integrations::social::share_links;
use wordless_kernel::error::WordlessError;

use crate::blocks::{escape_html, plain_text_excerpt, render_content};
use crate::feeds::site_url;
use crate::pages::{
    finish_page, get_posts_per_page, nav_items, pagination, site_info, template_nav, FallbackPage,
    RenderOptions, RenderedPage,
};
use crate::templates::{TemplateContext, TemplateKind, TemplatePostData};

/// WooCommerce data for the storefront, parsed from the Woo Worker's
/// JSON (size remedy, 2026-11-20: this crate cannot depend on
/// `wordless-woocommerce` — it lives in another Worker now). Only the
/// fields the forms need; unknown fields are ignored and missing ones
/// default, so a Woo Worker answer always parses. Field names are
/// camelCase, as the Woo Worker's serde shapes.
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct WooProductView {
    pub variations: Vec<WooVariationView>,
    pub attributes: Vec<WooAttributeView>,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct WooVariationView {
    pub label: String,
    pub effective_price_minor: i64,
    pub status: String,
    pub stock: i64,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct WooAttributeView {
    pub name: String,
    pub slug: String,
    pub options: Vec<String>,
    pub is_variation: bool,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct WooZoneView {
    pub zone: WooZoneNameView,
    pub methods: Vec<WooMethodView>,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct WooZoneNameView {
    pub name: String,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct WooMethodView {
    pub id: i64,
    #[serde(rename = "type")]
    pub method_type: String,
    pub cost_minor: i64,
    pub min_order_minor: i64,
    pub name: String,
}

/// Shop pages render through the active block theme as a synthetic
/// "page" post (v1 `shopPost`).
fn shop_post(title: &str, url: &str, content_html: String) -> TemplatePostData {
    TemplatePostData {
        title: title.to_string(),
        date: String::new(),
        excerpt: String::new(),
        content_html,
        url: url.to_string(),
        comment_count: 0,
        terms: Vec::new(),
    }
}

fn stock_note(product: &Product) -> String {
    if product.stock < 0 {
        String::new()
    } else if product.stock == 0 {
        " <span class=\"stock-out\">Out of stock</span>".to_string()
    } else if product.stock <= 5 {
        format!(
            " <span class=\"stock-low\">Only {} left</span>",
            product.stock
        )
    } else {
        String::new()
    }
}

fn add_to_cart_form(product: &Product, redirect_to: &str) -> String {
    if product.stock == 0 {
        return "<p><em>Out of stock.</em></p>".to_string();
    }
    format!(
        "<form method=\"post\" action=\"/shop/cart/add\">\n<input type=\"hidden\" name=\"productId\" value=\"{}\">\n<input type=\"hidden\" name=\"redirectTo\" value=\"{}\">\n<label>Quantity <input type=\"number\" name=\"quantity\" value=\"1\" min=\"1\" max=\"99\" style=\"width:4em\"></label>\n<button type=\"submit\">Add to cart</button>\n</form>",
        product.id,
        escape_html(redirect_to)
    )
}

/// P8: the add-to-cart form for a variable product — one `<select>`
/// per variation attribute (server-rendered, no JavaScript). The
/// `/shop/cart/add` handler resolves the chosen attributes to a
/// variation id; a simple product (or a variable product whose Woo
/// detail could not be loaded) falls back to the plain form.
fn add_to_cart_form_woo(
    product: &Product,
    woo: Option<&WooProductView>,
    redirect_to: &str,
) -> String {
    let Some(woo) = woo else {
        return add_to_cart_form(product, redirect_to);
    };
    if product.product_type != "variable" {
        return add_to_cart_form(product, redirect_to);
    }
    let published: Vec<&WooVariationView> = woo
        .variations
        .iter()
        .filter(|v| v.status == "published")
        .collect();
    if published.is_empty() {
        return "<p><em>No variations available yet.</em></p>".to_string();
    }
    let selectors: Vec<String> = woo
        .attributes
        .iter()
        .filter(|a| a.is_variation)
        .map(|a| {
            let options: Vec<String> = a
                .options
                .iter()
                .map(|o| {
                    format!(
                        "<option value=\"{}\">{}</option>",
                        escape_html(o),
                        escape_html(o)
                    )
                })
                .collect();
            format!(
                "<label>{} <select name=\"attr_{}\">{}</select></label>\n",
                escape_html(&a.name),
                escape_html(&a.slug),
                options.join("")
            )
        })
        .collect();
    let rows: Vec<String> = published
        .iter()
        .map(|v| {
            let stock = if v.stock == 0 {
                " <span class=\"stock-out\">Out of stock</span>".to_string()
            } else if v.stock <= 5 {
                format!(" <span class=\"stock-low\">Only {} left</span>", v.stock)
            } else {
                String::new()
            };
            format!(
                "<li>{} — {}{}</li>",
                escape_html(&v.label),
                escape_html(&format_money(v.effective_price_minor, &product.currency)),
                stock
            )
        })
        .collect();
    format!(
        "<ul class=\"variation-list\">{}</ul>\n<form method=\"post\" action=\"/shop/cart/add\">\n<input type=\"hidden\" name=\"productId\" value=\"{}\">\n<input type=\"hidden\" name=\"redirectTo\" value=\"{}\">\n{}<label>Quantity <input type=\"number\" name=\"quantity\" value=\"1\" min=\"1\" max=\"99\" style=\"width:4em\"></label>\n<button type=\"submit\">Add to cart</button>\n</form>",
        rows.join(""),
        product.id,
        escape_html(redirect_to),
        selectors.join("")
    )
}

/// The shop listing (`GET /shop`). `cart` is the visitor's current
/// cart, when one exists (v1 reads it from `opts.cartToken`).
pub async fn render_shop(
    db: &dyn Db,
    site_id: i64,
    page: i64,
    cart: Option<&Cart>,
    opts: &RenderOptions,
) -> Result<RenderedPage, WordlessError> {
    let (site_name, site_description) = site_info(db, site_id).await?;
    let nav = nav_items(db, site_id).await?;
    let per_page = get_posts_per_page(db, site_id).await?;
    let result = list_products(db, Some("published"), page, per_page).await?;
    let cart_link = match cart {
        Some(c) if !c.lines.is_empty() => format!(
            "<p><a href=\"/shop/cart\">View cart ({} item{} — {})</a></p>",
            c.lines.len(),
            if c.lines.len() == 1 { "" } else { "s" },
            escape_html(&format_money(c.total_minor, &c.currency))
        ),
        _ => String::new(),
    };
    let inner = if result.products.is_empty() {
        format!("{cart_link}<p>No products for sale yet.</p>")
    } else {
        let items: Vec<String> = result
            .products
            .iter()
            .map(|p| {
                let desc = if p.description.is_empty() {
                    String::new()
                } else {
                    format!(
                        "<p>{}</p>",
                        escape_html(&plain_text_excerpt(&p.description, 160))
                    )
                };
                format!(
                    "<li><h2><a href=\"/shop/{}\">{}</a></h2><p class=\"price\">{}</p>{}{}</li>",
                    escape_html(&p.slug),
                    escape_html(&p.name),
                    escape_html(&format_money(p.price_minor, &p.currency)),
                    stock_note(p),
                    desc
                )
            })
            .collect();
        format!(
            "{cart_link}<ul class=\"product-list\">{}</ul>{}",
            items.join(""),
            pagination(page, result.total_pages, "/shop", "")
        )
    };
    let body = format!("<h1>Shop</h1>{inner}");
    let mut ctx = TemplateContext::new(
        &site_name,
        &site_description,
        template_nav(&nav),
        TemplateKind::Page,
    );
    ctx.title = "Shop".to_string();
    ctx.post = Some(shop_post("Shop", "/shop", inner));
    ctx.page = page;
    ctx.total_pages = result.total_pages;
    ctx.base_url = "/shop".to_string();
    finish_page(
        db,
        opts,
        ctx,
        FallbackPage {
            title: "Shop".to_string(),
            site_name: site_name.clone(),
            description: site_description.clone(),
            nav,
            body,
            head_extra: None,
            status: 200,
        },
        None,
    )
    .await
}

/// One product (`GET /shop/:slug`).
pub async fn render_product_page(
    db: &dyn Db,
    site_id: i64,
    product: &Product,
    opts: &RenderOptions,
) -> Result<RenderedPage, WordlessError> {
    let (site_name, site_description) = site_info(db, site_id).await?;
    let nav = nav_items(db, site_id).await?;
    let url = format!("/shop/{}", product.slug);
    let share = share_links(&format!("{}{}", site_url(db, site_id).await?, url), &product.name);
    let share_html = format!(
        "<section class=\"share\"><h2>Share this</h2><p><a href=\"{}\" target=\"_blank\" rel=\"noreferrer\">X</a> · <a href=\"{}\" target=\"_blank\" rel=\"noreferrer\">Facebook</a> · <a href=\"{}\" target=\"_blank\" rel=\"noreferrer\">WhatsApp</a> · <a href=\"{}\" target=\"_blank\" rel=\"noreferrer\">Telegram</a></p></section>",
        escape_html(&share.x),
        escape_html(&share.facebook),
        escape_html(&share.whatsapp),
        escape_html(&share.telegram)
    );
    let desc = if product.description.is_empty() {
        "<p>No description yet.</p>".to_string()
    } else {
        render_content(&product.description)
    };
    // P8: variable products get the attribute selectors + variation
    // list; the parent's own stock is ignored for them (real stock
    // is per variation) and its price is the minimum variation
    // price, shown as "From …".
    // Size remedy: the Woo detail comes from the Woo Worker via
    // RenderOptions. Best-effort — a missing/unreachable Woo Worker
    // falls back to the plain form, same as a variable product whose
    // Woo detail could not be loaded before the split.
    let woo: Option<WooProductView> = if product.product_type == "variable" {
        match &opts.woo_client {
            Some(client) => client
                .get_product(product.id)
                .await
                .ok()
                .and_then(|v| serde_json::from_value(v).ok()),
            None => None,
        }
    } else {
        None
    };
    let is_variable = woo.is_some();
    let price_html = if is_variable {
        format!(
            "From {}",
            format_money(product.price_minor, &product.currency)
        )
    } else {
        format_money(product.price_minor, &product.currency)
    };
    let stock_html = if is_variable {
        String::new()
    } else {
        stock_note(product)
    };
    let form_html = add_to_cart_form_woo(product, woo.as_ref(), &url);
    let inner = format!(
        "<article>\n<p class=\"price\">{}</p>{}\n{}\n{}\n<p><a href=\"/shop\">&larr; Back to the shop</a></p>\n</article>\n{}",
        escape_html(&price_html),
        stock_html,
        desc,
        form_html,
        share_html
    );
    let body = format!(
        "<article>\n<h1>{}</h1>\n<p class=\"price\">{}</p>{}\n{}\n{}\n<p><a href=\"/shop\">&larr; Back to the shop</a></p>\n</article>\n{}",
        escape_html(&product.name),
        escape_html(&price_html),
        stock_html,
        desc,
        form_html,
        share_html
    );
    let mut ctx = TemplateContext::new(
        &site_name,
        &site_description,
        template_nav(&nav),
        TemplateKind::Page,
    );
    ctx.title = product.name.clone();
    ctx.post = Some(shop_post(&product.name, &url, inner));
    ctx.page = 1;
    ctx.total_pages = 1;
    ctx.base_url = url.clone();
    finish_page(
        db,
        opts,
        ctx,
        FallbackPage {
            title: product.name.clone(),
            site_name: site_name.clone(),
            description: site_description.clone(),
            nav,
            body,
            head_extra: None,
            status: 200,
        },
        None,
    )
    .await
}

/// The cart (`GET /shop/cart`).
pub async fn render_cart_page(
    db: &dyn Db,
    site_id: i64,
    cart: &Cart,
    opts: &RenderOptions,
    notice: Option<&str>,
) -> Result<RenderedPage, WordlessError> {
    let (site_name, site_description) = site_info(db, site_id).await?;
    let nav = nav_items(db, site_id).await?;
    let notice_html = notice
        .map(|n| format!("<p class=\"notice\">{}</p>", escape_html(n)))
        .unwrap_or_default();
    let lines = if cart.lines.is_empty() {
        "<p>Your cart is empty.</p>".to_string()
    } else {
        let rows: Vec<String> = cart
            .lines
            .iter()
            .map(|l| {
                // P8: a variation line shows its label ("Size: M,
                // Colour: Red") and carries its variation id in both
                // forms, so quantity/removal hit the right line.
                let label = l
                    .variation_label
                    .as_ref()
                    .map(|v| format!("<br><small>{}</small>", escape_html(v)))
                    .unwrap_or_default();
                let variation_field = l
                    .variation_id
                    .map(|v| {
                        format!(
                            "<input type=\"hidden\" name=\"variationId\" value=\"{}\">",
                            v
                        )
                    })
                    .unwrap_or_default();
                format!(
                    "<tr><td><a href=\"/shop/{}\">{}</a>{}</td><td>{}</td><td><form method=\"post\" action=\"/shop/cart/quantity\" style=\"display:inline\"><input type=\"hidden\" name=\"productId\" value=\"{}\">{}<input type=\"number\" name=\"quantity\" value=\"{}\" min=\"0\" max=\"99\" style=\"width:4em\"><button type=\"submit\">Update</button></form></td><td>{}</td><td><form method=\"post\" action=\"/shop/cart/remove\" style=\"display:inline\"><input type=\"hidden\" name=\"productId\" value=\"{}\">{}<button type=\"submit\">Remove</button></form></td></tr>",
                    escape_html(&l.slug),
                    escape_html(&l.name),
                    label,
                    escape_html(&format_money(l.price_minor, &l.currency)),
                    l.product_id,
                    variation_field,
                    l.quantity,
                    escape_html(&format_money(l.line_total_minor, &l.currency)),
                    l.product_id,
                    variation_field
                )
            })
            .collect();
        let currency_note = cart
            .currency_note
            .as_ref()
            .map(|n| format!("<p><em>{}</em></p>", escape_html(n)))
            .unwrap_or_default();
        // P8: the checkout form takes a coupon code, the customer's
        // country (tax + shipping), and an optional shipping method.
        // Methods are listed grouped by zone; the empty choice means
        // "cheapest applicable" (free shipping when its threshold is
        // met). The checkout validates the method against the zone
        // for the country given and 303s back with a notice when it
        // does not belong there.
        // Size remedy: shipping zones come from the Woo Worker via
        // RenderOptions. Best-effort — a missing/unreachable Woo
        // Worker means no shipping-method selector (the checkout
        // still validates server-side).
        let zones: Vec<WooZoneView> = match &opts.woo_client {
            Some(client) => client
                .zones()
                .await
                .ok()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let method_options: Vec<String> = zones
            .iter()
            .flat_map(|z| {
                z.methods.iter().map(|m| {
                    let cost = if m.method_type == "free_shipping" {
                        format!(
                            "free over {}",
                            format_money(m.min_order_minor, &cart.currency)
                        )
                    } else {
                        format_money(m.cost_minor, &cart.currency)
                    };
                    format!(
                        "<option value=\"{}\">{} — {} ({})</option>",
                        m.id,
                        escape_html(&z.zone.name),
                        escape_html(&m.name),
                        escape_html(&cost)
                    )
                })
            })
            .collect();
        let method_field = if method_options.is_empty() {
            String::new()
        } else {
            format!(
                "<label for=\"checkout-shipping\">Shipping method</label>\n<select id=\"checkout-shipping\" name=\"shippingMethodId\"><option value=\"\">Cheapest available</option>{}</select>\n",
                method_options.join("")
            )
        };
        format!(
            "<table><thead><tr><th>Product</th><th>Price</th><th>Quantity</th><th>Total</th><th></th></tr></thead><tbody>{}</tbody></table>\n<p><strong>Total: {}</strong></p>{}<h2>Checkout</h2>\n<form method=\"post\" action=\"/shop/checkout\">\n<label for=\"checkout-name\">Name</label>\n<input id=\"checkout-name\" name=\"customerName\" required maxlength=\"200\">\n<label for=\"checkout-email\">Email</label>\n<input id=\"checkout-email\" name=\"customerEmail\" type=\"email\" required maxlength=\"100\">\n<label for=\"checkout-coupon\">Coupon code (optional)</label>\n<input id=\"checkout-coupon\" name=\"couponCode\" maxlength=\"50\">\n<label for=\"checkout-country\">Country (2-letter code, for tax and shipping)</label>\n<input id=\"checkout-country\" name=\"country\" maxlength=\"2\" style=\"width:4em\">\n{}<button type=\"submit\">Continue to payment</button>\n</form>\n<p><small>Payment is processed by the store's configured payment provider. No payment provider is configured yet? The checkout will say so instead of taking your money.</small></p>",
            rows.join(""),
            escape_html(&format_money(cart.total_minor, &cart.currency)),
            currency_note,
            method_field
        )
    };
    let body = format!("<h1>Your cart</h1>{notice_html}{lines}<p><a href=\"/shop\">&larr; Continue shopping</a></p>");
    let inner =
        format!("{notice_html}{lines}<p><a href=\"/shop\">&larr; Continue shopping</a></p>");
    let mut ctx = TemplateContext::new(
        &site_name,
        &site_description,
        template_nav(&nav),
        TemplateKind::Page,
    );
    ctx.title = "Your cart".to_string();
    ctx.post = Some(shop_post("Your cart", "/shop/cart", inner));
    ctx.page = 1;
    ctx.total_pages = 1;
    ctx.base_url = "/shop/cart".to_string();
    finish_page(
        db,
        opts,
        ctx,
        FallbackPage {
            title: "Your cart".to_string(),
            site_name: site_name.clone(),
            description: site_description.clone(),
            nav,
            body,
            head_extra: None,
            status: 200,
        },
        None,
    )
    .await
}

/// Mock (test-mode) checkout page: explains that no money moves and
/// offers the simulated payment.
pub async fn render_test_checkout(
    db: &dyn Db,
    site_id: i64,
    order: &Order,
    reference: &str,
    opts: &RenderOptions,
) -> Result<RenderedPage, WordlessError> {
    let (site_name, site_description) = site_info(db, site_id).await?;
    let nav = nav_items(db, site_id).await?;
    let inner = format!(
        "<p><strong>This is a test checkout — no money moves.</strong> The store is using the mock payment provider (local development / tests only).</p>\n<p>Order {} — total {}.</p>\n<form method=\"post\" action=\"/shop/checkout/test\">\n<input type=\"hidden\" name=\"orderId\" value=\"{}\">\n<input type=\"hidden\" name=\"reference\" value=\"{}\">\n<button type=\"submit\">Simulate successful payment (test only)</button>\n</form>\n<p><a href=\"/shop/cart\">Back to cart</a></p>",
        escape_html(&order.id),
        escape_html(&format_money(order.total_minor, &order.currency)),
        escape_html(&order.id),
        escape_html(reference)
    );
    let body = format!("<h1>Test checkout</h1>\n{inner}");
    let mut ctx = TemplateContext::new(
        &site_name,
        &site_description,
        template_nav(&nav),
        TemplateKind::Page,
    );
    ctx.title = "Test checkout".to_string();
    ctx.post = Some(shop_post("Test checkout", "/shop/checkout/test", inner));
    ctx.page = 1;
    ctx.total_pages = 1;
    ctx.base_url = "/shop/checkout/test".to_string();
    finish_page(
        db,
        opts,
        ctx,
        FallbackPage {
            title: "Test checkout".to_string(),
            site_name: site_name.clone(),
            description: site_description.clone(),
            nav,
            body,
            head_extra: None,
            status: 200,
        },
        None,
    )
    .await
}

/// Checkout result (`GET /shop/checkout/success` and `/cancelled`).
pub async fn render_checkout_result(
    db: &dyn Db,
    site_id: i64,
    order: Option<&Order>,
    opts: &RenderOptions,
) -> Result<RenderedPage, WordlessError> {
    let (site_name, site_description) = site_info(db, site_id).await?;
    let nav = nav_items(db, site_id).await?;
    let (title, body, inner) = match order {
        None => (
            "Checkout".to_string(),
            "<h1>Order not found</h1><p>We could not find that order. <a href=\"/shop\">Back to the shop</a>.</p>".to_string(),
            "<p>We could not find that order. <a href=\"/shop\">Back to the shop</a>.</p>".to_string(),
        ),
        Some(o) => {
            let items: Vec<String> = o
                .items
                .iter()
                .map(|i| {
                    format!(
                        "<li>{} × {} — {}</li>",
                        escape_html(&i.name),
                        i.quantity,
                        escape_html(&format_money(i.price_minor * i.quantity, &o.currency))
                    )
                })
                .collect();
            let heading = if o.status == "paid" || o.status == "fulfilled" {
                "Thank you — your order is confirmed".to_string()
            } else {
                format!("Order {}", o.status)
            };
            let pending_note = if o.status == "pending" {
                " — payment has not been confirmed yet. If you paid, the provider webhook may still be on its way; this page refreshes the status."
            } else {
                ""
            };
            let body = format!(
                "<h1>{}</h1>\n<p>Order {} — {}.</p>\n<ul>{}</ul>\n<p>Status: <strong>{}</strong>{}</p>\n<p><a href=\"/shop\">Back to the shop</a></p>",
                escape_html(&heading),
                escape_html(&o.id),
                escape_html(&format_money(o.total_minor, &o.currency)),
                items.join(""),
                escape_html(&o.status),
                pending_note
            );
            let inner = format!(
                "<p>Order {} — {}.</p>\n<ul>{}</ul>\n<p>Status: <strong>{}</strong>{}</p>\n<p><a href=\"/shop\">Back to the shop</a></p>",
                escape_html(&o.id),
                escape_html(&format_money(o.total_minor, &o.currency)),
                items.join(""),
                escape_html(&o.status),
                pending_note
            );
            (heading, body, inner)
        }
    };
    let mut ctx = TemplateContext::new(
        &site_name,
        &site_description,
        template_nav(&nav),
        TemplateKind::Page,
    );
    ctx.title = title.clone();
    ctx.post = Some(shop_post(&title, "/shop/checkout/success", inner));
    ctx.page = 1;
    ctx.total_pages = 1;
    ctx.base_url = "/shop/checkout/success".to_string();
    finish_page(
        db,
        opts,
        ctx,
        FallbackPage {
            title,
            site_name: site_name.clone(),
            description: site_description.clone(),
            nav,
            body,
            head_extra: None,
            status: 200,
        },
        None,
    )
    .await
}
