//! Orders (port of v1 `src/modules/commerce/orders.ts`).
//!
//! An order is created from a cart (pending), becomes paid through a
//! verified payment webhook or an admin transition, then fulfilled, or
//! cancelled. Stock is decremented exactly once when an order becomes
//! paid (`stock_applied` guards replays and double clicks) and restored
//! when a paid order is cancelled. Unlimited-stock products (stock -1)
//! never change. Order items snapshot the product name and price at
//! purchase time, so later product edits never rewrite history.

use serde_json::Value;
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, not_found, WordlessError};
use wordless_kernel::time::now_iso;
use wordless_kernel::validate::{require_email, require_string};

use crate::cart::{clear_cart, get_cart};
use crate::ids::new_uuid_v4;
use crate::products::get_product;
use crate::types::{
    Order, OrderItem, OrderList, ORDER_STATUS_CANCELLED, ORDER_STATUS_FULFILLED, ORDER_STATUS_PAID,
    ORDER_STATUS_PENDING,
};

fn row_to_order_item(row: &Row) -> OrderItem {
    // variation_id is NULL for simple-product items (row_int gives 0).
    let variation_id = match row_int(row, "variation_id") {
        v if v >= 1 => Some(v),
        _ => None,
    };
    let variation_label = match row_text(row, "variation_label") {
        s if s.is_empty() => None,
        s => Some(s),
    };
    OrderItem {
        id: row_int(row, "id"),
        order_id: row_text(row, "order_id"),
        product_id: row_int(row, "product_id"),
        name: row_text(row, "name"),
        price_minor: row_int(row, "price_minor"),
        quantity: row_int(row, "quantity"),
        variation_id,
        variation_label,
    }
}

fn row_to_order(row: &Row, items: Vec<OrderItem>) -> Order {
    // P8 columns: absent/empty in the DB reads as "not a Woo order".
    let coupon_code = match row_text(row, "coupon_code") {
        s if s.is_empty() => None,
        s => Some(s),
    };
    let shipping_method = match row_text(row, "shipping_method") {
        s if s.is_empty() => None,
        s => Some(s),
    };
    let source = match row_text(row, "source") {
        s if s.is_empty() || s == "wordless" => None,
        s => Some(s),
    };
    let source_id = match row_text(row, "source_id") {
        s if s.is_empty() => None,
        s => Some(s),
    };
    Order {
        id: row_text(row, "id"),
        customer_name: row_text(row, "customer_name"),
        customer_email: row_text(row, "customer_email"),
        total_minor: row_int(row, "total_minor"),
        currency: row_text(row, "currency"),
        status: row_text(row, "status"),
        payment_provider: row_text(row, "payment_provider"),
        payment_reference: row_text(row, "payment_reference"),
        stock_applied: row_int(row, "stock_applied") == 1,
        items,
        coupon_code,
        discount_minor: row_int(row, "discount_minor"),
        tax_minor: row_int(row, "tax_minor"),
        shipping_minor: row_int(row, "shipping_minor"),
        shipping_method,
        source,
        source_id,
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

/// P8 Woo totals carried into an order at creation time. `None`
/// means "plain P3 order" (no coupon, no tax, no shipping, source
/// `'wordless'`). The grand total is always
/// subtotal − discount + tax + shipping.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OrderTotals {
    pub coupon_code: Option<String>,
    pub discount_minor: i64,
    pub tax_minor: i64,
    pub shipping_minor: i64,
    pub shipping_method: Option<String>,
    /// `'woo'` for Woo checkouts; `'woo-import'` for imported orders
    /// (created by the Woo crate, not here).
    pub source: String,
}

async fn order_items(db: &dyn Db, order_id: &str) -> Result<Vec<OrderItem>, WordlessError> {
    let rows = db
        .prepare("SELECT * FROM wl_order_items WHERE order_id = ? ORDER BY id ASC")
        .bind(vec![Param::Text(order_id.to_string())])
        .all::<Row>()
        .await?;
    Ok(rows.iter().map(row_to_order_item).collect())
}

/// Fetch an order (with its items) by id, or `None` when missing.
pub async fn get_order(db: &dyn Db, id: &str) -> Result<Option<Order>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_orders WHERE id = ? LIMIT 1")
        .bind(vec![Param::Text(id.to_string())])
        .first()
        .await?;
    match row {
        Some(r) => {
            let items = order_items(db, id).await?;
            Ok(Some(row_to_order(&r, items)))
        }
        None => Ok(None),
    }
}

/// Find an order by its payment reference, or by its own id
/// (Paystack/Flutterwave use the order id as the reference). An empty
/// reference never matches: orders created by a checkout that failed
/// before a payment reference was assigned carry an empty
/// `payment_reference`, and a webhook carrying an empty reference must
/// not be able to pay one of them.
pub async fn get_order_by_reference(
    db: &dyn Db,
    reference: &str,
) -> Result<Option<Order>, WordlessError> {
    if reference.is_empty() {
        return Ok(None);
    }
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_orders WHERE payment_reference = ? OR id = ? LIMIT 1")
        .bind(vec![
            Param::Text(reference.to_string()),
            Param::Text(reference.to_string()),
        ])
        .first()
        .await?;
    match row {
        Some(r) => {
            let id = row_text(&r, "id");
            let items = order_items(db, &id).await?;
            Ok(Some(row_to_order(&r, items)))
        }
        None => Ok(None),
    }
}

/// Create a pending order from a cart. Stock and prices are re-checked
/// at checkout time (stock can change after add-to-cart, and a price
/// change refuses the order). The cart is consumed by the order — it
/// must not survive to be checked out a second time.
pub async fn create_order_from_cart(
    db: &dyn Db,
    cart_token: &str,
    customer_name: &Value,
    customer_email: &Value,
) -> Result<Order, WordlessError> {
    create_order_from_cart_with(db, cart_token, customer_name, customer_email, None).await
}

/// P8: create a pending order from a cart, carrying Woo totals
/// (coupon, discount, tax, shipping, source). The grand total stored
/// on the order is subtotal − discount + tax + shipping; the caller
/// (the Woo crate) computes the breakdown from the same cart first,
/// so a mismatch here is a wiring bug and is refused.
pub async fn create_order_from_cart_with(
    db: &dyn Db,
    cart_token: &str,
    customer_name: &Value,
    customer_email: &Value,
    totals: Option<OrderTotals>,
) -> Result<Order, WordlessError> {
    let customer_name = require_string(customer_name, "customerName", 1, 200)?;
    let customer_email = require_email(customer_email, "customerEmail")?;
    let cart = get_cart(db, cart_token).await?;
    if cart.lines.is_empty() {
        return Err(bad_request("The cart is empty."));
    }
    if let Some(note) = &cart.currency_note {
        return Err(bad_request(note));
    }
    for line in &cart.lines {
        let product = get_product(db, line.product_id).await?;
        let Some(product) = product else {
            return Err(bad_request(&format!(
                "\"{}\" is no longer available.",
                line.name
            )));
        };
        if product.status != "published" {
            return Err(bad_request(&format!(
                "\"{}\" is no longer available for sale.",
                line.name
            )));
        }
        // P8: variation lines check variation stock/price, not the
        // parent's (a variable product's own stock is meaningless).
        match line.variation_id {
            Some(vid) => {
                let variation = crate::variation::get_variation(db, vid).await?;
                let Some(variation) = variation else {
                    return Err(bad_request(&format!(
                        "That option of \"{}\" is no longer available.",
                        line.name
                    )));
                };
                if variation.status != "published" {
                    return Err(bad_request(&format!(
                        "That option of \"{}\" is no longer available for sale.",
                        line.name
                    )));
                }
                if variation.stock >= 0 && line.quantity > variation.stock {
                    return Err(bad_request(&format!(
                        "Only {} of \"{}\" ({}) is in stock.",
                        variation.stock,
                        line.name,
                        line.variation_label.clone().unwrap_or_default()
                    )));
                }
                if crate::variation::effective_price(&variation) != line.price_minor {
                    return Err(bad_request(&format!(
                        "The price of \"{}\" ({}) changed. Please review the cart and try again.",
                        line.name,
                        line.variation_label.clone().unwrap_or_default()
                    )));
                }
            }
            None => {
                if product.stock >= 0 && line.quantity > product.stock {
                    return Err(bad_request(&format!(
                        "Only {} of \"{}\" is in stock.",
                        product.stock, line.name
                    )));
                }
                if product.price_minor != line.price_minor || product.currency != line.currency {
                    return Err(bad_request(&format!(
                        "The price of \"{}\" changed. Please review the cart and try again.",
                        line.name
                    )));
                }
            }
        }
    }
    let totals = totals.unwrap_or_default();
    let grand_total =
        cart.total_minor - totals.discount_minor + totals.tax_minor + totals.shipping_minor;
    if grand_total < 0 {
        return Err(bad_request("The order total cannot be negative."));
    }
    let id = new_uuid_v4()?;
    let now = now_iso();
    db.prepare(
        "INSERT INTO wl_orders (id, customer_name, customer_email, total_minor, currency, status, payment_provider, payment_reference, stock_applied, coupon_code, discount_minor, tax_minor, shipping_minor, shipping_method, source, source_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, 'pending', '', '', 0, ?, ?, ?, ?, ?, ?, '', ?, ?)",
    )
    .bind(vec![
        Param::Text(id.clone()),
        Param::Text(customer_name),
        Param::Text(customer_email),
        Param::Integer(grand_total),
        Param::Text(cart.currency),
        Param::Text(totals.coupon_code.clone().unwrap_or_default()),
        Param::Integer(totals.discount_minor),
        Param::Integer(totals.tax_minor),
        Param::Integer(totals.shipping_minor),
        Param::Text(totals.shipping_method.clone().unwrap_or_default()),
        Param::Text(if totals.source.is_empty() { "wordless".to_string() } else { totals.source.clone() }),
        Param::Text(now.clone()),
        Param::Text(now.clone()),
    ])
    .run()
    .await?;
    for line in &cart.lines {
        db.prepare(
            "INSERT INTO wl_order_items (order_id, product_id, name, price_minor, quantity, variation_id, variation_label, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(vec![
            Param::Text(id.clone()),
            Param::Integer(line.product_id),
            Param::Text(line.name.clone()),
            Param::Integer(line.price_minor),
            Param::Integer(line.quantity),
            match line.variation_id {
                Some(v) => Param::Integer(v),
                None => Param::Null,
            },
            Param::Text(line.variation_label.clone().unwrap_or_default()),
            Param::Text(now.clone()),
        ])
        .run()
        .await?;
    }
    let order = get_order(db, &id)
        .await?
        .ok_or_else(|| WordlessError::new("internal_error", 500, "Order was not created."))?;
    clear_cart(db, cart_token).await?;
    Ok(order)
}

/// Record which provider + reference an order will be paid through.
/// Only a pending order can be sent to a payment provider.
pub async fn set_order_payment(
    db: &dyn Db,
    order_id: &str,
    provider: &str,
    reference: &str,
) -> Result<Order, WordlessError> {
    let order = get_order(db, order_id)
        .await?
        .ok_or_else(|| not_found(&format!("Order {order_id} not found.")))?;
    if order.status != ORDER_STATUS_PENDING {
        return Err(bad_request(&format!(
            "Order {order_id} is {} and can no longer be sent to a payment provider.",
            order.status
        )));
    }
    db.prepare("UPDATE wl_orders SET payment_provider = ?, payment_reference = ?, updated_at = ? WHERE id = ?")
        .bind(vec![
            Param::Text(provider.to_string()),
            Param::Text(reference.to_string()),
            Param::Text(now_iso()),
            Param::Text(order_id.to_string()),
        ])
        .run()
        .await?;
    get_order(db, order_id)
        .await?
        .ok_or_else(|| WordlessError::new("internal_error", 500, "Order was not updated."))
}

/// Is `from → to` an allowed order transition?
/// pending → paid | cancelled; paid → fulfilled | cancelled.
pub fn is_allowed_transition(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        (
            ORDER_STATUS_PENDING,
            ORDER_STATUS_PAID | ORDER_STATUS_CANCELLED
        ) | (
            ORDER_STATUS_PAID,
            ORDER_STATUS_FULFILLED | ORDER_STATUS_CANCELLED
        )
    )
}

async fn apply_stock(db: &dyn Db, order: &Order) -> Result<(), WordlessError> {
    if order.stock_applied {
        return Ok(());
    }
    for item in &order.items {
        // P8: a variation item decrements the VARIATION row, never the
        // parent product (a variable product's own stock is meaningless).
        if let Some(vid) = item.variation_id {
            let variation = crate::variation::get_variation(db, vid).await?;
            let Some(variation) = variation else { continue };
            if variation.stock < 0 {
                continue;
            }
            if variation.stock < item.quantity {
                return Err(bad_request(&format!(
                    "Not enough stock for \"{}\" ({}) ({} left, order needs {}). Do not mark this order paid.",
                    item.name,
                    item.variation_label.clone().unwrap_or_default(),
                    variation.stock,
                    item.quantity
                )));
            }
            db.prepare(
                "UPDATE wl_woo_variations SET stock = stock - ?, updated_at = ? WHERE id = ?",
            )
            .bind(vec![
                Param::Integer(item.quantity),
                Param::Text(now_iso()),
                Param::Integer(vid),
            ])
            .run()
            .await?;
            continue;
        }
        let product = get_product(db, item.product_id).await?;
        let Some(product) = product else { continue };
        if product.stock < 0 {
            continue;
        }
        if product.stock < item.quantity {
            return Err(bad_request(&format!(
                "Not enough stock for \"{}\" ({} left, order needs {}). Do not mark this order paid.",
                item.name, product.stock, item.quantity
            )));
        }
        db.prepare("UPDATE wl_products SET stock = stock - ?, updated_at = ? WHERE id = ?")
            .bind(vec![
                Param::Integer(item.quantity),
                Param::Text(now_iso()),
                Param::Integer(item.product_id),
            ])
            .run()
            .await?;
    }
    db.prepare("UPDATE wl_orders SET stock_applied = 1, updated_at = ? WHERE id = ?")
        .bind(vec![Param::Text(now_iso()), Param::Text(order.id.clone())])
        .run()
        .await?;
    Ok(())
}

async fn restore_stock(db: &dyn Db, order: &Order) -> Result<(), WordlessError> {
    if !order.stock_applied {
        return Ok(());
    }
    for item in &order.items {
        if let Some(vid) = item.variation_id {
            let variation = crate::variation::get_variation(db, vid).await?;
            let Some(variation) = variation else { continue };
            if variation.stock < 0 {
                continue;
            }
            db.prepare(
                "UPDATE wl_woo_variations SET stock = stock + ?, updated_at = ? WHERE id = ?",
            )
            .bind(vec![
                Param::Integer(item.quantity),
                Param::Text(now_iso()),
                Param::Integer(vid),
            ])
            .run()
            .await?;
            continue;
        }
        let product = get_product(db, item.product_id).await?;
        let Some(product) = product else { continue };
        if product.stock < 0 {
            continue;
        }
        db.prepare("UPDATE wl_products SET stock = stock + ?, updated_at = ? WHERE id = ?")
            .bind(vec![
                Param::Integer(item.quantity),
                Param::Text(now_iso()),
                Param::Integer(item.product_id),
            ])
            .run()
            .await?;
    }
    db.prepare("UPDATE wl_orders SET stock_applied = 0, updated_at = ? WHERE id = ?")
        .bind(vec![Param::Text(now_iso()), Param::Text(order.id.clone())])
        .run()
        .await?;
    Ok(())
}

/// P8: increment a coupon's usage count exactly once, atomically with
/// the paid transition (this runs next to the stock decrement, in the
/// same call path as every webhook and admin transition — there is no
/// second call site to forget). The `usage_count < max_uses` guard
/// keeps a coupon from ever exceeding its limit even if two orders
/// are paid concurrently; a failed checkout never burns a use because
/// this only runs on the pending → paid transition.
async fn apply_coupon_use(db: &dyn Db, order: &Order) -> Result<(), WordlessError> {
    let Some(code) = &order.coupon_code else {
        return Ok(());
    };
    db.prepare(
        "UPDATE wl_woo_coupons SET usage_count = usage_count + 1, updated_at = ? WHERE code = ? AND (max_uses = 0 OR usage_count < max_uses)",
    )
    .bind(vec![Param::Text(now_iso()), Param::Text(code.clone())])
    .run()
    .await?;
    Ok(())
}

/// Move an order to a new status. Stock decrements exactly once on the
/// pending → paid transition and is restored when a paid order is
/// cancelled. Anything else is refused with a 400.
pub async fn transition_order(
    db: &dyn Db,
    order_id: &str,
    to: &str,
) -> Result<Order, WordlessError> {
    let order = get_order(db, order_id)
        .await?
        .ok_or_else(|| not_found(&format!("Order {order_id} not found.")))?;
    if !is_allowed_transition(&order.status, to) {
        return Err(bad_request(&format!(
            "Order cannot move from {} to {to}.",
            order.status
        )));
    }
    if to == ORDER_STATUS_PAID {
        apply_stock(db, &order).await?;
        apply_coupon_use(db, &order).await?;
    }
    if to == ORDER_STATUS_CANCELLED && order.status == ORDER_STATUS_PAID {
        restore_stock(db, &order).await?;
    }
    db.prepare("UPDATE wl_orders SET status = ?, updated_at = ? WHERE id = ?")
        .bind(vec![
            Param::Text(to.to_string()),
            Param::Text(now_iso()),
            Param::Text(order_id.to_string()),
        ])
        .run()
        .await?;
    get_order(db, order_id)
        .await?
        .ok_or_else(|| WordlessError::new("internal_error", 500, "Order was not updated."))
}

/// List orders, newest first, with their items. `status` is one of the
/// four order statuses or `None` (all). Page ≥ 1, per-page clamped
/// 1–100, default 20.
pub async fn list_orders(
    db: &dyn Db,
    status: Option<&str>,
    page: i64,
    per_page: i64,
) -> Result<OrderList, WordlessError> {
    let page = page.max(1);
    let per_page = per_page.clamp(1, 100);
    let offset = (page - 1) * per_page;
    let (where_sql, mut params): (&str, Vec<Param>) = match status {
        Some(s)
            if s == ORDER_STATUS_PENDING
                || s == ORDER_STATUS_PAID
                || s == ORDER_STATUS_FULFILLED
                || s == ORDER_STATUS_CANCELLED =>
        {
            ("WHERE status = ?", vec![Param::Text(s.to_string())])
        }
        Some(other) => {
            return Err(bad_request(&format!(
                "Field \"status\" must be \"pending\", \"paid\", \"fulfilled\" or \"cancelled\", got \"{other}\"."
            )))
        }
        None => ("", vec![]),
    };
    let total_row: Option<Row> = db
        .prepare(&format!("SELECT COUNT(*) AS n FROM wl_orders {where_sql}"))
        .bind(params.clone())
        .first()
        .await?;
    let total = total_row.as_ref().map(|r| row_int(r, "n")).unwrap_or(0);
    params.push(Param::Integer(per_page));
    params.push(Param::Integer(offset));
    let rows = db
        .prepare(&format!(
            "SELECT * FROM wl_orders {where_sql} ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?"
        ))
        .bind(params)
        .all::<Row>()
        .await?;
    let mut orders = Vec::with_capacity(rows.len());
    for row in &rows {
        let id = row_text(row, "id");
        let items = order_items(db, &id).await?;
        orders.push(row_to_order(row, items));
    }
    Ok(OrderList {
        orders,
        total,
        page,
        per_page,
        total_pages: ((total + per_page - 1) / per_page).max(1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cart::{add_to_cart, get_cart, new_cart_token};
    use crate::products::create_product;
    use crate::test_support::SqliteDb;
    use crate::types::ProductInput;
    use futures_executor::block_on;
    use serde_json::json;

    pub(crate) fn make_product(db: &SqliteDb, stock: i64) -> crate::types::Product {
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

    pub(crate) fn make_pending_order(db: &SqliteDb, qty: i64) -> Order {
        let product = make_product(db, 5);
        let token = new_cart_token().unwrap();
        block_on(add_to_cart(db, &token, product.id, &json!(qty))).unwrap();
        block_on(create_order_from_cart(
            db,
            &token,
            &json!("Ada"),
            &json!("ada@example.test"),
        ))
        .unwrap()
    }

    #[test]
    fn creates_pending_order_from_cart_and_snapshots_prices() {
        let db = SqliteDb::new();
        let product = make_product(&db, 5);
        let token = new_cart_token().unwrap();
        block_on(add_to_cart(&db, &token, product.id, &json!(2))).unwrap();
        let order = block_on(create_order_from_cart(
            &db,
            &token,
            &json!("Ada"),
            &json!("ada@example.test"),
        ))
        .unwrap();
        assert_eq!(order.status, "pending");
        assert_eq!(order.total_minor, 250000);
        assert_eq!(order.items.len(), 1);
        assert_eq!(order.items[0].product_id, product.id);
        assert_eq!(order.items[0].name, "Pack Hoodie");
        assert_eq!(order.items[0].price_minor, 125000);
        assert_eq!(order.items[0].quantity, 2);
        // The cart is consumed by the order.
        assert_eq!(block_on(get_cart(&db, &token)).unwrap().lines.len(), 0);
    }

    #[test]
    fn refuses_empty_cart_and_bad_customer_details() {
        let db = SqliteDb::new();
        let err = block_on(create_order_from_cart(
            &db,
            &new_cart_token().unwrap(),
            &json!("Ada"),
            &json!("ada@example.test"),
        ))
        .unwrap_err();
        assert!(err.message.contains("cart is empty"), "{}", err.message);
        let product = make_product(&db, 5);
        let token = new_cart_token().unwrap();
        block_on(add_to_cart(&db, &token, product.id, &json!(1))).unwrap();
        let err = block_on(create_order_from_cart(
            &db,
            &token,
            &json!("Ada"),
            &json!("not-an-email"),
        ))
        .unwrap_err();
        assert!(err.message.contains("valid email"), "{}", err.message);
    }

    #[test]
    fn decrements_stock_exactly_once_when_paid_and_restores_on_cancel() {
        let db = SqliteDb::new();
        let order = make_pending_order(&db, 2);
        let product_id = order.items[0].product_id;
        let paid = block_on(transition_order(&db, &order.id, "paid")).unwrap();
        assert_eq!(paid.status, "paid");
        assert!(paid.stock_applied);
        assert_eq!(
            block_on(get_product(&db, product_id))
                .unwrap()
                .unwrap()
                .stock,
            3
        ); // 5 - 2
           // A second "paid" transition is refused — stock can never apply twice.
        let err = block_on(transition_order(&db, &order.id, "paid")).unwrap_err();
        assert!(err.message.contains("cannot move"), "{}", err.message);
        assert_eq!(
            block_on(get_product(&db, product_id))
                .unwrap()
                .unwrap()
                .stock,
            3
        );
        let cancelled = block_on(transition_order(&db, &order.id, "cancelled")).unwrap();
        assert_eq!(cancelled.status, "cancelled");
        assert_eq!(
            block_on(get_product(&db, product_id))
                .unwrap()
                .unwrap()
                .stock,
            5
        );
    }

    #[test]
    fn refuses_illegal_transitions() {
        let db = SqliteDb::new();
        let order = make_pending_order(&db, 1);
        let err = block_on(transition_order(&db, &order.id, "fulfilled")).unwrap_err();
        assert_eq!(err.message, "Order cannot move from pending to fulfilled.");
    }

    #[test]
    fn lists_orders_with_items_and_finds_by_reference() {
        let db = SqliteDb::new();
        let order = make_pending_order(&db, 1);
        let list = block_on(list_orders(&db, None, 1, 20)).unwrap();
        assert_eq!(list.total, 1);
        assert_eq!(list.orders[0].items.len(), 1);
        assert!(block_on(get_order_by_reference(&db, &order.id))
            .unwrap()
            .is_some());
        assert!(block_on(get_order_by_reference(&db, "")).unwrap().is_none());
    }
}
