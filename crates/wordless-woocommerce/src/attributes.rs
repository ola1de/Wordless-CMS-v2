//! Product attributes (P8) — the option lists of a variable product
//! (`wl_woo_attributes`).
//!
//! An attribute belongs to one product: `name` ("Size"), `slug`
//! (derived), `position` (display order), `visible` (shown on the
//! product page), `is_variation` (its values pick a variation),
//! `options` (the full value list, e.g. ["S", "M", "L"]).
//! Variation rows carry one value per variation attribute
//! (`attributes_json` in `wl_woo_variations`).

use serde_json::Value;
use wordless_commerce::validate::slugify;
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, not_found, WordlessError};
use wordless_kernel::time::now_iso;
use wordless_kernel::validate::require_string;

use crate::types::{Attribute, AttributeInput};

pub(crate) fn row_to_attribute(row: &Row) -> Attribute {
    let options: Vec<String> =
        serde_json::from_str(&row_text(row, "options_json")).unwrap_or_default();
    Attribute {
        id: row_int(row, "id"),
        product_id: row_int(row, "product_id"),
        name: row_text(row, "name"),
        slug: row_text(row, "slug"),
        position: row_int(row, "position"),
        visible: row_int(row, "visible") == 1,
        is_variation: row_int(row, "is_variation") == 1,
        options,
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

fn require_options(value: &Value) -> Result<Vec<String>, WordlessError> {
    let Value::Array(items) = value else {
        return Err(bad_request(
            "Field \"options\" must be an array of strings.",
        ));
    };
    if items.is_empty() || items.len() > 100 {
        return Err(bad_request(
            "Field \"options\" must have between 1 and 100 values.",
        ));
    }
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Some(s) = item.as_str() else {
            return Err(bad_request(
                "Field \"options\" must be an array of strings.",
            ));
        };
        let s = s.trim();
        if s.is_empty() || s.len() > 100 {
            return Err(bad_request(
                "Each option must be between 1 and 100 characters.",
            ));
        }
        if !out.iter().any(|o: &String| o == s) {
            out.push(s.to_string());
        }
    }
    Ok(out)
}

fn require_bool(value: &Value, field: &str, default: bool) -> Result<bool, WordlessError> {
    match value {
        Value::Null => Ok(default),
        Value::Bool(b) => Ok(*b),
        _ => Err(bad_request(&format!(
            "Field \"{field}\" must be true or false."
        ))),
    }
}

/// The product must exist and be variable before it can have
/// attributes. Returns the product's type.
pub(crate) async fn require_variable_product(
    db: &dyn Db,
    product_id: i64,
) -> Result<(), WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT type FROM wl_products WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(product_id)])
        .first()
        .await?;
    let Some(row) = row else {
        return Err(not_found(&format!("Product {product_id} not found.")));
    };
    if row_text(&row, "type") != "variable" {
        return Err(bad_request(&format!(
            "Product {product_id} is not a variable product — attributes belong to variable products only."
        )));
    }
    Ok(())
}

/// All attributes of a product, in position order.
pub async fn list_attributes(
    db: &dyn Db,
    product_id: i64,
) -> Result<Vec<Attribute>, WordlessError> {
    let rows = db
        .prepare(
            "SELECT * FROM wl_woo_attributes WHERE product_id = ? ORDER BY position ASC, id ASC",
        )
        .bind(vec![Param::Integer(product_id)])
        .all::<Row>()
        .await?;
    Ok(rows.iter().map(row_to_attribute).collect())
}

/// Create an attribute on a variable product. The slug is derived
/// from the name and is unique per product.
pub async fn create_attribute(
    db: &dyn Db,
    product_id: i64,
    input: AttributeInput,
) -> Result<Attribute, WordlessError> {
    require_variable_product(db, product_id).await?;
    let name = require_string(input.name.as_ref().unwrap_or(&Value::Null), "name", 1, 100)?;
    let slug = slugify(&name);
    if slug.is_empty() {
        return Err(bad_request(
            "Field \"name\" must contain letters or digits.",
        ));
    }
    let options = require_options(input.options.as_ref().unwrap_or(&Value::Null))?;
    let position = match input.position.as_ref() {
        Some(v) if !v.is_null() => v
            .as_i64()
            .ok_or_else(|| bad_request("Field \"position\" must be an integer."))?,
        _ => {
            let row: Option<Row> = db
                .prepare("SELECT COUNT(*) AS n FROM wl_woo_attributes WHERE product_id = ?")
                .bind(vec![Param::Integer(product_id)])
                .first()
                .await?;
            row.as_ref().map(|r| row_int(r, "n")).unwrap_or(0)
        }
    };
    let visible = require_bool(
        input.visible.as_ref().unwrap_or(&Value::Null),
        "visible",
        true,
    )?;
    let is_variation = require_bool(
        input.is_variation.as_ref().unwrap_or(&Value::Null),
        "isVariation",
        true,
    )?;
    let now = now_iso();
    let options_json = serde_json::to_string(&options)
        .map_err(|e| WordlessError::new("internal_error", 500, &e.to_string()))?;
    let meta = db
        .prepare(
            "INSERT INTO wl_woo_attributes (product_id, name, slug, position, visible, is_variation, options_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(vec![
            Param::Integer(product_id),
            Param::Text(name),
            Param::Text(slug),
            Param::Integer(position),
            Param::Integer(visible as i64),
            Param::Integer(is_variation as i64),
            Param::Text(options_json),
            Param::Text(now.clone()),
            Param::Text(now),
        ])
        .run()
        .await;
    let meta = match meta {
        Ok(m) => m,
        Err(e) if e.to_string().contains("UNIQUE") => {
            return Err(bad_request(
                "This product already has an attribute with that name.",
            ))
        }
        Err(e) => return Err(e.into()),
    };
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_woo_attributes WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(meta.last_row_id.unwrap_or(0))])
        .first()
        .await?;
    row.map(|r| row_to_attribute(&r))
        .ok_or_else(|| WordlessError::new("internal_error", 500, "Attribute was not created."))
}

/// Update an attribute (partial: absent fields keep their value).
/// Renaming recomputes the slug; the per-product name uniqueness
/// rule still applies.
pub async fn update_attribute(
    db: &dyn Db,
    product_id: i64,
    attribute_id: i64,
    input: AttributeInput,
) -> Result<Attribute, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_woo_attributes WHERE id = ? AND product_id = ? LIMIT 1")
        .bind(vec![
            Param::Integer(attribute_id),
            Param::Integer(product_id),
        ])
        .first()
        .await?;
    let Some(row) = row else {
        return Err(not_found(&format!("Attribute {attribute_id} not found.")));
    };
    let existing = row_to_attribute(&row);
    let name = match input.name.as_ref() {
        Some(v) if !v.is_null() => require_string(v, "name", 1, 100)?,
        _ => existing.name.clone(),
    };
    let slug = slugify(&name);
    if slug.is_empty() {
        return Err(bad_request(
            "Field \"name\" must contain letters or digits.",
        ));
    }
    let options = match input.options.as_ref() {
        Some(v) if !v.is_null() => require_options(v)?,
        _ => existing.options.clone(),
    };
    let position = match input.position.as_ref() {
        Some(v) if !v.is_null() => v
            .as_i64()
            .ok_or_else(|| bad_request("Field \"position\" must be an integer."))?,
        _ => existing.position,
    };
    let visible = require_bool(
        input.visible.as_ref().unwrap_or(&Value::Null),
        "visible",
        existing.visible,
    )?;
    let is_variation = require_bool(
        input.is_variation.as_ref().unwrap_or(&Value::Null),
        "isVariation",
        existing.is_variation,
    )?;
    let options_json = serde_json::to_string(&options)
        .map_err(|e| WordlessError::new("internal_error", 500, &e.to_string()))?;
    let meta = db
        .prepare(
            "UPDATE wl_woo_attributes SET name = ?, slug = ?, position = ?, visible = ?, is_variation = ?, options_json = ?, updated_at = ? WHERE id = ? AND product_id = ?",
        )
        .bind(vec![
            Param::Text(name),
            Param::Text(slug),
            Param::Integer(position),
            Param::Integer(visible as i64),
            Param::Integer(is_variation as i64),
            Param::Text(options_json),
            Param::Text(now_iso()),
            Param::Integer(attribute_id),
            Param::Integer(product_id),
        ])
        .run()
        .await;
    if let Err(e) = meta {
        if e.to_string().contains("UNIQUE") {
            return Err(bad_request(
                "This product already has an attribute with that name.",
            ));
        }
        return Err(e.into());
    }
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_woo_attributes WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(attribute_id)])
        .first()
        .await?;
    row.map(|r| row_to_attribute(&r))
        .ok_or_else(|| WordlessError::new("internal_error", 500, "Attribute was not updated."))
}

/// Delete an attribute. Refused when a variation still uses it —
/// remove the variations first, so no variation is left pointing at
/// a missing attribute.
pub async fn delete_attribute(
    db: &dyn Db,
    product_id: i64,
    attribute_id: i64,
) -> Result<bool, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_woo_attributes WHERE id = ? AND product_id = ? LIMIT 1")
        .bind(vec![
            Param::Integer(attribute_id),
            Param::Integer(product_id),
        ])
        .first()
        .await?;
    let Some(row) = row else {
        return Err(not_found(&format!("Attribute {attribute_id} not found.")));
    };
    let name = row_text(&row, "name");
    let used: Option<Row> = db
        .prepare("SELECT COUNT(*) AS n FROM wl_woo_variations WHERE product_id = ? AND attributes_json LIKE ?")
        .bind(vec![
            Param::Integer(product_id),
            Param::Text(format!("%\"{name}\":%")),
        ])
        .first()
        .await?;
    if used.as_ref().map(|r| row_int(r, "n")).unwrap_or(0) > 0 {
        return Err(bad_request(&format!(
            "Attribute \"{name}\" is still used by one or more variations. Delete those variations first."
        )));
    }
    let meta = db
        .prepare("DELETE FROM wl_woo_attributes WHERE id = ? AND product_id = ?")
        .bind(vec![
            Param::Integer(attribute_id),
            Param::Integer(product_id),
        ])
        .run()
        .await?;
    Ok(meta.changes > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestDb;
    use futures_executor::block_on;
    use serde_json::json;
    use wordless_commerce::products::create_product;
    use wordless_commerce::types::ProductInput;

    fn make_variable(db: &TestDb) -> i64 {
        block_on(create_product(
            db,
            ProductInput {
                name: Some(json!("Pack Hoodie")),
                price_minor: Some(json!(0)),
                stock: Some(json!(-1)),
                status: Some(json!("published")),
                product_type: Some(json!("variable")),
                ..Default::default()
            },
        ))
        .unwrap()
        .id
    }

    fn attr_input(name: &str, options: Vec<&str>) -> AttributeInput {
        AttributeInput {
            name: Some(json!(name)),
            options: Some(json!(options)),
            ..Default::default()
        }
    }

    #[test]
    fn creates_lists_and_deletes_attributes_on_variable_products() {
        let db = TestDb::new();
        let pid = make_variable(&db);
        let a = block_on(create_attribute(
            &db,
            pid,
            attr_input("Size", vec!["S", "M", "L"]),
        ))
        .unwrap();
        assert_eq!(a.slug, "size");
        assert_eq!(a.options, vec!["S", "M", "L"]);
        assert!(a.is_variation);
        let list = block_on(list_attributes(&db, pid)).unwrap();
        assert_eq!(list.len(), 1);
        assert!(block_on(delete_attribute(&db, pid, a.id)).unwrap());
        assert_eq!(block_on(list_attributes(&db, pid)).unwrap().len(), 0);
    }

    #[test]
    fn refuses_attributes_on_simple_products_and_duplicate_names() {
        let db = TestDb::new();
        let simple = block_on(create_product(
            &db,
            ProductInput {
                name: Some(json!("Simple")),
                ..Default::default()
            },
        ))
        .unwrap();
        let err = block_on(create_attribute(
            &db,
            simple.id,
            attr_input("Size", vec!["S"]),
        ))
        .unwrap_err();
        assert!(
            err.message.contains("not a variable product"),
            "{}",
            err.message
        );
        let pid = make_variable(&db);
        block_on(create_attribute(&db, pid, attr_input("Size", vec!["S"]))).unwrap();
        let err = block_on(create_attribute(&db, pid, attr_input("Size", vec!["M"]))).unwrap_err();
        assert!(
            err.message.contains("already has an attribute"),
            "{}",
            err.message
        );
    }
}
