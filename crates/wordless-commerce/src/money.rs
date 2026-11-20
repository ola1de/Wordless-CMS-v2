//! Money (port of v1 `src/modules/commerce/money.ts`) — integer
//! minor units only.
//!
//! Prices are stored as INTEGER kobo/cents and never as float, the same
//! rule as LIFEOS money handling: a float price is a rounding bug
//! waiting to happen. All formatting goes through here so every surface
//! (admin, shop pages, order emails, agent reports) shows the same
//! number.

use serde_json::Value;
use wordless_kernel::error::{bad_request, WordlessError};

/// Currency symbol table (v1 `CURRENCY_SYMBOLS`).
fn symbol_for(currency: &str) -> Option<&'static str> {
    match currency {
        "NGN" => Some("₦"),
        "USD" => Some("$"),
        "EUR" => Some("€"),
        "GBP" => Some("£"),
        "GHS" => Some("₵"),
        "KES" => Some("KSh"),
        "ZAR" => Some("R"),
        _ => None,
    }
}

/// A 3-letter uppercase currency code (NGN, USD, …).
/// Port of v1 `requireCurrency`.
pub fn require_currency(value: &Value, field: &str) -> Result<String, WordlessError> {
    let s = match value {
        Value::String(s) => s.trim().to_string(),
        _ => {
            return Err(bad_request(&format!(
                "Field \"{field}\" must be a 3-letter currency code (e.g. NGN)."
            )))
        }
    };
    let ok = s.len() == 3 && s.chars().all(|c| c.is_ascii_alphabetic());
    if !ok {
        return Err(bad_request(&format!(
            "Field \"{field}\" must be a 3-letter currency code (e.g. NGN)."
        )));
    }
    Ok(s.to_ascii_uppercase())
}

/// An integer amount in minor units (kobo/cents). Floats are rejected.
/// Port of v1 `requireMinorUnits`.
pub fn require_minor_units(value: &Value, field: &str, min: i64) -> Result<i64, WordlessError> {
    let n = match value {
        Value::Number(n) => n.as_i64().ok_or_else(|| {
            bad_request(&format!(
                "Field \"{field}\" must be an integer amount in minor units (kobo/cents), never a decimal."
            ))
        })?,
        _ => {
            return Err(bad_request(&format!(
                "Field \"{field}\" must be an integer amount in minor units (kobo/cents), never a decimal."
            )))
        }
    };
    if n < min {
        return Err(bad_request(&format!(
            "Field \"{field}\" must be at least {min}."
        )));
    }
    Ok(n)
}

/// `125000` minor NGN → `"₦1,250.00"`. Unknown currency →
/// `"XXX 1,250.00"`. Port of v1 `formatMoney`.
pub fn format_money(minor: i64, currency: &str) -> String {
    let major = minor / 100;
    let cents = (minor % 100).abs();
    let sign = if minor < 0 { "-" } else { "" };
    let grouped = group_thousands(major.abs());
    let amount = format!("{sign}{grouped}.{cents:02}");
    match symbol_for(&currency.to_ascii_uppercase()) {
        Some(symbol) => format!("{symbol}{amount}"),
        None => format!("{} {amount}", currency.to_ascii_uppercase()),
    }
}

fn group_thousands(n: i64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Major units (e.g. 1250.00) → minor units, rounding half-up. Only used
/// at provider boundaries (Flutterwave takes major units).
/// Port of v1 `toMinorUnits`.
pub fn to_minor_units(major: f64) -> i64 {
    (major * 100.0).round() as i64
}

/// Minor units → major units (e.g. 125000 → 1250). Only used at provider
/// boundaries. Port of v1 `toMajorUnits`.
pub fn to_major_units(minor: i64) -> f64 {
    minor as f64 / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn formats_minor_units_with_symbol_and_grouping() {
        assert_eq!(format_money(125000, "NGN"), "₦1,250.00");
        assert_eq!(format_money(999, "USD"), "$9.99");
        assert_eq!(format_money(0, "EUR"), "€0.00");
        assert_eq!(format_money(100, "XXX"), "XXX 1.00");
        assert_eq!(format_money(-125050, "NGN"), "₦-1,250.50"); // v1: sign sits inside, after the symbol
    }

    #[test]
    fn rejects_fractional_prices() {
        assert!(require_minor_units(&json!(12.5), "priceMinor", 0)
            .unwrap_err()
            .message
            .contains("integer"));
        assert!(require_minor_units(&json!("1250"), "priceMinor", 0)
            .unwrap_err()
            .message
            .contains("integer"));
        assert_eq!(
            require_minor_units(&json!(1250), "priceMinor", 0).unwrap(),
            1250
        );
        assert!(require_minor_units(&json!(-1), "priceMinor", 0)
            .unwrap_err()
            .message
            .contains("at least 0"));
    }

    #[test]
    fn converts_minor_major_without_float_drift() {
        assert_eq!(to_minor_units(1250.0), 125000);
        assert_eq!(to_major_units(125000), 1250.0);
    }

    #[test]
    fn currency_validation_matches_v1() {
        assert_eq!(require_currency(&json!("ngn"), "currency").unwrap(), "NGN");
        assert!(require_currency(&json!("Naira"), "currency").is_err());
        assert!(require_currency(&json!("NG"), "currency").is_err());
    }
}
