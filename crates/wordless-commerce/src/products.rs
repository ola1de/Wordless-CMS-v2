//! Products (port of v1 `src/modules/commerce/products.ts`).
//!
//! Stored in `wl_products`. Prices are integer minor units (see
//! `money`); stock is an integer count (`-1` = unlimited). Slugs are
//! unique; a clash appends `-2`, `-3`, … like posts do. Deleting a
//! product that appears in any order is refused — order items keep
//! their own name/price snapshot, but the product row is the audit
//! trail for what was sold.

use serde_json::Value;
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, not_found, WordlessError};
use wordless_kernel::time::now_iso;
use wordless_kernel::validate::require_string;

use crate::money::{require_currency, require_minor_units};
use crate::types::{Product, ProductInput, ProductList, PRODUCT_STATUS_DRAFT};
use crate::validate::{require_int, require_slug, slugify};

/// Map a `wl_products` row to a `Product` (public so the Woo crate
/// can read products by SKU without duplicating the mapping).
pub fn row_to_product(row: &Row) -> Product {
    Product {
        id: row_int(row, "id"),
        name: row_text(row, "name"),
        slug: row_text(row, "slug"),
        description: row_text(row, "description"),
        price_minor: row_int(row, "price_minor"),
        currency: row_text(row, "currency"),
        stock: row_int(row, "stock"),
        status: row_text(row, "status"),
        image_id: row_int(row, "image_id"),
        // Rows written before P8 have no `type` column value ("") —
        // read as "simple" (the wire omits it either way).
        product_type: {
            let t = row_text(row, "type");
            if t.is_empty() {
                "simple".to_string()
            } else {
                t
            }
        },
        sku: row_text(row, "sku"),
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

/// P8: `"simple"` or `"variable"` (default `"simple"`).
fn require_product_type(value: &Value) -> Result<String, WordlessError> {
    match value {
        Value::Null => Ok("simple".to_string()),
        Value::String(s) if s == "simple" || s == "variable" => Ok(s.clone()),
        _ => Err(bad_request(
            "Field \"type\" must be \"simple\" or \"variable\".",
        )),
    }
}

fn require_sku(value: &Value) -> Result<String, WordlessError> {
    match value {
        Value::Null => Ok(String::new()),
        Value::String(s) if s.len() <= 100 => Ok(s.trim().to_string()),
        Value::String(_) => Err(bad_request("Field \"sku\" must be at most 100 characters.")),
        _ => Err(bad_request("Field \"sku\" must be a string.")),
    }
}

/// True when `sku` is already used by another product (variations are
/// checked by the Woo crate, which owns them).
async fn sku_taken_by_product(
    db: &dyn Db,
    sku: &str,
    exclude_id: Option<i64>,
) -> Result<bool, WordlessError> {
    if sku.is_empty() {
        return Ok(false);
    }
    let row: Option<Row> = db
        .prepare("SELECT id FROM wl_products WHERE sku = ? LIMIT 1")
        .bind(vec![Param::Text(sku.to_string())])
        .first()
        .await?;
    Ok(match row {
        None => false,
        Some(r) => exclude_id != Some(row_int(&r, "id")),
    })
}

fn require_product_status(value: &Value) -> Result<String, WordlessError> {
    match value {
        Value::Null => Ok(PRODUCT_STATUS_DRAFT.to_string()),
        Value::String(s) if s == "draft" || s == "published" => Ok(s.clone()),
        _ => Err(bad_request(
            "Field \"status\" must be \"draft\" or \"published\".",
        )),
    }
}

async fn ensure_unique_slug(
    db: &dyn Db,
    base: &str,
    exclude_id: Option<i64>,
) -> Result<String, WordlessError> {
    let mut candidate = base.to_string();
    let mut n = 2;
    loop {
        let row: Option<Row> = db
            .prepare("SELECT id FROM wl_products WHERE slug = ? LIMIT 1")
            .bind(vec![Param::Text(candidate.clone())])
            .first()
            .await?;
        match row {
            None => return Ok(candidate),
            Some(r) if exclude_id == Some(row_int(&r, "id")) => return Ok(candidate),
            Some(_) => {
                candidate = format!("{base}-{n}");
                n += 1;
            }
        }
    }
}

/// Create a product. Defaults: price 0, currency `NGN`, stock `-1`
/// (unlimited), status `draft`, no image. A missing/empty `slug` is
/// derived from the name; a clash appends `-2`, `-3`, …
pub async fn create_product(db: &dyn Db, input: ProductInput) -> Result<Product, WordlessError> {
    let name = require_string(input.name.as_ref().unwrap_or(&Value::Null), "name", 1, 200)?;
    let slug_base = match input.slug.as_ref() {
        Some(Value::String(s)) if !s.is_empty() => require_slug(&Value::String(s.clone()), "slug")?,
        Some(v) if !v.is_null() => require_slug(v, "slug")?,
        _ => slugify(&name),
    };
    let slug = ensure_unique_slug(db, &slug_base, None).await?;
    let description = match input.description.as_ref() {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    if description.len() > 100_000 {
        return Err(bad_request(
            "Field \"description\" is too long (max 100,000 characters).",
        ));
    }
    let price_minor = match input.price_minor.as_ref() {
        Some(v) => require_minor_units(v, "priceMinor", 0)?,
        None => 0,
    };
    let currency = match input.currency.as_ref() {
        Some(v) => require_currency(v, "currency")?,
        None => "NGN".to_string(),
    };
    let stock = match input.stock.as_ref() {
        Some(v) => require_int(v, "stock", -1, i64::MAX)?,
        None => -1,
    };
    let status = match input.status.as_ref() {
        Some(v) => require_product_status(v)?,
        None => PRODUCT_STATUS_DRAFT.to_string(),
    };
    let image_id = match input.image_id.as_ref() {
        Some(v) => require_int(v, "imageId", 0, i64::MAX)?,
        None => 0,
    };
    let product_type = match input.product_type.as_ref() {
        Some(v) => require_product_type(v)?,
        None => "simple".to_string(),
    };
    let sku = match input.sku.as_ref() {
        Some(v) => require_sku(v)?,
        None => String::new(),
    };
    if sku_taken_by_product(db, &sku, None).await? {
        return Err(bad_request(&format!(
            "SKU \"{sku}\" is already used by another product."
        )));
    }
    let now = now_iso();
    let meta = db
        .prepare(
            "INSERT INTO wl_products (name, slug, description, price_minor, currency, stock, status, image_id, type, sku, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(vec![
            Param::Text(name),
            Param::Text(slug),
            Param::Text(description),
            Param::Integer(price_minor),
            Param::Text(currency),
            Param::Integer(stock),
            Param::Text(status),
            Param::Integer(image_id),
            Param::Text(product_type),
            Param::Text(sku),
            Param::Text(now.clone()),
            Param::Text(now),
        ])
        .run()
        .await?;
    let created = get_product(db, meta.last_row_id.unwrap_or(0)).await?;
    created.ok_or_else(|| WordlessError::new("internal_error", 500, "Product was not created."))
}

/// Fetch a product by id, or `None` when missing.
pub async fn get_product(db: &dyn Db, id: i64) -> Result<Option<Product>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_products WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(id)])
        .first()
        .await?;
    Ok(row.map(|r| row_to_product(&r)))
}

/// Fetch a product by slug, or `None` when missing.
pub async fn get_product_by_slug(
    db: &dyn Db,
    slug: &str,
) -> Result<Option<Product>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_products WHERE slug = ? LIMIT 1")
        .bind(vec![Param::Text(slug.to_string())])
        .first()
        .await?;
    Ok(row.map(|r| row_to_product(&r)))
}

/// Update a product. Only the fields present in `input` change; an
/// absent `slug` keeps the current one.
pub async fn update_product(
    db: &dyn Db,
    id: i64,
    input: ProductInput,
) -> Result<Product, WordlessError> {
    let existing = get_product(db, id)
        .await?
        .ok_or_else(|| not_found(&format!("Product {id} not found.")))?;
    let name = match input.name.as_ref() {
        Some(v) => require_string(v, "name", 1, 200)?,
        None => existing.name.clone(),
    };
    let slug = match input.slug.as_ref() {
        Some(v) => ensure_unique_slug(db, &require_slug(v, "slug")?, Some(id)).await?,
        None => existing.slug.clone(),
    };
    let description = match input.description.as_ref() {
        Some(Value::String(s)) => s.clone(),
        Some(_) => String::new(),
        None => existing.description.clone(),
    };
    if description.len() > 100_000 {
        return Err(bad_request(
            "Field \"description\" is too long (max 100,000 characters).",
        ));
    }
    let price_minor = match input.price_minor.as_ref() {
        Some(v) => require_minor_units(v, "priceMinor", 0)?,
        None => existing.price_minor,
    };
    let currency = match input.currency.as_ref() {
        Some(v) => require_currency(v, "currency")?,
        None => existing.currency.clone(),
    };
    let stock = match input.stock.as_ref() {
        Some(v) => require_int(v, "stock", -1, i64::MAX)?,
        None => existing.stock,
    };
    let status = match input.status.as_ref() {
        Some(v) => require_product_status(v)?,
        None => existing.status.clone(),
    };
    let image_id = match input.image_id.as_ref() {
        Some(v) => require_int(v, "imageId", 0, i64::MAX)?,
        None => existing.image_id,
    };
    let product_type = match input.product_type.as_ref() {
        Some(v) => require_product_type(v)?,
        None => existing.product_type.clone(),
    };
    // A variable product keeps its type when variations exist; a
    // simple product never has variations, so no check is needed here.
    let sku = match input.sku.as_ref() {
        Some(v) => require_sku(v)?,
        None => existing.sku.clone(),
    };
    if sku_taken_by_product(db, &sku, Some(id)).await? {
        return Err(bad_request(&format!(
            "SKU \"{sku}\" is already used by another product."
        )));
    }
    db.prepare(
        "UPDATE wl_products SET name = ?, slug = ?, description = ?, price_minor = ?, currency = ?, stock = ?, status = ?, image_id = ?, type = ?, sku = ?, updated_at = ? WHERE id = ?",
    )
    .bind(vec![
        Param::Text(name),
        Param::Text(slug),
        Param::Text(description),
        Param::Integer(price_minor),
        Param::Text(currency),
        Param::Integer(stock),
        Param::Text(status),
        Param::Integer(image_id),
        Param::Text(product_type),
        Param::Text(sku),
        Param::Text(now_iso()),
        Param::Integer(id),
    ])
    .run()
    .await?;
    let updated = get_product(db, id).await?;
    updated.ok_or_else(|| WordlessError::new("internal_error", 500, "Product was not updated."))
}

/// Delete a product. Refused when the product appears in any order.
/// Returns `true` when a row was deleted.
pub async fn delete_product(db: &dyn Db, id: i64) -> Result<bool, WordlessError> {
    if get_product(db, id).await?.is_none() {
        return Err(not_found(&format!("Product {id} not found.")));
    }
    let row: Option<Row> = db
        .prepare("SELECT COUNT(*) AS n FROM wl_order_items WHERE product_id = ?")
        .bind(vec![Param::Integer(id)])
        .first()
        .await?;
    let used = row.as_ref().map(|r| row_int(r, "n")).unwrap_or(0);
    if used > 0 {
        return Err(bad_request(&format!(
            "Product {id} appears in {used} order item(s) and cannot be deleted. Unpublish it instead."
        )));
    }
    let meta = db
        .prepare("DELETE FROM wl_products WHERE id = ?")
        .bind(vec![Param::Integer(id)])
        .run()
        .await?;
    Ok(meta.changes > 0)
}

/// List products, newest first. `status` is `"draft"`, `"published"` or
/// `None` (all). Page ≥ 1, per-page clamped 1–100, default 20.
pub async fn list_products(
    db: &dyn Db,
    status: Option<&str>,
    page: i64,
    per_page: i64,
) -> Result<ProductList, WordlessError> {
    let page = page.max(1);
    let per_page = per_page.clamp(1, 100);
    let offset = (page - 1) * per_page;
    let (where_sql, mut params): (&str, Vec<Param>) = match status {
        Some("draft") => ("WHERE status = ?", vec![Param::Text("draft".to_string())]),
        Some("published") => (
            "WHERE status = ?",
            vec![Param::Text("published".to_string())],
        ),
        Some(other) => {
            return Err(bad_request(&format!(
                "Field \"status\" must be \"draft\" or \"published\", got \"{other}\"."
            )))
        }
        None => ("", vec![]),
    };
    let total_row: Option<Row> = db
        .prepare(&format!(
            "SELECT COUNT(*) AS n FROM wl_products {where_sql}"
        ))
        .bind(params.clone())
        .first()
        .await?;
    let total = total_row.as_ref().map(|r| row_int(r, "n")).unwrap_or(0);
    params.push(Param::Integer(per_page));
    params.push(Param::Integer(offset));
    let rows = db
        .prepare(&format!(
            "SELECT * FROM wl_products {where_sql} ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?"
        ))
        .bind(params)
        .all::<Row>()
        .await?;
    Ok(ProductList {
        products: rows.iter().map(row_to_product).collect(),
        total,
        page,
        per_page,
        total_pages: ((total + per_page - 1) / per_page).max(1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SqliteDb;
    use futures_executor::block_on;
    use serde_json::json;

    fn input(name: &str, price: i64, stock: i64, status: &str) -> ProductInput {
        ProductInput {
            name: Some(json!(name)),
            price_minor: Some(json!(price)),
            stock: Some(json!(stock)),
            status: Some(json!(status)),
            ..Default::default()
        }
    }

    #[test]
    fn creates_reads_updates_and_lists_products() {
        let db = SqliteDb::new();
        let created = block_on(create_product(
            &db,
            input("Pack Hoodie", 125000, 5, "published"),
        ))
        .unwrap();
        assert_eq!(created.slug, "pack-hoodie");
        assert_eq!(created.price_minor, 125000);
        let updated = block_on(update_product(
            &db,
            created.id,
            ProductInput {
                price_minor: Some(json!(150000)),
                stock: Some(json!(3)),
                ..Default::default()
            },
        ))
        .unwrap();
        assert_eq!(updated.price_minor, 150000);
        assert_eq!(updated.stock, 3);
        let list = block_on(list_products(&db, Some("published"), 1, 20)).unwrap();
        assert_eq!(list.total, 1);
        assert_eq!(
            block_on(get_product(&db, created.id))
                .unwrap()
                .unwrap()
                .name,
            "Pack Hoodie"
        );
    }

    #[test]
    fn makes_slugs_unique_with_numeric_suffix() {
        let db = SqliteDb::new();
        let a = block_on(create_product(
            &db,
            input("Pack Hoodie", 125000, 5, "published"),
        ))
        .unwrap();
        let b = block_on(create_product(
            &db,
            input("Pack Hoodie", 125000, 5, "published"),
        ))
        .unwrap();
        assert_eq!(a.slug, "pack-hoodie");
        assert_eq!(b.slug, "pack-hoodie-2");
    }

    #[test]
    fn rejects_fractional_prices_and_bad_currency() {
        let db = SqliteDb::new();
        let err = block_on(create_product(
            &db,
            ProductInput {
                name: Some(json!("Bad")),
                price_minor: Some(json!(10.5)),
                ..Default::default()
            },
        ))
        .unwrap_err();
        assert!(err.message.contains("integer"), "{}", err.message);
        let err = block_on(create_product(
            &db,
            ProductInput {
                name: Some(json!("Bad")),
                currency: Some(json!("Naira")),
                ..Default::default()
            },
        ))
        .unwrap_err();
        assert!(err.message.contains("currency"), "{}", err.message);
    }

    #[test]
    fn defaults_are_draft_unlimited_ngn() {
        let db = SqliteDb::new();
        let p = block_on(create_product(
            &db,
            ProductInput {
                name: Some(json!("Digital")),
                ..Default::default()
            },
        ))
        .unwrap();
        assert_eq!(p.status, "draft");
        assert_eq!(p.stock, -1);
        assert_eq!(p.currency, "NGN");
        assert_eq!(p.price_minor, 0);
    }
}
