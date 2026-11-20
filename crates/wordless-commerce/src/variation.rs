//! Variation lookups (P8) — read-only SQL on `wl_woo_variations`.
//!
//! This lives in `wordless-commerce`, not `wordless-woocommerce`, so
//! the cart and order code can resolve variation prices and stock
//! without a crate cycle (the Woo crate depends on commerce, never
//! the reverse). The Woo crate owns variation CRUD; this module only
//! reads the rows the cart and the paid transition need. The table
//! always exists in production (migration `0014_p8.sql`); unit tests
//! carry it in `test_support`'s schema.

use serde_json::Value;
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::WordlessError;

/// One `wl_woo_variations` row, as the cart and orders need it.
#[derive(Debug, Clone, PartialEq)]
pub struct VariationRow {
    pub id: i64,
    pub product_id: i64,
    pub sku: String,
    pub status: String,
    pub price_minor: i64,
    pub sale_price_minor: i64,
    pub stock: i64,
    pub attributes_json: String,
    pub image_id: i64,
}

fn row_to_variation(row: &Row) -> VariationRow {
    VariationRow {
        id: row_int(row, "id"),
        product_id: row_int(row, "product_id"),
        sku: row_text(row, "sku"),
        status: row_text(row, "status"),
        price_minor: row_int(row, "price_minor"),
        sale_price_minor: row_int(row, "sale_price_minor"),
        stock: row_int(row, "stock"),
        attributes_json: row_text(row, "attributes_json"),
        image_id: row_int(row, "image_id"),
    }
}

/// Fetch a variation by id, or `None` when missing.
pub(crate) async fn get_variation(
    db: &dyn Db,
    id: i64,
) -> Result<Option<VariationRow>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_woo_variations WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(id)])
        .first()
        .await?;
    Ok(row.map(|r| row_to_variation(&r)))
}

/// The price actually charged for a variation: the sale price when
/// one is set (> 0), else the regular price.
pub(crate) fn effective_price(v: &VariationRow) -> i64 {
    if v.sale_price_minor > 0 {
        v.sale_price_minor
    } else {
        v.price_minor
    }
}

/// `{"Size": "M", "Colour": "Red"}` → `"Size: M, Colour: Red"`
/// (keys sorted, so the label is deterministic). Unparseable JSON →
/// `""`.
pub(crate) fn variation_label(attributes_json: &str) -> String {
    let parsed: Value = match serde_json::from_str(attributes_json) {
        Ok(v) => v,
        Err(_) => return String::new(),
    };
    let Value::Object(map) = parsed else {
        return String::new();
    };
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    keys.iter()
        .filter_map(|k| {
            map.get(*k)
                .and_then(|v| v.as_str())
                .map(|val| format!("{k}: {val}"))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_sorts_keys_and_skips_non_strings() {
        assert_eq!(
            variation_label(r#"{"Colour": "Red", "Size": "M"}"#),
            "Colour: Red, Size: M"
        );
        assert_eq!(variation_label("{}"), "");
        assert_eq!(variation_label("not json"), "");
        assert_eq!(variation_label(r#"{"Size": 3}"#), "");
    }

    #[test]
    fn sale_price_wins_when_set() {
        let v = VariationRow {
            id: 1,
            product_id: 1,
            sku: String::new(),
            status: "published".to_string(),
            price_minor: 10000,
            sale_price_minor: 8000,
            stock: 5,
            attributes_json: "{}".to_string(),
            image_id: 0,
        };
        assert_eq!(effective_price(&v), 8000);
        let v2 = VariationRow {
            sale_price_minor: 0,
            ..v
        };
        assert_eq!(effective_price(&v2), 10000);
    }
}
