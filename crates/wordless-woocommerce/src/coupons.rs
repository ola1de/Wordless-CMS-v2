//! Coupons (P8) — `wl_woo_coupons`.
//!
//! A coupon is `fixed` (a minor-unit amount off) or `percent`
//! (basis points off the subtotal). Rules: `active`, not expired,
//! `usage_count < max_uses` (0 = unlimited), subtotal ≥
//! `min_order_minor`. Validation is pure (`validate_coupon`); the
//! usage count increments exactly once with the paid transition in
//! `wordless-commerce` (`apply_coupon_use`) — never at apply time, so
//! a failed checkout never burns a use. The discount never exceeds
//! the subtotal.

use serde_json::Value;
use wordless_commerce::money::require_minor_units;
use wordless_commerce::validate::require_int;
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, not_found, WordlessError};
use wordless_kernel::time::now_iso;
use wordless_kernel::validate::require_string;

use crate::types::{Coupon, CouponInput};

pub(crate) fn row_to_coupon(row: &Row) -> Coupon {
    Coupon {
        id: row_int(row, "id"),
        code: row_text(row, "code"),
        description: row_text(row, "description"),
        r#type: row_text(row, "type"),
        amount_minor: row_int(row, "amount_minor"),
        percent_bp: row_int(row, "percent_bp"),
        min_order_minor: row_int(row, "min_order_minor"),
        max_uses: row_int(row, "max_uses"),
        usage_count: row_int(row, "usage_count"),
        expires_at: match row_text(row, "expires_at") {
            s if s.is_empty() => None,
            s => Some(s),
        },
        active: row_int(row, "active") == 1,
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

/// What validating a coupon code against a subtotal found.
#[derive(Debug, Clone, PartialEq)]
pub struct CouponValidation {
    /// The coupon (None when the code is unknown).
    pub coupon: Option<Coupon>,
    /// The discount in minor units (0 when invalid).
    pub discount_minor: i64,
    /// Plain-language reason when invalid (None when valid).
    pub reason: Option<String>,
}

fn require_coupon_type(value: &Value) -> Result<String, WordlessError> {
    match value {
        Value::Null => Ok("fixed".to_string()),
        Value::String(s) if s == "fixed" || s == "percent" => Ok(s.clone()),
        _ => Err(bad_request(
            "Field \"type\" must be \"fixed\" or \"percent\".",
        )),
    }
}

fn require_code(value: &Value) -> Result<String, WordlessError> {
    let code = require_string(value, "code", 1, 50)?;
    let code = code.trim().to_ascii_uppercase();
    if code
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Ok(code)
    } else {
        Err(bad_request(
            "Field \"code\" may only contain letters, digits, hyphens and underscores.",
        ))
    }
}

fn require_expires_at(value: &Value) -> Result<Option<String>, WordlessError> {
    match value {
        Value::Null => Ok(None),
        Value::String(s) if s.trim().is_empty() => Ok(None),
        Value::String(s) => Ok(Some(s.trim().to_string())),
        _ => Err(bad_request(
            "Field \"expiresAt\" must be an ISO-8601 date string.",
        )),
    }
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

/// Fetch a coupon by id, or `None` when unknown.
pub async fn get_coupon(db: &dyn Db, id: i64) -> Result<Option<Coupon>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_woo_coupons WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(id)])
        .first()
        .await?;
    Ok(row.as_ref().map(row_to_coupon))
}

/// Fetch a coupon by code (case-insensitive — codes are stored
/// upper-case), or `None` when unknown.
pub async fn get_coupon_by_code(db: &dyn Db, code: &str) -> Result<Option<Coupon>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_woo_coupons WHERE code = ? LIMIT 1")
        .bind(vec![Param::Text(code.trim().to_ascii_uppercase())])
        .first()
        .await?;
    Ok(row.map(|r| row_to_coupon(&r)))
}

/// Validate a coupon code against a subtotal. Never errors on an
/// unknown or unusable code — it reports `reason` instead, so the
/// caller (quote vs checkout) can decide: the quote reports it, the
/// checkout refuses with a 400.
pub async fn validate_coupon(
    db: &dyn Db,
    code: &str,
    subtotal_minor: i64,
    currency: &str,
) -> Result<CouponValidation, WordlessError> {
    let code = code.trim();
    if code.is_empty() {
        return Ok(CouponValidation {
            coupon: None,
            discount_minor: 0,
            reason: None,
        });
    }
    let Some(coupon) = get_coupon_by_code(db, code).await? else {
        return Ok(CouponValidation {
            coupon: None,
            discount_minor: 0,
            reason: Some(format!("Coupon \"{code}\" does not exist.")),
        });
    };
    if !coupon.active {
        return Ok(CouponValidation {
            coupon: Some(coupon),
            discount_minor: 0,
            reason: Some(format!("Coupon \"{code}\" is not active.")),
        });
    }
    let mut reason: Option<String> = None;
    if let Some(expires) = coupon.expires_at.clone() {
        if expires < now_iso() {
            reason = Some(format!("Coupon \"{code}\" expired on {expires}."));
        }
    }
    if reason.is_none() && coupon.max_uses > 0 && coupon.usage_count >= coupon.max_uses {
        reason = Some(format!("Coupon \"{code}\" has reached its usage limit."));
    }
    if reason.is_none() && subtotal_minor < coupon.min_order_minor {
        reason = Some(format!(
            "Coupon \"{code}\" needs a minimum order of {}.",
            wordless_commerce::money::format_money(coupon.min_order_minor, currency)
        ));
    }
    let discount = if reason.is_some() {
        0
    } else {
        match coupon.r#type.as_str() {
            "percent" => (subtotal_minor * coupon.percent_bp + 5000) / 10000,
            _ => coupon.amount_minor,
        }
        .min(subtotal_minor)
        .max(0)
    };
    Ok(CouponValidation {
        coupon: Some(coupon),
        discount_minor: discount,
        reason,
    })
}

/// All coupons, newest first.
pub async fn list_coupons(db: &dyn Db) -> Result<Vec<Coupon>, WordlessError> {
    let rows = db
        .prepare("SELECT * FROM wl_woo_coupons ORDER BY id DESC")
        .bind(vec![])
        .all::<Row>()
        .await?;
    Ok(rows.iter().map(row_to_coupon).collect())
}

/// Create a coupon. Codes are stored upper-case and unique.
pub async fn create_coupon(db: &dyn Db, input: CouponInput) -> Result<Coupon, WordlessError> {
    let code = require_code(input.code.as_ref().unwrap_or(&Value::Null))?;
    let r#type = require_coupon_type(input.r#type.as_ref().unwrap_or(&Value::Null))?;
    let amount_minor = match input.amount_minor.as_ref() {
        Some(v) => require_minor_units(v, "amountMinor", 0)?,
        None => 0,
    };
    let percent_bp = match input.percent_bp.as_ref() {
        Some(v) => require_int(v, "percentBp", 0, 10000)?,
        None => 0,
    };
    if r#type == "percent" && percent_bp == 0 {
        return Err(bad_request("A percent coupon needs \"percentBp\" above 0."));
    }
    if r#type == "fixed" && amount_minor == 0 {
        return Err(bad_request("A fixed coupon needs \"amountMinor\" above 0."));
    }
    let min_order_minor = match input.min_order_minor.as_ref() {
        Some(v) => require_minor_units(v, "minOrderMinor", 0)?,
        None => 0,
    };
    let max_uses = match input.max_uses.as_ref() {
        Some(v) => require_int(v, "maxUses", 0, i64::MAX)?,
        None => 0,
    };
    let description = match input.description.as_ref() {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    let expires_at = require_expires_at(input.expires_at.as_ref().unwrap_or(&Value::Null))?;
    let active = require_bool(
        input.active.as_ref().unwrap_or(&Value::Null),
        "active",
        true,
    )?;
    let now = now_iso();
    let meta = db
        .prepare(
            "INSERT INTO wl_woo_coupons (code, description, type, amount_minor, percent_bp, min_order_minor, max_uses, usage_count, expires_at, active, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?, ?)",
        )
        .bind(vec![
            Param::Text(code),
            Param::Text(description),
            Param::Text(r#type),
            Param::Integer(amount_minor),
            Param::Integer(percent_bp),
            Param::Integer(min_order_minor),
            Param::Integer(max_uses),
            match &expires_at {
                Some(e) => Param::Text(e.clone()),
                None => Param::Null,
            },
            Param::Integer(active as i64),
            Param::Text(now.clone()),
            Param::Text(now),
        ])
        .run()
        .await;
    let meta = match meta {
        Ok(m) => m,
        Err(e) if e.to_string().contains("UNIQUE") => {
            return Err(bad_request("A coupon with that code already exists."))
        }
        Err(e) => return Err(e.into()),
    };
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_woo_coupons WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(meta.last_row_id.unwrap_or(0))])
        .first()
        .await?;
    row.map(|r| row_to_coupon(&r))
        .ok_or_else(|| WordlessError::new("internal_error", 500, "Coupon was not created."))
}

/// Update a coupon (partial: absent fields keep their value). The
/// usage count is never written here — it only moves in
/// `apply_coupon_use` at the paid transition.
pub async fn update_coupon(
    db: &dyn Db,
    id: i64,
    input: CouponInput,
) -> Result<Coupon, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_woo_coupons WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(id)])
        .first()
        .await?;
    let Some(row) = row else {
        return Err(not_found(&format!("Coupon {id} not found.")));
    };
    let existing = row_to_coupon(&row);
    let code = match input.code.as_ref() {
        Some(v) if !v.is_null() => require_code(v)?,
        _ => existing.code.clone(),
    };
    let r#type = match input.r#type.as_ref() {
        Some(v) if !v.is_null() => require_coupon_type(v)?,
        _ => existing.r#type.clone(),
    };
    let amount_minor = match input.amount_minor.as_ref() {
        Some(v) if !v.is_null() => require_minor_units(v, "amountMinor", 0)?,
        _ => existing.amount_minor,
    };
    let percent_bp = match input.percent_bp.as_ref() {
        Some(v) if !v.is_null() => require_int(v, "percentBp", 0, 10000)?,
        _ => existing.percent_bp,
    };
    if r#type == "percent" && percent_bp == 0 {
        return Err(bad_request("A percent coupon needs \"percentBp\" above 0."));
    }
    if r#type == "fixed" && amount_minor == 0 {
        return Err(bad_request("A fixed coupon needs \"amountMinor\" above 0."));
    }
    let min_order_minor = match input.min_order_minor.as_ref() {
        Some(v) if !v.is_null() => require_minor_units(v, "minOrderMinor", 0)?,
        _ => existing.min_order_minor,
    };
    let max_uses = match input.max_uses.as_ref() {
        Some(v) if !v.is_null() => require_int(v, "maxUses", 0, i64::MAX)?,
        _ => existing.max_uses,
    };
    let description = match input.description.as_ref() {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => existing.description.clone(),
        _ => return Err(bad_request("Field \"description\" must be a string.")),
    };
    let expires_at = match input.expires_at.as_ref() {
        Some(v) => require_expires_at(v)?,
        None => existing.expires_at.clone(),
    };
    let active = require_bool(
        input.active.as_ref().unwrap_or(&Value::Null),
        "active",
        existing.active,
    )?;
    let meta = db
        .prepare(
            "UPDATE wl_woo_coupons SET code = ?, description = ?, type = ?, amount_minor = ?, percent_bp = ?, min_order_minor = ?, max_uses = ?, expires_at = ?, active = ?, updated_at = ? WHERE id = ?",
        )
        .bind(vec![
            Param::Text(code),
            Param::Text(description),
            Param::Text(r#type),
            Param::Integer(amount_minor),
            Param::Integer(percent_bp),
            Param::Integer(min_order_minor),
            Param::Integer(max_uses),
            match &expires_at {
                Some(e) => Param::Text(e.clone()),
                None => Param::Null,
            },
            Param::Integer(active as i64),
            Param::Text(now_iso()),
            Param::Integer(id),
        ])
        .run()
        .await;
    if let Err(e) = meta {
        if e.to_string().contains("UNIQUE") {
            return Err(bad_request("A coupon with that code already exists."));
        }
        return Err(e.into());
    }
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_woo_coupons WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(id)])
        .first()
        .await?;
    row.map(|r| row_to_coupon(&r))
        .ok_or_else(|| WordlessError::new("internal_error", 500, "Coupon was not updated."))
}

/// Delete a coupon. A coupon already used by an order stays in the
/// order's snapshot (`coupon_code` on `wl_orders`), so deleting the
/// coupon never rewrites order history.
pub async fn delete_coupon(db: &dyn Db, id: i64) -> Result<bool, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT id FROM wl_woo_coupons WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(id)])
        .first()
        .await?;
    if row.is_none() {
        return Err(not_found(&format!("Coupon {id} not found.")));
    }
    let meta = db
        .prepare("DELETE FROM wl_woo_coupons WHERE id = ?")
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

    fn coupon_input(code: &str) -> CouponInput {
        CouponInput {
            code: Some(json!(code)),
            amount_minor: Some(json!(50000)),
            ..Default::default()
        }
    }

    #[test]
    fn validates_fixed_and_percent_coupons_and_caps_at_subtotal() {
        let db = TestDb::new();
        block_on(create_coupon(&db, coupon_input("SAVE50"))).unwrap();
        block_on(create_coupon(
            &db,
            CouponInput {
                code: Some(json!("TENPCT")),
                r#type: Some(json!("percent")),
                percent_bp: Some(json!(1000)),
                ..Default::default()
            },
        ))
        .unwrap();
        let v = block_on(validate_coupon(&db, "save50", 200000, "NGN")).unwrap();
        assert_eq!(v.discount_minor, 50000);
        assert!(v.reason.is_none());
        let v = block_on(validate_coupon(&db, "TENPCT", 200000, "NGN")).unwrap();
        assert_eq!(v.discount_minor, 20000); // 10% of 200,000
                                             // Fixed discount never exceeds the subtotal.
        let v = block_on(validate_coupon(&db, "SAVE50", 30000, "NGN")).unwrap();
        assert_eq!(v.discount_minor, 30000);
        // Unknown code reports a reason instead of erroring.
        let v = block_on(validate_coupon(&db, "NOPE", 200000, "NGN")).unwrap();
        assert_eq!(v.discount_minor, 0);
        assert!(v.reason.is_some());
    }

    #[test]
    fn enforces_expiry_usage_limit_and_minimum_order() {
        let db = TestDb::new();
        block_on(create_coupon(
            &db,
            CouponInput {
                code: Some(json!("OLD")),
                amount_minor: Some(json!(1000)),
                expires_at: Some(json!("2020-01-01T00:00:00.000Z")),
                ..Default::default()
            },
        ))
        .unwrap();
        let v = block_on(validate_coupon(&db, "OLD", 100000, "NGN")).unwrap();
        assert!(v.reason.as_deref().unwrap_or("").contains("expired"));
        block_on(create_coupon(
            &db,
            CouponInput {
                code: Some(json!("ONCE")),
                amount_minor: Some(json!(1000)),
                max_uses: Some(json!(1)),
                ..Default::default()
            },
        ))
        .unwrap();
        // Simulate one use, then the coupon is spent.
        db.exec_batch_sql("UPDATE wl_woo_coupons SET usage_count = 1 WHERE code = 'ONCE'");
        let v = block_on(validate_coupon(&db, "ONCE", 100000, "NGN")).unwrap();
        assert!(v.reason.as_deref().unwrap_or("").contains("usage limit"));
        block_on(create_coupon(
            &db,
            CouponInput {
                code: Some(json!("BIG")),
                amount_minor: Some(json!(1000)),
                min_order_minor: Some(json!(500000)),
                ..Default::default()
            },
        ))
        .unwrap();
        let v = block_on(validate_coupon(&db, "BIG", 100000, "NGN")).unwrap();
        assert!(v.reason.as_deref().unwrap_or("").contains("minimum order"));
    }
}
