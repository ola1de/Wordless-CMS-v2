//! Row-mapping helpers — the Rust equivalents of v1's `asText` /
//! `asInt` in `src/modules/users/service.ts`.
//!
//! D1 returns JS objects; numbers arrive as JSON numbers (f64), so
//! integer columns must be read through `as_f64` and truncated —
//! never assumed to be `i64` already.

use serde_json::Value;

use crate::Row;

/// The column as text; missing/null/non-string values become `""`.
/// (Numbers are stringified, as v1's `String(v)` does.)
pub fn row_text(row: &Row, key: &str) -> String {
    match row.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

/// The column as text, or `None` when it is missing or null.
pub fn row_opt_text(row: &Row, key: &str) -> Option<String> {
    match row.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::Bool(b)) => Some(b.to_string()),
        _ => None,
    }
}

/// The column as an integer; missing/null/non-numeric values become 0.
pub fn row_int(row: &Row, key: &str) -> i64 {
    match row.get(key) {
        Some(Value::Number(n)) => n
            .as_i64()
            .unwrap_or_else(|| n.as_f64().map(|f| f as i64).unwrap_or(0)),
        Some(Value::String(s)) => s.parse::<i64>().unwrap_or(0),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_d1_shaped_rows() {
        let row = json!({
            "ID": 7,
            "user_login": "admin",
            "revoked": 0,
            "missing": null,
            "float_col": 3.7,
        });
        assert_eq!(row_text(&row, "user_login"), "admin");
        assert_eq!(row_text(&row, "missing"), "");
        assert_eq!(row_text(&row, "absent"), "");
        assert_eq!(row_text(&row, "ID"), "7");
        assert_eq!(row_int(&row, "ID"), 7);
        assert_eq!(row_int(&row, "float_col"), 3);
        assert_eq!(row_int(&row, "missing"), 0);
        assert_eq!(row_opt_text(&row, "missing"), None);
        assert_eq!(row_opt_text(&row, "user_login").as_deref(), Some("admin"));
    }
}
