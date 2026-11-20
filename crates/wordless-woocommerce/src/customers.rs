//! Customers (P8) — `wl_woo_customers`.
//!
//! A customer is keyed by email (unique). Checkout upserts by email
//! (name + billing address from the latest order); CSV import upserts
//! the same way. `total_orders` / `total_spent_minor` are maintained
//! from orders placed through the Woo checkout and from imports —
//! they are store records, never analytics (no invented numbers:
//! they count exactly the orders in `wl_orders` for that email).
//! `source` is 'wordless' | 'woo' | 'woo-import'.

use serde_json::Value;
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::WordlessError;
use wordless_kernel::time::now_iso;
use wordless_kernel::validate::require_email;

use crate::types::Customer;

pub(crate) fn row_to_customer(row: &Row) -> Customer {
    Customer {
        id: row_int(row, "id"),
        email: row_text(row, "email"),
        first_name: row_text(row, "first_name"),
        last_name: row_text(row, "last_name"),
        username: row_text(row, "username"),
        billing_country: row_text(row, "billing_country"),
        billing_state: row_text(row, "billing_state"),
        billing_postcode: row_text(row, "billing_postcode"),
        total_orders: row_int(row, "total_orders"),
        total_spent_minor: row_int(row, "total_spent_minor"),
        source: row_text(row, "source"),
        source_id: row_text(row, "source_id"),
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

/// Upsert a customer by email. New customers start at 0 orders / 0
/// spent; an existing customer keeps their totals and gets the
/// freshest name/address. Returns the customer row.
pub async fn upsert_customer(
    db: &dyn Db,
    email: &str,
    first_name: &str,
    last_name: &str,
    billing_country: &str,
    billing_state: &str,
    billing_postcode: &str,
    source: &str,
    source_id: &str,
) -> Result<Customer, WordlessError> {
    let email = require_email(&Value::String(email.to_string()), "email")?;
    let now = now_iso();
    db.prepare(
        "INSERT INTO wl_woo_customers (email, first_name, last_name, billing_country, billing_state, billing_postcode, total_orders, total_spent_minor, source, source_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, 0, 0, ?, ?, ?, ?)
         ON CONFLICT(email) DO UPDATE SET first_name = excluded.first_name, last_name = excluded.last_name, billing_country = excluded.billing_country, billing_state = excluded.billing_state, billing_postcode = excluded.billing_postcode, updated_at = excluded.updated_at",
    )
    .bind(vec![
        Param::Text(email.clone()),
        Param::Text(first_name.to_string()),
        Param::Text(last_name.to_string()),
        Param::Text(billing_country.to_string()),
        Param::Text(billing_state.to_string()),
        Param::Text(billing_postcode.to_string()),
        Param::Text(source.to_string()),
        Param::Text(source_id.to_string()),
        Param::Text(now.clone()),
        Param::Text(now),
    ])
    .run()
    .await?;
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_woo_customers WHERE email = ? LIMIT 1")
        .bind(vec![Param::Text(email)])
        .first()
        .await?;
    row.map(|r| row_to_customer(&r))
        .ok_or_else(|| WordlessError::new("internal_error", 500, "Customer was not saved."))
}

/// Record one order against a customer (totals only — the order row
/// itself is written by the order code). Called after an order is
/// created, never on a failed checkout.
pub async fn record_order_for_customer(
    db: &dyn Db,
    email: &str,
    total_minor: i64,
) -> Result<(), WordlessError> {
    db.prepare(
        "UPDATE wl_woo_customers SET total_orders = total_orders + 1, total_spent_minor = total_spent_minor + ?, updated_at = ? WHERE email = ?",
    )
    .bind(vec![
        Param::Integer(total_minor),
        Param::Text(now_iso()),
        Param::Text(email.to_string()),
    ])
    .run()
    .await?;
    Ok(())
}

/// Fetch a customer by id, or `None` when missing.
pub async fn get_customer(db: &dyn Db, id: i64) -> Result<Option<Customer>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_woo_customers WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(id)])
        .first()
        .await?;
    Ok(row.map(|r| row_to_customer(&r)))
}

/// List customers, newest first. Page ≥ 1, per-page clamped 1–100,
/// default 20.
pub async fn list_customers(
    db: &dyn Db,
    page: i64,
    per_page: i64,
) -> Result<(Vec<Customer>, i64), WordlessError> {
    let page = page.max(1);
    let per_page = per_page.clamp(1, 100);
    let offset = (page - 1) * per_page;
    let total_row: Option<Row> = db
        .prepare("SELECT COUNT(*) AS n FROM wl_woo_customers")
        .bind(vec![])
        .first()
        .await?;
    let total = total_row.as_ref().map(|r| row_int(r, "n")).unwrap_or(0);
    let rows = db
        .prepare(
            "SELECT * FROM wl_woo_customers ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?",
        )
        .bind(vec![Param::Integer(per_page), Param::Integer(offset)])
        .all::<Row>()
        .await?;
    Ok((rows.iter().map(row_to_customer).collect(), total))
}

/// Split a full name ("Ada Lovelace") into (first, last). One word →
/// (word, "").
pub fn split_name(full_name: &str) -> (String, String) {
    let full_name = full_name.trim();
    match full_name.rfind(' ') {
        Some(i) => (
            full_name[..i].trim().to_string(),
            full_name[i + 1..].trim().to_string(),
        ),
        None => (full_name.to_string(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestDb;
    use futures_executor::block_on;

    #[test]
    fn upserts_by_email_and_counts_orders() {
        let db = TestDb::new();
        let c = block_on(upsert_customer(
            &db,
            "ada@example.test",
            "Ada",
            "Lovelace",
            "NG",
            "LA",
            "100001",
            "woo",
            "",
        ))
        .unwrap();
        assert_eq!(c.total_orders, 0);
        block_on(record_order_for_customer(&db, "ada@example.test", 250000)).unwrap();
        let c2 = block_on(upsert_customer(
            &db,
            "ada@example.test",
            "Ada",
            "Byron",
            "NG",
            "OY",
            "200001",
            "woo",
            "",
        ))
        .unwrap();
        assert_eq!(c2.id, c.id);
        assert_eq!(c2.last_name, "Byron");
        assert_eq!(c2.total_orders, 1);
        assert_eq!(c2.total_spent_minor, 250000);
        let err = block_on(upsert_customer(
            &db,
            "not-an-email",
            "A",
            "B",
            "",
            "",
            "",
            "woo",
            "",
        ))
        .unwrap_err();
        assert!(err.message.contains("valid email"), "{}", err.message);
    }

    #[test]
    fn splits_names() {
        assert_eq!(
            split_name("Ada Lovelace"),
            ("Ada".to_string(), "Lovelace".to_string())
        );
        assert_eq!(split_name("Ada"), ("Ada".to_string(), String::new()));
        assert_eq!(
            split_name("  Ada  Van  "),
            ("Ada".to_string(), "Van".to_string())
        );
    }
}
