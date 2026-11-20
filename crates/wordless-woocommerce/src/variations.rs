//! Variations (P8) — the purchasable combinations of a variable
//! product (`wl_woo_variations`).
//!
//! A variation carries one value per variation attribute of its
//! parent (`attributes_json`, e.g. `{"Size": "M", "Colour": "Red"}`),
//! its own price (regular + optional sale price) and stock. The
//! parent's `price_minor` is maintained as the minimum effective
//! variation price (`sync_parent_price`), so the shop list shows the
//! "from" price. A variation's SKU must be unique across products
//! AND variations (the WooCommerce rule); an empty SKU means "no SKU".
//!
//! Stock and price rules live here for writes; reads at cart and
//! order time go through `wordless-commerce`'s variation lookup (the
//! same rows, the same effective-price rule — one rule, two call
//! sites, both tested).

use serde_json::Value;
use wordless_commerce::money::require_minor_units;
use wordless_commerce::products::get_product;
use wordless_commerce::validate::require_int;
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, not_found, WordlessError};
use wordless_kernel::time::now_iso;

use crate::attributes::list_attributes;
use crate::types::{Variation, VariationInput};

pub(crate) fn row_to_variation(row: &Row) -> Variation {
    let price_minor = row_int(row, "price_minor");
    let sale_price_minor = row_int(row, "sale_price_minor");
    let attributes_json = row_text(row, "attributes_json");
    let attributes: Value =
        serde_json::from_str(&attributes_json).unwrap_or(Value::Object(Default::default()));
    let label = variation_label(&attributes);
    Variation {
        id: row_int(row, "id"),
        product_id: row_int(row, "product_id"),
        sku: row_text(row, "sku"),
        status: row_text(row, "status"),
        price_minor,
        sale_price_minor,
        effective_price_minor: if sale_price_minor > 0 {
            sale_price_minor
        } else {
            price_minor
        },
        stock: row_int(row, "stock"),
        attributes,
        label,
        image_id: row_int(row, "image_id"),
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

/// `{"Size": "M", "Colour": "Red"}` → `"Colour: Red, Size: M"`
/// (keys sorted, deterministic). Same rule as the commerce lookup.
pub fn variation_label(attributes: &Value) -> String {
    let Value::Object(map) = attributes else {
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

/// The parent's display price = the minimum effective variation
/// price (0 when it has no published variations).
pub(crate) async fn sync_parent_price(db: &dyn Db, product_id: i64) -> Result<(), WordlessError> {
    let rows = db
        .prepare(
            "SELECT price_minor, sale_price_minor FROM wl_woo_variations WHERE product_id = ? AND status = 'published'",
        )
        .bind(vec![Param::Integer(product_id)])
        .all::<Row>()
        .await?;
    let min = rows
        .iter()
        .map(|r| {
            let sale = row_int(r, "sale_price_minor");
            if sale > 0 {
                sale
            } else {
                row_int(r, "price_minor")
            }
        })
        .min()
        .unwrap_or(0);
    db.prepare("UPDATE wl_products SET price_minor = ?, updated_at = ? WHERE id = ?")
        .bind(vec![
            Param::Integer(min),
            Param::Text(now_iso()),
            Param::Integer(product_id),
        ])
        .run()
        .await?;
    Ok(())
}

fn require_variation_attributes(value: &Value) -> Result<Value, WordlessError> {
    match value {
        Value::Null => Ok(Value::Object(Default::default())),
        Value::Object(map) => {
            if map.len() > 20 {
                return Err(bad_request("A variation can have at most 20 attributes."));
            }
            for (k, v) in map {
                if k.is_empty() || k.len() > 100 {
                    return Err(bad_request(
                        "Attribute names must be between 1 and 100 characters.",
                    ));
                }
                let Some(s) = v.as_str() else {
                    return Err(bad_request("Attribute values must be strings."));
                };
                if s.is_empty() || s.len() > 100 {
                    return Err(bad_request(
                        "Attribute values must be between 1 and 100 characters.",
                    ));
                }
            }
            Ok(value.clone())
        }
        _ => Err(bad_request(
            "Field \"attributes\" must be an object of attribute name to value.",
        )),
    }
}

async fn sku_taken(
    db: &dyn Db,
    sku: &str,
    exclude_variation: Option<i64>,
) -> Result<bool, WordlessError> {
    if sku.is_empty() {
        return Ok(false);
    }
    let row: Option<Row> = db
        .prepare("SELECT id FROM wl_products WHERE sku = ? LIMIT 1")
        .bind(vec![Param::Text(sku.to_string())])
        .first()
        .await?;
    if row.is_some() {
        return Ok(true);
    }
    let row: Option<Row> = db
        .prepare("SELECT id FROM wl_woo_variations WHERE sku = ? LIMIT 1")
        .bind(vec![Param::Text(sku.to_string())])
        .first()
        .await?;
    Ok(match row {
        None => false,
        Some(r) => exclude_variation != Some(row_int(&r, "id")),
    })
}

/// Validate a variation's attribute values against the parent's
/// variation attributes: every variation attribute must be present
/// with a value from its option list; no unknown attribute names.
async fn validate_against_parent(
    db: &dyn Db,
    product_id: i64,
    attributes: &Value,
) -> Result<(), WordlessError> {
    let attrs = list_attributes(db, product_id).await?;
    let Value::Object(map) = attributes else {
        return Err(bad_request("Field \"attributes\" must be an object."));
    };
    for attr in attrs.iter().filter(|a| a.is_variation) {
        let Some(value) = map.get(&attr.name).and_then(|v| v.as_str()) else {
            return Err(bad_request(&format!(
                "Variation is missing a value for attribute \"{}\".",
                attr.name
            )));
        };
        if !attr.options.iter().any(|o| o == value) {
            return Err(bad_request(&format!(
                "\"{value}\" is not one of the options for attribute \"{}\" ({}).",
                attr.name,
                attr.options.join(", ")
            )));
        }
    }
    for key in map.keys() {
        if !attrs.iter().any(|a| &a.name == key) {
            return Err(bad_request(&format!(
                "\"{key}\" is not an attribute of this product."
            )));
        }
    }
    Ok(())
}

fn require_variation_status(value: &Value) -> Result<String, WordlessError> {
    match value {
        Value::Null => Ok("published".to_string()),
        Value::String(s) if s == "draft" || s == "published" => Ok(s.clone()),
        _ => Err(bad_request(
            "Field \"status\" must be \"draft\" or \"published\".",
        )),
    }
}

/// All variations of a product (newest first).
pub async fn list_variations(
    db: &dyn Db,
    product_id: i64,
) -> Result<Vec<Variation>, WordlessError> {
    let rows = db
        .prepare("SELECT * FROM wl_woo_variations WHERE product_id = ? ORDER BY id DESC")
        .bind(vec![Param::Integer(product_id)])
        .all::<Row>()
        .await?;
    Ok(rows.iter().map(row_to_variation).collect())
}

/// Fetch one variation of a product, or `None` when missing.
pub async fn get_variation_for_product(
    db: &dyn Db,
    product_id: i64,
    variation_id: i64,
) -> Result<Option<Variation>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_woo_variations WHERE id = ? AND product_id = ? LIMIT 1")
        .bind(vec![
            Param::Integer(variation_id),
            Param::Integer(product_id),
        ])
        .first()
        .await?;
    Ok(row.map(|r| row_to_variation(&r)))
}

/// Create a variation on a variable product.
pub async fn create_variation(
    db: &dyn Db,
    product_id: i64,
    input: VariationInput,
) -> Result<Variation, WordlessError> {
    let product = get_product(db, product_id)
        .await?
        .ok_or_else(|| not_found(&format!("Product {product_id} not found.")))?;
    if product.product_type != "variable" {
        return Err(bad_request(&format!(
            "Product {product_id} is not a variable product — variations belong to variable products only."
        )));
    }
    let attributes =
        require_variation_attributes(input.attributes.as_ref().unwrap_or(&Value::Null))?;
    validate_against_parent(db, product_id, &attributes).await?;
    let sku = match input.sku.as_ref() {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Null) | None => String::new(),
        _ => return Err(bad_request("Field \"sku\" must be a string.")),
    };
    if sku.len() > 100 {
        return Err(bad_request("Field \"sku\" must be at most 100 characters."));
    }
    if sku_taken(db, &sku, None).await? {
        return Err(bad_request(&format!("SKU \"{sku}\" is already used.")));
    }
    let status = require_variation_status(input.status.as_ref().unwrap_or(&Value::Null))?;
    let price_minor = match input.price_minor.as_ref() {
        Some(v) => require_minor_units(v, "priceMinor", 0)?,
        None => 0,
    };
    let sale_price_minor = match input.sale_price_minor.as_ref() {
        Some(v) => require_minor_units(v, "salePriceMinor", 0)?,
        None => 0,
    };
    let stock = match input.stock.as_ref() {
        Some(v) => require_int(v, "stock", -1, i64::MAX)?,
        None => -1,
    };
    let image_id = match input.image_id.as_ref() {
        Some(v) => require_int(v, "imageId", 0, i64::MAX)?,
        None => 0,
    };
    let now = now_iso();
    let attributes_json = serde_json::to_string(&attributes)
        .map_err(|e| WordlessError::new("internal_error", 500, &e.to_string()))?;
    let meta = db
        .prepare(
            "INSERT INTO wl_woo_variations (product_id, sku, status, price_minor, sale_price_minor, stock, attributes_json, image_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(vec![
            Param::Integer(product_id),
            Param::Text(sku),
            Param::Text(status),
            Param::Integer(price_minor),
            Param::Integer(sale_price_minor),
            Param::Integer(stock),
            Param::Text(attributes_json),
            Param::Integer(image_id),
            Param::Text(now.clone()),
            Param::Text(now),
        ])
        .run()
        .await?;
    sync_parent_price(db, product_id).await?;
    let created = get_variation_for_product(db, product_id, meta.last_row_id.unwrap_or(0)).await?;
    created.ok_or_else(|| WordlessError::new("internal_error", 500, "Variation was not created."))
}

/// Update a variation. Only the fields present change; the parent's
/// price is re-synced afterwards.
pub async fn update_variation(
    db: &dyn Db,
    product_id: i64,
    variation_id: i64,
    input: VariationInput,
) -> Result<Variation, WordlessError> {
    let existing = get_variation_for_product(db, product_id, variation_id)
        .await?
        .ok_or_else(|| not_found(&format!("Variation {variation_id} not found.")))?;
    let attributes = match input.attributes.as_ref() {
        Some(v) => require_variation_attributes(v)?,
        None => existing.attributes.clone(),
    };
    validate_against_parent(db, product_id, &attributes).await?;
    let sku = match input.sku.as_ref() {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Null) => String::new(),
        Some(_) => return Err(bad_request("Field \"sku\" must be a string.")),
        None => existing.sku.clone(),
    };
    if sku.len() > 100 {
        return Err(bad_request("Field \"sku\" must be at most 100 characters."));
    }
    if sku_taken(db, &sku, Some(variation_id)).await? {
        return Err(bad_request(&format!("SKU \"{sku}\" is already used.")));
    }
    let status = match input.status.as_ref() {
        Some(v) => require_variation_status(v)?,
        None => existing.status.clone(),
    };
    let price_minor = match input.price_minor.as_ref() {
        Some(v) => require_minor_units(v, "priceMinor", 0)?,
        None => existing.price_minor,
    };
    let sale_price_minor = match input.sale_price_minor.as_ref() {
        Some(v) => require_minor_units(v, "salePriceMinor", 0)?,
        None => existing.sale_price_minor,
    };
    let stock = match input.stock.as_ref() {
        Some(v) => require_int(v, "stock", -1, i64::MAX)?,
        None => existing.stock,
    };
    let image_id = match input.image_id.as_ref() {
        Some(v) => require_int(v, "imageId", 0, i64::MAX)?,
        None => existing.image_id,
    };
    let attributes_json = serde_json::to_string(&attributes)
        .map_err(|e| WordlessError::new("internal_error", 500, &e.to_string()))?;
    db.prepare(
        "UPDATE wl_woo_variations SET sku = ?, status = ?, price_minor = ?, sale_price_minor = ?, stock = ?, attributes_json = ?, image_id = ?, updated_at = ? WHERE id = ? AND product_id = ?",
    )
    .bind(vec![
        Param::Text(sku),
        Param::Text(status),
        Param::Integer(price_minor),
        Param::Integer(sale_price_minor),
        Param::Integer(stock),
        Param::Text(attributes_json),
        Param::Integer(image_id),
        Param::Text(now_iso()),
        Param::Integer(variation_id),
        Param::Integer(product_id),
    ])
    .run()
    .await?;
    sync_parent_price(db, product_id).await?;
    let updated = get_variation_for_product(db, product_id, variation_id).await?;
    updated.ok_or_else(|| WordlessError::new("internal_error", 500, "Variation was not updated."))
}

/// Delete a variation. Refused when the variation appears in any
/// order — order items keep their snapshot, but the variation row is
/// the audit trail for what was sold.
pub async fn delete_variation(
    db: &dyn Db,
    product_id: i64,
    variation_id: i64,
) -> Result<bool, WordlessError> {
    if get_variation_for_product(db, product_id, variation_id)
        .await?
        .is_none()
    {
        return Err(not_found(&format!("Variation {variation_id} not found.")));
    }
    let row: Option<Row> = db
        .prepare("SELECT COUNT(*) AS n FROM wl_order_items WHERE variation_id = ?")
        .bind(vec![Param::Integer(variation_id)])
        .first()
        .await?;
    if row.as_ref().map(|r| row_int(r, "n")).unwrap_or(0) > 0 {
        return Err(bad_request(&format!(
            "Variation {variation_id} appears in an order and cannot be deleted. Unpublish it instead."
        )));
    }
    let meta = db
        .prepare("DELETE FROM wl_woo_variations WHERE id = ? AND product_id = ?")
        .bind(vec![
            Param::Integer(variation_id),
            Param::Integer(product_id),
        ])
        .run()
        .await?;
    sync_parent_price(db, product_id).await?;
    Ok(meta.changes > 0)
}

/// Find the published variation whose attributes match `selections`
/// exactly (the storefront form posts attribute values, not ids).
/// `selections` is a list of (attribute name, value).
pub async fn find_variation_by_attributes(
    db: &dyn Db,
    product_id: i64,
    selections: &[(String, String)],
) -> Result<Option<Variation>, WordlessError> {
    let variations = list_variations(db, product_id).await?;
    for variation in variations {
        if variation.status != "published" {
            continue;
        }
        let Value::Object(map) = &variation.attributes else {
            continue;
        };
        if map.len() != selections.len() {
            continue;
        }
        let matches = selections
            .iter()
            .all(|(name, value)| map.get(name).and_then(|v| v.as_str()) == Some(value.as_str()));
        if matches {
            return Ok(Some(variation));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attributes::create_attribute;
    use crate::test_support::TestDb;
    use crate::types::AttributeInput;
    use futures_executor::block_on;
    use serde_json::json;
    use wordless_commerce::products::create_product;
    use wordless_commerce::types::ProductInput;

    pub(crate) fn make_variable_with_size(db: &TestDb) -> i64 {
        let pid = block_on(create_product(
            db,
            ProductInput {
                name: Some(json!("Pack Hoodie")),
                price_minor: Some(json!(0)),
                stock: Some(json!(-1)),
                status: Some(json!("published")),
                product_type: Some(json!("variable")),
                sku: Some(json!("HOODIE")),
                ..Default::default()
            },
        ))
        .unwrap()
        .id;
        block_on(create_attribute(
            db,
            pid,
            AttributeInput {
                name: Some(json!("Size")),
                options: Some(json!(["S", "M", "L"])),
                ..Default::default()
            },
        ))
        .unwrap();
        pid
    }

    fn var_input(price: i64, stock: i64, size: &str) -> VariationInput {
        VariationInput {
            price_minor: Some(json!(price)),
            stock: Some(json!(stock)),
            attributes: Some(json!({ "Size": size })),
            ..Default::default()
        }
    }

    #[test]
    fn creates_variations_and_syncs_parent_price_to_minimum() {
        let db = TestDb::new();
        let pid = make_variable_with_size(&db);
        let small = block_on(create_variation(&db, pid, var_input(10000, 5, "S"))).unwrap();
        assert_eq!(small.label, "Size: S");
        assert_eq!(small.effective_price_minor, 10000);
        let large = block_on(create_variation(
            &db,
            pid,
            VariationInput {
                sale_price_minor: Some(json!(8000)),
                ..var_input(12000, 3, "L")
            },
        ))
        .unwrap();
        assert_eq!(large.effective_price_minor, 8000);
        // Parent price is the minimum effective variation price.
        let product = block_on(get_product(&db, pid)).unwrap().unwrap();
        assert_eq!(product.price_minor, 8000);
        let found = block_on(find_variation_by_attributes(
            &db,
            pid,
            &[("Size".to_string(), "L".to_string())],
        ))
        .unwrap()
        .unwrap();
        assert_eq!(found.id, large.id);
        assert!(block_on(find_variation_by_attributes(
            &db,
            pid,
            &[("Size".to_string(), "XL".to_string())],
        ))
        .unwrap()
        .is_none());
    }

    #[test]
    fn refuses_unknown_attribute_values_and_duplicate_skus() {
        let db = TestDb::new();
        let pid = make_variable_with_size(&db);
        let err = block_on(create_variation(&db, pid, var_input(10000, 5, "XL"))).unwrap_err();
        assert!(
            err.message.contains("not one of the options"),
            "{}",
            err.message
        );
        block_on(create_variation(
            &db,
            pid,
            VariationInput {
                sku: Some(json!("HOODIE-S")),
                ..var_input(10000, 5, "S")
            },
        ))
        .unwrap();
        // SKU collides with the parent product's SKU.
        let err = block_on(create_variation(
            &db,
            pid,
            VariationInput {
                sku: Some(json!("HOODIE")),
                ..var_input(10000, 5, "M")
            },
        ))
        .unwrap_err();
        assert!(err.message.contains("already used"), "{}", err.message);
    }
}
