//! Shop page data helpers (for `/shop` and `/shop/:slug`).
//!
//! These reuse the same product functions as the admin API — the
//! storefront lists published products only and a draft is never
//! reachable by slug. Rendering (HTML, pagination links, cart forms)
//! lives in `wordless-render`, which calls these helpers; v1's
//! `renderShop` / `renderProductPage` (`src/modules/render/service.ts`)
//! list published products with `listProducts({ status: "published" })`
//! and look a product up by slug the same way.

use wordless_db::Db;
use wordless_kernel::error::WordlessError;

use crate::products::{get_product_by_slug, list_products};
use crate::types::{Product, ProductList};

/// Published products for the `/shop` list, newest first. Page ≥ 1,
/// per-page clamped 1–100, default 20.
pub async fn list_shop_products(
    db: &dyn Db,
    page: i64,
    per_page: i64,
) -> Result<ProductList, WordlessError> {
    list_products(db, Some("published"), page, per_page).await
}

/// The published product for `/shop/:slug`, or `None` when the slug is
/// unknown or the product is a draft.
pub async fn get_shop_product(db: &dyn Db, slug: &str) -> Result<Option<Product>, WordlessError> {
    let product = get_product_by_slug(db, slug).await?;
    Ok(product.filter(|p| p.status == "published"))
}

/// The stock note for a product card, mirroring v1's `stockNote` text
/// (without the HTML): `""` for unlimited stock, `"Out of stock"` at
/// 0, `"Only N left"` at 5 or fewer, `""` otherwise.
pub fn stock_note(product: &Product) -> String {
    if product.stock < 0 {
        String::new()
    } else if product.stock == 0 {
        "Out of stock".to_string()
    } else if product.stock <= 5 {
        format!("Only {} left", product.stock)
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::products::create_product;
    use crate::test_support::SqliteDb;
    use crate::types::ProductInput;
    use futures_executor::block_on;
    use serde_json::json;

    fn make(db: &SqliteDb, name: &str, status: &str, stock: i64) -> Product {
        block_on(create_product(
            db,
            ProductInput {
                name: Some(json!(name)),
                price_minor: Some(json!(5000)),
                stock: Some(json!(stock)),
                status: Some(json!(status)),
                ..Default::default()
            },
        ))
        .unwrap()
    }

    #[test]
    fn shop_lists_published_products_only() {
        let db = SqliteDb::new();
        make(&db, "For Sale", "published", 10);
        make(&db, "Hidden Draft", "draft", 10);
        let list = block_on(list_shop_products(&db, 1, 20)).unwrap();
        assert_eq!(list.total, 1);
        assert_eq!(list.products[0].name, "For Sale");
    }

    #[test]
    fn shop_product_by_slug_hides_drafts() {
        let db = SqliteDb::new();
        make(&db, "For Sale", "published", 3);
        make(&db, "Hidden Draft", "draft", 10);
        assert!(block_on(get_shop_product(&db, "for-sale"))
            .unwrap()
            .is_some());
        assert!(block_on(get_shop_product(&db, "hidden-draft"))
            .unwrap()
            .is_none());
        assert!(block_on(get_shop_product(&db, "nope")).unwrap().is_none());
    }

    #[test]
    fn stock_note_matches_v1_text() {
        let db = SqliteDb::new();
        assert_eq!(stock_note(&make(&db, "A", "published", -1)), "");
        assert_eq!(stock_note(&make(&db, "B", "published", 0)), "Out of stock");
        assert_eq!(stock_note(&make(&db, "C", "published", 3)), "Only 3 left");
        assert_eq!(stock_note(&make(&db, "D", "published", 6)), "");
    }
}
