//! WORDLESS v2 WooCommerce module (P8) — the store as a compatibility
//! subset, never WooCommerce PHP.
//!
//! Scope (doc §3 P8, decisions recorded in the outcome block before
//! any code was written):
//! - **Variable products:** `wl_products.type = 'variable'` plus
//!   `wl_woo_attributes` (the option lists) and `wl_woo_variations`
//!   (one row per purchasable combination, own price/stock). A
//!   variation's effective price is its sale price when set, else its
//!   regular price; the parent's `price_minor` is maintained as the
//!   minimum variation price.
//! - **Reuse-or-extend (decided at P8): EXTEND.** Products, cart,
//!   orders and webhooks are the P3 commerce tables and pipeline —
//!   the Woo crate adds only what P3 lacks. `wl_woo_products`,
//!   `wl_woo_orders` and `wl_woo_order_items` are deliberately NOT
//!   created (see the doc outcome block for the reasoning).
//! - **Coupons** (fixed / percent, usage limits, expiry, minimum
//!   order), **taxes** (configurable rates, most-specific match,
//!   off by default), **shipping** (zones + flat-rate / free-shipping
//!   methods), **customers** (upserted at checkout and import).
//! - **Import (Q12, decided on evidence):** CSV only. Products in
//!   WooCommerce's native product-export format (verified columns);
//!   orders in the de-facto WebToffee-style shape (`line_item_1…`);
//!   customers in WooCommerce's native customer-export columns. WXR
//!   `shop_order` import was deliberately NOT built — the format
//!   lacks itemized line items, so it would create orders with no
//!   items. Every imported row lands in the report as imported /
//!   updated / skipped / failed with a reason — nothing is silently
//!   dropped.
//! - **Money:** integer minor units only, everywhere (same rule as
//!   P3/LIFEOS). Tax and coupon percent are basis points; tax rounds
//!   half-up. No float ever touches a price.
//!
//! Modules: `types` (wire shapes, camelCase), `attributes` +
//! `variations` (variable-product CRUD + the storefront's
//! attribute-to-variation lookup), `coupons`, `taxes`, `shipping`,
//! `customers`, `totals` (the one quote/checkout computation),
//! `csv` (hand-rolled CSV parser — no new dependency — plus the
//! three importers), `checkout` (Woo checkout: order + provider +
//! gateway, reusing the P3 payment flow verbatim).
//!
//! Service functions take `db: &dyn Db` plus plain parameters and
//! return `Result<_, WordlessError>`; route handlers live in
//! `wordless-worker` (`routes_p8.rs`).

pub mod attributes;
pub mod checkout;
pub mod coupons;
pub mod csv;
pub mod customers;
pub mod products;
pub mod shipping;
pub mod taxes;
pub mod totals;
pub mod types;
pub mod variations;

#[cfg(test)]
mod test_support;

pub use types::{
    Attribute, AttributeInput, Coupon, CouponInput, Customer, ShippingMethod, ShippingMethodInput,
    ShippingZone, ShippingZoneInput, TaxRate, TaxRateInput, Totals, Variation, VariationInput,
    WooProduct,
};
