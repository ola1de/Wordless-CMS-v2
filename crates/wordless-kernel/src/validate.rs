//! Input validation helpers.
//! (Port of v1 `src/kernel/validate.ts` — same trimming, same error
//! messages, same bounds. Messages are part of the API surface: the
//! admin shows them, so they must match v1 byte for byte.)

use crate::error::{bad_request, WordlessError};

/// Require a non-empty string within length bounds (v1 `requireString`).
/// The value is trimmed; an empty-after-trim value is rejected.
pub fn require_string(
    value: &serde_json::Value,
    field: &str,
    min: usize,
    max: usize,
) -> Result<String, WordlessError> {
    let s = match value {
        serde_json::Value::String(s) => s.trim().to_string(),
        _ => {
            return Err(bad_request(&format!(
                "Field \"{field}\" is required and must be a non-empty string."
            )))
        }
    };
    if s.is_empty() {
        return Err(bad_request(&format!(
            "Field \"{field}\" is required and must be a non-empty string."
        )));
    }
    if s.len() < min {
        return Err(bad_request(&format!(
            "Field \"{field}\" must be at least {min} characters."
        )));
    }
    if s.len() > max {
        return Err(bad_request(&format!(
            "Field \"{field}\" must be at most {max} characters."
        )));
    }
    Ok(s)
}

pub fn require_email(value: &serde_json::Value, field: &str) -> Result<String, WordlessError> {
    let s = require_string(value, field, 0, 100)?;
    // Same shape as v1's EMAIL_RE: ^[^\s@]+@[^\s@]+\.[^\s@]+$
    let mut parts = s.split('@');
    let local = parts.next().unwrap_or("");
    let domain = parts.next().unwrap_or("");
    let valid = !local.is_empty()
        && !domain.is_empty()
        && parts.next().is_none()
        && !local.chars().any(|c| c.is_whitespace())
        && !domain.chars().any(|c| c.is_whitespace())
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.');
    if !valid {
        return Err(bad_request(&format!(
            "Field \"{field}\" must be a valid email address."
        )));
    }
    Ok(s.to_lowercase())
}

/// Parse an integer query parameter with bounds (v1 `intInRange`).
/// Absent or empty returns `default`.
pub fn int_in_range(
    raw: Option<&str>,
    field: &str,
    min: i64,
    max: i64,
    default: i64,
) -> Result<i64, WordlessError> {
    let Some(raw) = raw else { return Ok(default) };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(default);
    }
    let parsed: i64 = raw
        .parse()
        .map_err(|_| bad_request(&format!("Query parameter \"{field}\" must be an integer.")))?;
    if parsed < min || parsed > max {
        return Err(bad_request(&format!(
            "Query parameter \"{field}\" must be between {min} and {max}."
        )));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn string_trims_and_bounds() {
        assert_eq!(require_string(&json!("  hi  "), "f", 1, 5).unwrap(), "hi");
        assert!(require_string(&json!("   "), "f", 1, 5).is_err());
        assert!(require_string(&json!(""), "f", 1, 5).is_err());
        assert!(require_string(&json!("toolong"), "f", 1, 5).is_err());
        assert!(require_string(&json!(3), "f", 1, 5).is_err());
        // v1 message shape
        let err = require_string(&json!(""), "login", 1, 60).unwrap_err();
        assert_eq!(
            err.message,
            "Field \"login\" is required and must be a non-empty string."
        );
    }

    #[test]
    fn email_shape() {
        assert_eq!(require_email(&json!("A@B.Co"), "email").unwrap(), "a@b.co");
        assert!(require_email(&json!("nope"), "email").is_err());
        assert!(require_email(&json!("a@b"), "email").is_err());
        assert!(require_email(&json!("a@b@c.co"), "email").is_err());
    }

    #[test]
    fn int_range_defaults_and_rejects() {
        assert_eq!(int_in_range(None, "page", 1, 100, 1).unwrap(), 1);
        assert_eq!(int_in_range(Some(""), "page", 1, 100, 1).unwrap(), 1);
        assert_eq!(int_in_range(Some("3"), "page", 1, 100, 1).unwrap(), 3);
        assert!(int_in_range(Some("0"), "page", 1, 100, 1).is_err());
        assert!(int_in_range(Some("abc"), "page", 1, 100, 1).is_err());
        assert!(int_in_range(Some("3.5"), "page", 1, 100, 1).is_err());
    }
}
