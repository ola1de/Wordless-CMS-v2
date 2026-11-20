//! Server-side cart (port of v1 `src/modules/commerce/cart.ts`).
//!
//! The cart lives in D1 (`wl_carts`) keyed by the `wordless_cart`
//! cookie token — no account needed, no personal data stored. Items
//! are a JSON array of `{ productId, quantity }`. Carts expire 7 days
//! after their last update and are pruned on read. Stock is checked at
//! add time AND at checkout (stock can change between the two).

use serde_json::Value;
use wordless_db::row::row_text;
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, not_found, WordlessError};
use wordless_kernel::time::{ms_from_iso, now_iso, now_ms};

use crate::ids::new_uuid_v4;
use crate::products::get_product;
use crate::types::{Cart, CartItem, CartLine};
use crate::validate::require_int;

/// The cart cookie name.
pub const CART_COOKIE: &str = "wordless_cart";
/// Cart lifetime: 7 days, in milliseconds.
pub const CART_TTL_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// Parse the cart token out of a `Cookie` header value. Returns `None`
/// for a missing header or a malformed token. Port of v1
/// `parseCartCookie` (token shape `[A-Za-z0-9-]{10,80}`).
pub fn parse_cart_cookie(cookie_header: Option<&str>) -> Option<String> {
    let header = cookie_header?;
    if header.is_empty() {
        return None;
    }
    for part in header.split(';') {
        let part = part.trim();
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        if name.trim() == CART_COOKIE {
            let value = value.trim();
            let ok = (10..=80).contains(&value.len())
                && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
            return if ok { Some(value.to_string()) } else { None };
        }
    }
    None
}

/// `Set-Cookie` value for a cart token. Port of v1 `cartCookieHeader`.
pub fn cart_cookie_header(token: &str) -> String {
    format!(
        "{CART_COOKIE}={token}; Path=/; Max-Age={}; SameSite=Lax",
        CART_TTL_MS / 1000
    )
}

fn parse_items(raw: &str) -> Vec<CartItem> {
    let parsed: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    let Value::Array(items) = parsed else {
        return vec![];
    };
    items
        .iter()
        .filter_map(|item| {
            let product_id = item.get("productId")?.as_i64()?;
            let quantity = item.get("quantity")?.as_i64()?;
            // P8: rows written before variations carry no variationId.
            let variation_id = item
                .get("variationId")
                .and_then(|v| v.as_i64())
                .filter(|v| *v >= 1);
            Some(CartItem {
                product_id,
                quantity,
                variation_id,
            })
        })
        .collect()
}

struct CartRow {
    items: String,
    updated_at: String,
}

fn is_expired(updated_at: &str) -> bool {
    match ms_from_iso(updated_at) {
        Some(t) => now_ms().saturating_sub(t) > CART_TTL_MS,
        // Unparseable timestamp: v1 treats `NaN` as expired.
        None => true,
    }
}

async fn load_cart_row(db: &dyn Db, token: &str) -> Result<Option<CartRow>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_carts WHERE id = ? LIMIT 1")
        .bind(vec![Param::Text(token.to_string())])
        .first()
        .await?;
    let Some(row) = row else { return Ok(None) };
    let cart_row = CartRow {
        items: row_text(&row, "items"),
        updated_at: row_text(&row, "updated_at"),
    };
    if is_expired(&cart_row.updated_at) {
        db.prepare("DELETE FROM wl_carts WHERE id = ?")
            .bind(vec![Param::Text(token.to_string())])
            .run()
            .await?;
        return Ok(None);
    }
    Ok(Some(cart_row))
}

async fn save_cart(db: &dyn Db, token: &str, items: &[CartItem]) -> Result<(), WordlessError> {
    let now = now_iso();
    let json = serde_json::to_string(items)
        .map_err(|e| WordlessError::new("internal_error", 500, &e.to_string()))?;
    db.prepare(
        "INSERT INTO wl_carts (id, items, created_at, updated_at) VALUES (?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET items = excluded.items, updated_at = excluded.updated_at",
    )
    .bind(vec![
        Param::Text(token.to_string()),
        Param::Text(json),
        Param::Text(now.clone()),
        Param::Text(now),
    ])
    .run()
    .await?;
    Ok(())
}

/// The cart for a token, enriched with product data. Missing or draft
/// products are dropped (and the cleaned cart is persisted, so a
/// deleted product stops reappearing). P8: a variation line resolves
/// its price/stock/label from the variation row; a line whose
/// variation is missing or draft is dropped, and a variable product
/// without a variation is dropped (it cannot be bought bare).
pub async fn get_cart(db: &dyn Db, token: &str) -> Result<Cart, WordlessError> {
    let row = load_cart_row(db, token).await?;
    let items = row
        .as_ref()
        .map(|r| parse_items(&r.items))
        .unwrap_or_default();
    let mut lines: Vec<CartLine> = Vec::new();
    let mut dropped = false;
    for item in &items {
        let product = get_product(db, item.product_id).await?;
        let Some(product) = product else {
            dropped = true;
            continue;
        };
        if product.status != "published" {
            dropped = true;
            continue;
        }
        // P8: resolve the variation, when the line carries one.
        let (price_minor, variation_label) = match item.variation_id {
            Some(variation_id) => {
                let variation = crate::variation::get_variation(db, variation_id).await?;
                let Some(variation) = variation else {
                    dropped = true;
                    continue;
                };
                if variation.product_id != product.id || variation.status != "published" {
                    dropped = true;
                    continue;
                }
                (
                    crate::variation::effective_price(&variation),
                    Some(crate::variation::variation_label(
                        &variation.attributes_json,
                    )),
                )
            }
            None => {
                if product.product_type == "variable" {
                    // A variable product with no variation chosen.
                    dropped = true;
                    continue;
                }
                (product.price_minor, None)
            }
        };
        lines.push(CartLine {
            product_id: product.id,
            name: product.name,
            slug: product.slug,
            price_minor,
            currency: product.currency,
            quantity: item.quantity,
            line_total_minor: price_minor * item.quantity,
            variation_id: item.variation_id,
            variation_label,
        });
    }
    if dropped && row.is_some() {
        save_cart(
            db,
            token,
            &lines
                .iter()
                .map(|l| CartItem {
                    product_id: l.product_id,
                    quantity: l.quantity,
                    variation_id: l.variation_id,
                })
                .collect::<Vec<_>>(),
        )
        .await?;
    }
    let currency = lines
        .first()
        .map(|l| l.currency.clone())
        .unwrap_or_else(|| "NGN".to_string());
    let mixed = lines.iter().any(|l| l.currency != currency);
    Ok(Cart {
        token: token.to_string(),
        total_minor: lines.iter().map(|l| l.line_total_minor).sum(),
        currency_note: if mixed {
            Some(
                "This cart mixes currencies; the total uses the first item's currency. Check out one currency at a time."
                    .to_string(),
            )
        } else {
            None
        },
        currency,
        lines,
    })
}

/// Create a fresh cart token (the caller sets the cookie).
/// Port of v1 `newCartToken` (`crypto.randomUUID()`).
pub fn new_cart_token() -> Result<String, WordlessError> {
    new_uuid_v4()
}

fn check_stock(stock: i64, name: &str, quantity: i64) -> Result<(), WordlessError> {
    if stock >= 0 && quantity > stock {
        return Err(bad_request(&format!(
            "Only {stock} of \"{name}\" is in stock."
        )));
    }
    Ok(())
}

/// Add a quantity of a product to the cart (adding to an existing
/// line). Quantity must be 1–99; the product must be published and
/// have enough stock for the new total.
pub async fn add_to_cart(
    db: &dyn Db,
    token: &str,
    product_id: i64,
    quantity: &Value,
) -> Result<Cart, WordlessError> {
    add_to_cart_variation(db, token, product_id, quantity, None).await
}

/// P8: add a quantity of a product — or of one of its variations — to
/// the cart. A variation id must belong to the product and be
/// published; its stock (not the parent's) is checked. A variable
/// product without a variation id is refused.
pub async fn add_to_cart_variation(
    db: &dyn Db,
    token: &str,
    product_id: i64,
    quantity: &Value,
    variation_id: Option<i64>,
) -> Result<Cart, WordlessError> {
    let qty = require_int(quantity, "quantity", 1, 99)?;
    let product = get_product(db, product_id)
        .await?
        .ok_or_else(|| not_found(&format!("Product {product_id} not found.")))?;
    if product.status != "published" {
        return Err(bad_request(&format!(
            "\"{}\" is not available for sale.",
            product.name
        )));
    }
    // P8: resolve the variation (or refuse a bare variable product).
    let variation_stock = match variation_id {
        Some(vid) => {
            let variation = crate::variation::get_variation(db, vid).await?;
            let Some(variation) = variation else {
                return Err(not_found(&format!("Variation {vid} not found.")));
            };
            if variation.product_id != product.id {
                return Err(bad_request(&format!(
                    "Variation {vid} does not belong to \"{}\".",
                    product.name
                )));
            }
            if variation.status != "published" {
                return Err(bad_request(&format!(
                    "That option of \"{}\" is not available for sale.",
                    product.name
                )));
            }
            Some(variation.stock)
        }
        None => {
            if product.product_type == "variable" {
                return Err(bad_request(&format!(
                    "\"{}\" has options — choose one before adding it to the cart.",
                    product.name
                )));
            }
            None
        }
    };
    let row = load_cart_row(db, token).await?;
    let mut items = row
        .as_ref()
        .map(|r| parse_items(&r.items))
        .unwrap_or_default();
    let existing_qty = items
        .iter()
        .find(|i| i.product_id == product_id && i.variation_id == variation_id)
        .map(|i| i.quantity)
        .unwrap_or(0);
    let next_qty = existing_qty + qty;
    match variation_stock {
        Some(stock) => check_stock(stock, &product.name, next_qty)?,
        None => check_stock(product.stock, &product.name, next_qty)?,
    }
    match items
        .iter_mut()
        .find(|i| i.product_id == product_id && i.variation_id == variation_id)
    {
        Some(item) => item.quantity = next_qty,
        None => items.push(CartItem {
            product_id,
            quantity: qty,
            variation_id,
        }),
    }
    save_cart(db, token, &items).await?;
    get_cart(db, token).await
}

/// Set a cart line's quantity (0 removes the line). Quantity must be
/// 0–99; stock is re-checked for the new quantity. A line is
/// identified by (product, variation) — two variations of one product
/// are separate lines.
pub async fn set_cart_quantity(
    db: &dyn Db,
    token: &str,
    product_id: i64,
    quantity: &Value,
) -> Result<Cart, WordlessError> {
    set_cart_quantity_variation(db, token, product_id, quantity, None).await
}

/// P8: set a cart line's quantity, for a (product, variation) line.
pub async fn set_cart_quantity_variation(
    db: &dyn Db,
    token: &str,
    product_id: i64,
    quantity: &Value,
    variation_id: Option<i64>,
) -> Result<Cart, WordlessError> {
    let qty = require_int(quantity, "quantity", 0, 99)?;
    let row = load_cart_row(db, token).await?;
    let mut items = row
        .as_ref()
        .map(|r| parse_items(&r.items))
        .unwrap_or_default();
    let Some(index) = items
        .iter()
        .position(|i| i.product_id == product_id && i.variation_id == variation_id)
    else {
        return Err(not_found(&format!(
            "Product {product_id} is not in the cart."
        )));
    };
    if qty == 0 {
        items.remove(index);
    } else {
        let product = get_product(db, product_id)
            .await?
            .ok_or_else(|| not_found(&format!("Product {product_id} not found.")))?;
        match variation_id {
            Some(vid) => {
                let variation = crate::variation::get_variation(db, vid).await?;
                let Some(variation) = variation else {
                    return Err(not_found(&format!("Variation {vid} not found.")));
                };
                check_stock(variation.stock, &product.name, qty)?;
            }
            None => check_stock(product.stock, &product.name, qty)?,
        }
        items[index].quantity = qty;
    }
    save_cart(db, token, &items).await?;
    get_cart(db, token).await
}

/// Delete the cart for a token (used when an order consumes the cart).
pub async fn clear_cart(db: &dyn Db, token: &str) -> Result<(), WordlessError> {
    db.prepare("DELETE FROM wl_carts WHERE id = ?")
        .bind(vec![Param::Text(token.to_string())])
        .run()
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::products::create_product;
    use crate::test_support::SqliteDb;
    use crate::types::ProductInput;
    use futures_executor::block_on;
    use serde_json::json;

    pub(crate) fn make_published_product(db: &SqliteDb, stock: i64) -> crate::types::Product {
        block_on(create_product(
            db,
            ProductInput {
                name: Some(json!("Pack Hoodie")),
                price_minor: Some(json!(125000)),
                currency: Some(json!("NGN")),
                stock: Some(json!(stock)),
                status: Some(json!("published")),
                ..Default::default()
            },
        ))
        .unwrap()
    }

    #[test]
    fn adds_items_enforces_stock_and_totals_in_minor_units() {
        let db = SqliteDb::new();
        let product = make_published_product(&db, 5);
        let token = new_cart_token().unwrap();
        let cart = block_on(add_to_cart(&db, &token, product.id, &json!(2))).unwrap();
        assert_eq!(cart.total_minor, 250000);
        assert_eq!(cart.lines[0].quantity, 2);
        let err = block_on(add_to_cart(&db, &token, product.id, &json!(4))).unwrap_err();
        assert!(err.message.contains("Only 5"), "{}", err.message);
        assert!(err.message.contains("in stock"), "{}", err.message);
        let reduced = block_on(set_cart_quantity(&db, &token, product.id, &json!(1))).unwrap();
        assert_eq!(reduced.total_minor, 125000);
        block_on(clear_cart(&db, &token)).unwrap();
        assert_eq!(block_on(get_cart(&db, &token)).unwrap().lines.len(), 0);
    }

    #[test]
    fn refuses_draft_products() {
        let db = SqliteDb::new();
        let product = block_on(create_product(
            &db,
            ProductInput {
                name: Some(json!("Draft Thing")),
                price_minor: Some(json!(1000)),
                stock: Some(json!(5)),
                status: Some(json!("draft")),
                ..Default::default()
            },
        ))
        .unwrap();
        let err = block_on(add_to_cart(
            &db,
            &new_cart_token().unwrap(),
            product.id,
            &json!(1),
        ))
        .unwrap_err();
        assert!(err.message.contains("not available"), "{}", err.message);
    }

    #[test]
    fn allows_unlimited_stock_without_a_cap() {
        let db = SqliteDb::new();
        let product = make_published_product(&db, -1);
        let cart = block_on(add_to_cart(
            &db,
            &new_cart_token().unwrap(),
            product.id,
            &json!(99),
        ))
        .unwrap();
        assert_eq!(cart.lines[0].quantity, 99);
    }

    #[test]
    fn cookie_parsing_matches_v1() {
        assert_eq!(
            parse_cart_cookie(Some(
                "wordless_cart=550e8400-e29b-41d4-a716-446655440000; Path=/"
            )),
            Some("550e8400-e29b-41d4-a716-446655440000".to_string())
        );
        assert_eq!(parse_cart_cookie(Some("other=1")), None);
        assert_eq!(parse_cart_cookie(Some("wordless_cart=short")), None);
        assert_eq!(parse_cart_cookie(None), None);
    }
}
