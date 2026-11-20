//! Woo product views (P8) — list/detail over the P3 `wl_products`
//! table plus the Woo detail (attributes, variations, terms).
//!
//! Writes go through `wordless-commerce` (`create_product` /
//! `update_product` / `delete_product`); this module only reads and
//! assembles the Woo view. SKU is the WooCommerce stable key:
//! `get_product_by_sku` finds a product by SKU, `get_variation_by_sku`
//! finds a variation (returning its parent product id).

use wordless_commerce::products::get_product;
use wordless_commerce::types::{Product, ProductList};
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, WordlessError};

use crate::attributes::list_attributes;
use crate::types::WooProduct;
use crate::variations::list_variations;

/// Categories/tags of a product, by taxonomy.
pub async fn get_product_terms(
    db: &dyn Db,
    product_id: i64,
    taxonomy: &str,
) -> Result<Vec<String>, WordlessError> {
    let rows = db
        .prepare("SELECT term FROM wl_woo_product_terms WHERE product_id = ? AND taxonomy = ? ORDER BY term ASC")
        .bind(vec![Param::Integer(product_id), Param::Text(taxonomy.to_string())])
        .all::<Row>()
        .await?;
    Ok(rows.iter().map(|r| row_text(r, "term")).collect())
}

/// Replace a product's terms for one taxonomy.
pub async fn set_product_terms(
    db: &dyn Db,
    product_id: i64,
    taxonomy: &str,
    terms: &[String],
) -> Result<(), WordlessError> {
    db.prepare("DELETE FROM wl_woo_product_terms WHERE product_id = ? AND taxonomy = ?")
        .bind(vec![
            Param::Integer(product_id),
            Param::Text(taxonomy.to_string()),
        ])
        .run()
        .await?;
    for term in terms {
        let term = term.trim();
        if term.is_empty() {
            continue;
        }
        db.prepare(
            "INSERT OR IGNORE INTO wl_woo_product_terms (product_id, taxonomy, term) VALUES (?, ?, ?)",
        )
        .bind(vec![
            Param::Integer(product_id),
            Param::Text(taxonomy.to_string()),
            Param::Text(term.to_string()),
        ])
        .run()
        .await?;
    }
    Ok(())
}

/// Delete a product's WooCommerce extras (attributes, variations,
/// imported terms) after the product itself is deleted. Called from
/// the DELETE route — `wordless-commerce::delete_product` refuses
/// when the product is in an order, so this only runs for products
/// that are actually gone and never orphans rows.
pub async fn delete_woo_product_extras(db: &dyn Db, product_id: i64) -> Result<(), WordlessError> {
    for table in [
        "wl_woo_attributes",
        "wl_woo_variations",
        "wl_woo_product_terms",
    ] {
        db.prepare(&format!("DELETE FROM {table} WHERE product_id = ?"))
            .bind(vec![Param::Integer(product_id)])
            .run()
            .await?;
    }
    Ok(())
}

/// A product with its Woo detail, or `None` when missing.
pub async fn get_woo_product(db: &dyn Db, id: i64) -> Result<Option<WooProduct>, WordlessError> {
    let Some(product) = get_product(db, id).await? else {
        return Ok(None);
    };
    Ok(Some(WooProduct {
        attributes: list_attributes(db, id).await?,
        variations: list_variations(db, id).await?,
        categories: get_product_terms(db, id, "product_cat").await?,
        tags: get_product_terms(db, id, "product_tag").await?,
        product,
    }))
}

/// Find a product by SKU ("" never matches), or `None`.
pub async fn get_product_by_sku(db: &dyn Db, sku: &str) -> Result<Option<Product>, WordlessError> {
    if sku.is_empty() {
        return Ok(None);
    }
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_products WHERE sku = ? LIMIT 1")
        .bind(vec![Param::Text(sku.to_string())])
        .first()
        .await?;
    Ok(row.map(|r| wordless_commerce::products::row_to_product(&r)))
}

/// Find a variation by SKU ("" never matches): (parent product id,
/// variation id), or `None`.
pub async fn get_variation_by_sku(
    db: &dyn Db,
    sku: &str,
) -> Result<Option<(i64, i64)>, WordlessError> {
    if sku.is_empty() {
        return Ok(None);
    }
    let row: Option<Row> = db
        .prepare("SELECT product_id, id FROM wl_woo_variations WHERE sku = ? LIMIT 1")
        .bind(vec![Param::Text(sku.to_string())])
        .first()
        .await?;
    Ok(row.map(|r| (row_int(&r, "product_id"), row_int(&r, "id"))))
}

/// List products with an optional type filter (`simple`/`variable`)
/// and status filter. Page ≥ 1, per-page clamped 1–100, default 20.
pub async fn list_woo_products(
    db: &dyn Db,
    product_type: Option<&str>,
    status: Option<&str>,
    page: i64,
    per_page: i64,
) -> Result<ProductList, WordlessError> {
    let page = page.max(1);
    let per_page = per_page.clamp(1, 100);
    let offset = (page - 1) * per_page;
    let mut where_sql = String::new();
    let mut params: Vec<Param> = Vec::new();
    if let Some(t) = product_type {
        if t != "simple" && t != "variable" {
            return Err(bad_request(
                "Field \"type\" must be \"simple\" or \"variable\".",
            ));
        }
        where_sql.push_str(" WHERE type = ?");
        params.push(Param::Text(t.to_string()));
    }
    if let Some(s) = status {
        if s != "draft" && s != "published" {
            return Err(bad_request(
                "Field \"status\" must be \"draft\" or \"published\".",
            ));
        }
        where_sql.push_str(if where_sql.is_empty() {
            " WHERE status = ?"
        } else {
            " AND status = ?"
        });
        params.push(Param::Text(s.to_string()));
    }
    let total_row: Option<Row> = db
        .prepare(&format!("SELECT COUNT(*) AS n FROM wl_products{where_sql}"))
        .bind(params.clone())
        .first()
        .await?;
    let total = total_row.as_ref().map(|r| row_int(r, "n")).unwrap_or(0);
    params.push(Param::Integer(per_page));
    params.push(Param::Integer(offset));
    let rows = db
        .prepare(&format!(
            "SELECT * FROM wl_products{where_sql} ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?"
        ))
        .bind(params)
        .all::<Row>()
        .await?;
    Ok(ProductList {
        products: rows
            .iter()
            .map(wordless_commerce::products::row_to_product)
            .collect(),
        total,
        page,
        per_page,
        total_pages: ((total + per_page - 1) / per_page).max(1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestDb;
    use futures_executor::block_on;
    use serde_json::json;
    use wordless_commerce::products::create_product;
    use wordless_commerce::types::ProductInput;

    #[test]
    fn terms_round_trip_and_sku_lookup() {
        let db = TestDb::new();
        let p = block_on(create_product(
            &db,
            ProductInput {
                name: Some(json!("Pack Hoodie")),
                sku: Some(json!("HOODIE")),
                product_type: Some(json!("variable")),
                status: Some(json!("published")),
                ..Default::default()
            },
        ))
        .unwrap();
        block_on(set_product_terms(
            &db,
            p.id,
            "product_cat",
            &["Clothing".to_string(), "Hoodies".to_string()],
        ))
        .unwrap();
        block_on(set_product_terms(
            &db,
            p.id,
            "product_tag",
            &["pack".to_string()],
        ))
        .unwrap();
        let detail = block_on(get_woo_product(&db, p.id)).unwrap().unwrap();
        assert_eq!(detail.categories, vec!["Clothing", "Hoodies"]);
        assert_eq!(detail.tags, vec!["pack"]);
        assert_eq!(
            block_on(get_product_by_sku(&db, "HOODIE"))
                .unwrap()
                .unwrap()
                .id,
            p.id
        );
        assert!(block_on(get_product_by_sku(&db, "NOPE")).unwrap().is_none());
        let list = block_on(list_woo_products(&db, Some("variable"), None, 1, 20)).unwrap();
        assert_eq!(list.total, 1);
        let list = block_on(list_woo_products(&db, Some("simple"), None, 1, 20)).unwrap();
        assert_eq!(list.total, 0);
    }
}
