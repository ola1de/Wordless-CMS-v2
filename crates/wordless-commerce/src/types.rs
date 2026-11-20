//! Commerce types (port of v1 `src/modules/commerce/types.ts`).
//!
//! Stock: `-1` means unlimited (digital goods / services). Any other
//! value is an integer count; an order can only take stock that exists.
//! Order status flow: pending → paid → fulfilled, or pending/paid →
//! cancelled. `stock_applied` records whether this order has already
//! decremented stock, so a replayed webhook or a double admin click can
//! never apply stock twice.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Product status: `"draft"` or `"published"`.
pub const PRODUCT_STATUS_DRAFT: &str = "draft";
/// Product status: `"draft"` or `"published"`.
pub const PRODUCT_STATUS_PUBLISHED: &str = "published";
/// Order status: `"pending"`.
pub const ORDER_STATUS_PENDING: &str = "pending";
/// Order status: `"paid"`.
pub const ORDER_STATUS_PAID: &str = "paid";
/// Order status: `"fulfilled"`.
pub const ORDER_STATUS_FULFILLED: &str = "fulfilled";
/// Order status: `"cancelled"`.
pub const ORDER_STATUS_CANCELLED: &str = "cancelled";

/// A product row from `wl_products` (wire format is camelCase).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Product {
    pub id: i64,
    pub name: String,
    pub slug: String,
    pub description: String,
    pub price_minor: i64,
    pub currency: String,
    /// `-1` = unlimited stock.
    pub stock: i64,
    pub status: String,
    pub image_id: i64,
    /// P8: `"simple"` or `"variable"`. Omitted from the wire when
    /// `"simple"` so v1 parity on existing routes is untouched.
    #[serde(
        rename = "type",
        default,
        skip_serializing_if = "is_simple_product_type"
    )]
    pub product_type: String,
    /// P8: the WooCommerce stable key ("" when unset). Omitted when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sku: String,
    pub created_at: String,
    pub updated_at: String,
}

/// True when the product type is the default `"simple"` (or empty,
/// for rows written before P8).
pub fn is_simple_product_type(s: &str) -> bool {
    s.is_empty() || s == "simple"
}

/// True when an integer is 0 (skip-serialise helper for Woo totals).
pub fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// Unvalidated product input, mirroring v1's `ProductInput` (`unknown`
/// fields). Every field is `None` when absent; validation happens in
/// `products` with v1's exact error messages.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProductInput {
    pub name: Option<Value>,
    pub slug: Option<Value>,
    pub description: Option<Value>,
    pub price_minor: Option<Value>,
    pub currency: Option<Value>,
    pub stock: Option<Value>,
    pub status: Option<Value>,
    pub image_id: Option<Value>,
    /// P8: `"simple"` or `"variable"` (JSON key `"type"`).
    #[serde(rename = "type")]
    pub product_type: Option<Value>,
    /// P8: SKU (WooCommerce stable key).
    pub sku: Option<Value>,
}

/// A paginated product list (wire format is camelCase).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductList {
    pub products: Vec<Product>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

/// One line of an order — a snapshot of the product name and price at
/// purchase time, so later product edits never rewrite history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderItem {
    pub id: i64,
    pub order_id: String,
    pub product_id: i64,
    pub name: String,
    pub price_minor: i64,
    pub quantity: i64,
    /// P8: the variation bought (None for a simple product). Omitted
    /// when absent so v1 parity on existing routes is untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variation_id: Option<i64>,
    /// P8: `"Size: M, Colour: Red"` snapshot. Omitted when empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variation_label: Option<String>,
}

/// An order row from `wl_orders` plus its items (wire format camelCase).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Order {
    pub id: String,
    pub customer_name: String,
    pub customer_email: String,
    pub total_minor: i64,
    pub currency: String,
    pub status: String,
    pub payment_provider: String,
    pub payment_reference: String,
    pub stock_applied: bool,
    pub items: Vec<OrderItem>,
    /// P8 Woo totals — all omitted from the wire when unset, so v1
    /// parity on existing routes is untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coupon_code: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub discount_minor: i64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tax_minor: i64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub shipping_minor: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shipping_method: Option<String>,
    /// P8 provenance: None for plain P3 orders (`'wordless'` in the
    /// DB); Some("woo") for Woo checkouts, Some("woo-import") for
    /// imported orders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A paginated order list (wire format is camelCase).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderList {
    pub orders: Vec<Order>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

/// One stored cart item: `{ productId, quantity, variationId? }`
/// (camelCase JSON in `wl_carts.items`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CartItem {
    pub product_id: i64,
    pub quantity: i64,
    /// P8: the variation in the cart (None for a simple product).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variation_id: Option<i64>,
}

/// One cart line, enriched with product data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CartLine {
    pub product_id: i64,
    pub name: String,
    pub slug: String,
    pub price_minor: i64,
    pub currency: String,
    pub quantity: i64,
    pub line_total_minor: i64,
    /// P8: the variation on this line (None for a simple product).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variation_id: Option<i64>,
    /// P8: `"Size: M, Colour: Red"` for a variation line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variation_label: Option<String>,
}

/// The cart for a token, enriched with product data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cart {
    pub token: String,
    pub lines: Vec<CartLine>,
    pub total_minor: i64,
    pub currency: String,
    /// `None` normally; set when the cart mixes currencies (only one
    /// currency is kept — the first line's).
    pub currency_note: Option<String>,
}
