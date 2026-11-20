//! WooCommerce wire types (P8). All wire formats are camelCase, the
//! same as the P3 commerce types. Money is integer minor units only.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wordless_commerce::Product;

/// One `wl_woo_attributes` row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attribute {
    pub id: i64,
    pub product_id: i64,
    pub name: String,
    pub slug: String,
    pub position: i64,
    pub visible: bool,
    pub is_variation: bool,
    pub options: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Unvalidated attribute input.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AttributeInput {
    pub name: Option<Value>,
    pub position: Option<Value>,
    pub visible: Option<Value>,
    pub is_variation: Option<Value>,
    pub options: Option<Value>,
}

/// One `wl_woo_variations` row. `attributes` maps attribute name →
/// chosen value, e.g. `{"Size": "M", "Colour": "Red"}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Variation {
    pub id: i64,
    pub product_id: i64,
    pub sku: String,
    pub status: String,
    pub price_minor: i64,
    pub sale_price_minor: i64,
    /// The price actually charged (sale when set, else regular).
    pub effective_price_minor: i64,
    pub stock: i64,
    pub attributes: Value,
    /// `"Size: M, Colour: Red"` (keys sorted, deterministic).
    pub label: String,
    pub image_id: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Unvalidated variation input.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VariationInput {
    pub sku: Option<Value>,
    pub status: Option<Value>,
    pub price_minor: Option<Value>,
    pub sale_price_minor: Option<Value>,
    pub stock: Option<Value>,
    pub attributes: Option<Value>,
    pub image_id: Option<Value>,
}

/// A variable (or simple) product with its WooCommerce detail: its
/// attributes, variations, and imported categories/tags.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WooProduct {
    #[serde(flatten)]
    pub product: Product,
    pub attributes: Vec<Attribute>,
    pub variations: Vec<Variation>,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
}

/// One `wl_woo_coupons` row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Coupon {
    pub id: i64,
    /// Stored upper-case (codes are case-insensitive).
    pub code: String,
    pub description: String,
    /// `"fixed"` or `"percent"`.
    pub r#type: String,
    pub amount_minor: i64,
    /// Basis points for `"percent"` (1000 = 10%).
    pub percent_bp: i64,
    pub min_order_minor: i64,
    /// 0 = unlimited uses.
    pub max_uses: i64,
    pub usage_count: i64,
    pub expires_at: Option<String>,
    pub active: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Unvalidated coupon input.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CouponInput {
    pub code: Option<Value>,
    pub description: Option<Value>,
    #[serde(rename = "type")]
    pub r#type: Option<Value>,
    pub amount_minor: Option<Value>,
    pub percent_bp: Option<Value>,
    pub min_order_minor: Option<Value>,
    pub max_uses: Option<Value>,
    pub expires_at: Option<Value>,
    pub active: Option<Value>,
}

/// One `wl_woo_tax_rates` row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaxRate {
    pub id: i64,
    pub name: String,
    /// ISO country code, "" = any country.
    pub country: String,
    /// "" = any state in that country.
    pub state: String,
    /// Basis points (750 = 7.5%).
    pub rate_bp: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Unvalidated tax-rate input.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TaxRateInput {
    pub name: Option<Value>,
    pub country: Option<Value>,
    pub state: Option<Value>,
    pub rate_bp: Option<Value>,
}

/// One `wl_woo_shipping_zones` row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShippingZone {
    pub id: i64,
    pub name: String,
    /// ISO country codes; an empty list = rest of the world.
    pub countries: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Unvalidated shipping-zone input.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ShippingZoneInput {
    pub name: Option<Value>,
    pub countries: Option<Value>,
}

/// One `wl_woo_shipping_methods` row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShippingMethod {
    pub id: i64,
    pub zone_id: i64,
    pub name: String,
    /// `"flat_rate"` or `"free_shipping"`.
    pub r#type: String,
    pub cost_minor: i64,
    /// Free-shipping threshold (subtotal after discount).
    pub min_order_minor: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Unvalidated shipping-method input.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ShippingMethodInput {
    pub name: Option<Value>,
    #[serde(rename = "type")]
    pub r#type: Option<Value>,
    pub cost_minor: Option<Value>,
    pub min_order_minor: Option<Value>,
}

/// One `wl_woo_customers` row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Customer {
    pub id: i64,
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub username: String,
    pub billing_country: String,
    pub billing_state: String,
    pub billing_postcode: String,
    pub total_orders: i64,
    pub total_spent_minor: i64,
    pub source: String,
    pub source_id: String,
    pub created_at: String,
    pub updated_at: String,
}

/// The quote/checkout breakdown. `total_minor` is always
/// subtotal − discount + tax + shipping. All amounts are integer
/// minor units in `currency`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub subtotal_minor: i64,
    pub discount_minor: i64,
    pub coupon_code: Option<String>,
    pub coupon_note: Option<String>,
    pub tax_minor: i64,
    pub tax_rate_bp: i64,
    pub tax_rate_name: Option<String>,
    pub shipping_minor: i64,
    pub shipping_method: Option<String>,
    pub shipping_method_id: Option<i64>,
    pub total_minor: i64,
    pub currency: String,
}
