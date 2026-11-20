//! Taxes (P8) — `wl_woo_tax_rates` + store settings.
//!
//! A rate is (country, state, rate_bp). Matching is most-specific
//! first: country+state, then country-only, then the global rate
//! (country "" and state ""). Rates are basis points (750 = 7.5%)
//! and tax is computed on (subtotal − discount), rounded half-up
//! with integer math: `(taxable * rate_bp + 5000) / 10000`.
//!
//! Tax is OFF by default (`wl_woo_tax_enabled`, default false) and
//! prices are exclusive by default (`wl_woo_prices_include_tax`,
//! default false), so the existing P3 checkout totals cannot change
//! until the owner enables tax. The settings live in `wp_options`,
//! the same as the payment-provider selection.

use serde_json::Value;
use wordless_commerce::validate::require_int;
use wordless_db::options::{get_option, set_option};
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, not_found, WordlessError};
use wordless_kernel::time::now_iso;
use wordless_kernel::validate::require_string;

use crate::types::{TaxRate, TaxRateInput};

/// `wp_options` key for "tax is enabled".
pub const TAX_ENABLED_OPTION: &str = "wl_woo_tax_enabled";
/// `wp_options` key for "prices include tax".
pub const PRICES_INCLUDE_TAX_OPTION: &str = "wl_woo_prices_include_tax";

pub(crate) fn row_to_tax_rate(row: &Row) -> TaxRate {
    TaxRate {
        id: row_int(row, "id"),
        name: row_text(row, "name"),
        country: row_text(row, "country"),
        state: row_text(row, "state"),
        rate_bp: row_int(row, "rate_bp"),
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

fn require_country_code(value: &Value, field: &str) -> Result<String, WordlessError> {
    match value {
        Value::Null => Ok(String::new()),
        Value::String(s) => {
            let s = s.trim().to_ascii_uppercase();
            if s.is_empty() {
                return Ok(String::new());
            }
            if s.len() == 2 && s.chars().all(|c| c.is_ascii_alphabetic()) {
                Ok(s)
            } else {
                Err(bad_request(&format!(
                    "Field \"{field}\" must be a 2-letter country code (e.g. NG) or empty for any country."
                )))
            }
        }
        _ => Err(bad_request(&format!(
            "Field \"{field}\" must be a country code string."
        ))),
    }
}

/// True when tax is enabled for the store (default false).
pub async fn tax_enabled(db: &dyn Db) -> Result<bool, WordlessError> {
    let v: bool = get_option(db, TAX_ENABLED_OPTION, false).await?;
    Ok(v)
}

/// True when stored prices already include tax (default false).
/// An enabled store with inclusive prices computes the tax portion
/// as `taxable − taxable * 10000 / (10000 + rate_bp)`; the total
/// never changes — only the breakdown does.
pub async fn prices_include_tax(db: &dyn Db) -> Result<bool, WordlessError> {
    let v: bool = get_option(db, PRICES_INCLUDE_TAX_OPTION, false).await?;
    Ok(v)
}

/// Store the two tax settings.
pub async fn set_tax_settings(
    db: &dyn Db,
    enabled: bool,
    include_tax: bool,
) -> Result<(bool, bool), WordlessError> {
    set_option(db, TAX_ENABLED_OPTION, &enabled).await?;
    set_option(db, PRICES_INCLUDE_TAX_OPTION, &include_tax).await?;
    Ok((enabled, include_tax))
}

/// The most specific rate for (country, state): country+state, then
/// country-only, then global. `None` when no rate matches at all.
pub async fn rate_for(
    db: &dyn Db,
    country: &str,
    state: &str,
) -> Result<Option<TaxRate>, WordlessError> {
    let country = country.trim().to_ascii_uppercase();
    let state = state.trim().to_ascii_uppercase();
    let rows = db
        .prepare(
            "SELECT * FROM wl_woo_tax_rates WHERE (country = ? AND state = ?) OR (country = ? AND state = '') OR (country = '' AND state = '') ORDER BY (country <> '') + (state <> '') DESC, id ASC",
        )
        .bind(vec![
            Param::Text(country.clone()),
            Param::Text(state.clone()),
            Param::Text(country),
        ])
        .all::<Row>()
        .await?;
    Ok(rows.first().map(row_to_tax_rate))
}

/// Tax on `taxable_minor` at `rate_bp`, rounded half-up (integer
/// math only). When prices already include tax, this is the tax
/// portion inside the price — the total does not grow.
pub fn tax_on(taxable_minor: i64, rate_bp: i64, prices_include_tax: bool) -> i64 {
    if rate_bp <= 0 || taxable_minor <= 0 {
        return 0;
    }
    if prices_include_tax {
        taxable_minor - (taxable_minor * 10000 + (10000 + rate_bp) / 2) / (10000 + rate_bp)
    } else {
        (taxable_minor * rate_bp + 5000) / 10000
    }
}

/// All tax rates, in match order (most specific first).
pub async fn list_tax_rates(db: &dyn Db) -> Result<Vec<TaxRate>, WordlessError> {
    let rows = db
        .prepare("SELECT * FROM wl_woo_tax_rates ORDER BY (country <> '') + (state <> '') DESC, country ASC, state ASC, id ASC")
        .bind(vec![])
        .all::<Row>()
        .await?;
    Ok(rows.iter().map(row_to_tax_rate).collect())
}

/// Create a tax rate.
pub async fn create_tax_rate(db: &dyn Db, input: TaxRateInput) -> Result<TaxRate, WordlessError> {
    let name = require_string(input.name.as_ref().unwrap_or(&Value::Null), "name", 1, 100)?;
    let country = require_country_code(input.country.as_ref().unwrap_or(&Value::Null), "country")?;
    let state = match input.state.as_ref() {
        Some(Value::String(s)) => s.trim().to_ascii_uppercase(),
        Some(Value::Null) | None => String::new(),
        _ => return Err(bad_request("Field \"state\" must be a string.")),
    };
    let rate_bp = match input.rate_bp.as_ref() {
        Some(v) => require_int(v, "rateBp", 0, 10000)?,
        None => {
            return Err(bad_request(
                "Field \"rateBp\" is required (basis points, 750 = 7.5%).",
            ))
        }
    };
    let now = now_iso();
    let meta = db
        .prepare(
            "INSERT INTO wl_woo_tax_rates (name, country, state, rate_bp, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(vec![
            Param::Text(name),
            Param::Text(country),
            Param::Text(state),
            Param::Integer(rate_bp),
            Param::Text(now.clone()),
            Param::Text(now),
        ])
        .run()
        .await?;
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_woo_tax_rates WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(meta.last_row_id.unwrap_or(0))])
        .first()
        .await?;
    row.map(|r| row_to_tax_rate(&r))
        .ok_or_else(|| WordlessError::new("internal_error", 500, "Tax rate was not created."))
}

/// Update a tax rate (partial: absent fields keep their value).
pub async fn update_tax_rate(
    db: &dyn Db,
    id: i64,
    input: TaxRateInput,
) -> Result<TaxRate, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_woo_tax_rates WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(id)])
        .first()
        .await?;
    let Some(row) = row else {
        return Err(not_found(&format!("Tax rate {id} not found.")));
    };
    let existing = row_to_tax_rate(&row);
    let name = match input.name.as_ref() {
        Some(v) if !v.is_null() => require_string(v, "name", 1, 100)?,
        _ => existing.name.clone(),
    };
    let country = match input.country.as_ref() {
        Some(v) if !v.is_null() => require_country_code(v, "country")?,
        _ => existing.country.clone(),
    };
    let state = match input.state.as_ref() {
        Some(Value::String(s)) => s.trim().to_ascii_uppercase(),
        Some(Value::Null) => String::new(),
        None => existing.state.clone(),
        _ => return Err(bad_request("Field \"state\" must be a string.")),
    };
    let rate_bp = match input.rate_bp.as_ref() {
        Some(v) if !v.is_null() => require_int(v, "rateBp", 0, 10000)?,
        _ => existing.rate_bp,
    };
    db.prepare(
        "UPDATE wl_woo_tax_rates SET name = ?, country = ?, state = ?, rate_bp = ?, updated_at = ? WHERE id = ?",
    )
    .bind(vec![
        Param::Text(name),
        Param::Text(country),
        Param::Text(state),
        Param::Integer(rate_bp),
        Param::Text(now_iso()),
        Param::Integer(id),
    ])
    .run()
    .await?;
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_woo_tax_rates WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(id)])
        .first()
        .await?;
    row.map(|r| row_to_tax_rate(&r))
        .ok_or_else(|| WordlessError::new("internal_error", 500, "Tax rate was not updated."))
}

/// Delete a tax rate.
pub async fn delete_tax_rate(db: &dyn Db, id: i64) -> Result<bool, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT id FROM wl_woo_tax_rates WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(id)])
        .first()
        .await?;
    if row.is_none() {
        return Err(not_found(&format!("Tax rate {id} not found.")));
    }
    let meta = db
        .prepare("DELETE FROM wl_woo_tax_rates WHERE id = ?")
        .bind(vec![Param::Integer(id)])
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

    fn rate_input(name: &str, country: &str, state: &str, bp: i64) -> TaxRateInput {
        TaxRateInput {
            name: Some(json!(name)),
            country: Some(json!(country)),
            state: Some(json!(state)),
            rate_bp: Some(json!(bp)),
        }
    }

    #[test]
    fn most_specific_rate_wins_and_tax_rounds_half_up() {
        let db = TestDb::new();
        block_on(create_tax_rate(&db, rate_input("Global", "", "", 500))).unwrap();
        block_on(create_tax_rate(&db, rate_input("Nigeria", "NG", "", 750))).unwrap();
        block_on(create_tax_rate(&db, rate_input("Lagos", "NG", "LA", 1000))).unwrap();
        let r = block_on(rate_for(&db, "NG", "LA")).unwrap().unwrap();
        assert_eq!(r.name, "Lagos");
        assert_eq!(r.rate_bp, 1000);
        let r = block_on(rate_for(&db, "NG", "OY")).unwrap().unwrap();
        assert_eq!(r.name, "Nigeria");
        let r = block_on(rate_for(&db, "GH", "")).unwrap().unwrap();
        assert_eq!(r.name, "Global");
        // 7.5% of 199,999 = 14,999.925 → 15,000 (half-up).
        assert_eq!(tax_on(199999, 750, false), 15000);
        // Inclusive: the tax inside a 107,500 price at 7.5% is 7,500.
        assert_eq!(tax_on(107500, 750, true), 7500);
        assert_eq!(tax_on(0, 750, false), 0);
    }

    #[test]
    fn settings_default_off() {
        let db = TestDb::new();
        assert!(!block_on(tax_enabled(&db)).unwrap());
        assert!(!block_on(prices_include_tax(&db)).unwrap());
        block_on(set_tax_settings(&db, true, false)).unwrap();
        assert!(block_on(tax_enabled(&db)).unwrap());
    }
}
