//! Input validation helpers (port of v1 `src/kernel/validate.ts`
//! `requireInt` / `requireSlug` and `src/modules/posts/service.ts`
//! `slugify`). Messages match v1 byte for byte — they are part of the
//! API surface.

use serde_json::Value;
use unicode_normalization::UnicodeNormalization;
use wordless_kernel::error::{bad_request, WordlessError};
use wordless_kernel::validate::require_string;

/// Require an integer within bounds. Port of v1 `requireInt`.
pub fn require_int(value: &Value, field: &str, min: i64, max: i64) -> Result<i64, WordlessError> {
    let n = match value {
        Value::Number(n) => n
            .as_i64()
            .ok_or_else(|| bad_request(&format!("Field \"{field}\" must be an integer.")))?,
        _ => {
            return Err(bad_request(&format!(
                "Field \"{field}\" must be an integer."
            )))
        }
    };
    if n < min || n > max {
        return Err(bad_request(&format!(
            "Field \"{field}\" must be between {min} and {max}."
        )));
    }
    Ok(n)
}

/// Require a URL slug (lowercase letters, digits and hyphens only).
/// The value is lowercased first, as in v1 `requireSlug`.
pub fn require_slug(value: &Value, field: &str) -> Result<String, WordlessError> {
    let s = require_string(value, field, 1, 200)?.to_lowercase();
    if s.is_empty()
        || !s
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(bad_request(&format!(
            "Field \"{field}\" must be a URL slug (lowercase letters, digits and hyphens only)."
        )));
    }
    Ok(s)
}

/// `"Hello, World!"` -> `"hello-world"`; `"Café & Bar"` -> `"cafe-bar"`;
/// `""` -> `"post"`. Port of v1 `slugify` (`posts/service.ts`): NFD
/// decomposition, strip combining marks (U+0300–U+036F), lowercase,
/// non-alphanumerics to `-`, trim leading/trailing hyphens, truncate
/// to 200 chars. (Same implementation as `wordless-cms` `slugify`;
/// the empty-input fallback is v1's `"post"`.)
pub fn slugify(text: &str) -> String {
    let decomposed: String = text.nfd().collect();
    let mut slug = String::new();
    for ch in decomposed.chars() {
        if ('\u{0300}'..='\u{036f}').contains(&ch) {
            continue;
        }
        let lower = ch.to_lowercase().next().unwrap_or(ch);
        if lower.is_ascii_alphanumeric() {
            slug.push(lower);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        return "post".to_string();
    }
    slug.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn slugify_shapes() {
        assert_eq!(slugify("Hello, World!"), "hello-world");
        assert_eq!(slugify("  Café & Bar  "), "cafe-bar");
        assert_eq!(slugify("Pack Hoodie"), "pack-hoodie");
        assert_eq!(slugify(""), "post");
    }

    #[test]
    fn int_bounds_match_v1() {
        assert_eq!(require_int(&json!(5), "stock", -1, 100).unwrap(), 5);
        assert!(require_int(&json!(5.5), "stock", -1, 100).is_err());
        assert!(require_int(&json!("5"), "stock", -1, 100).is_err());
        let err = require_int(&json!(101), "stock", -1, 100).unwrap_err();
        assert_eq!(err.message, "Field \"stock\" must be between -1 and 100.");
    }

    #[test]
    fn slug_validation_matches_v1() {
        assert_eq!(
            require_slug(&json!("Pack-Hoodie"), "slug").unwrap(),
            "pack-hoodie"
        );
        assert!(require_slug(&json!("has space"), "slug").is_err());
        assert!(require_slug(&json!(""), "slug").is_err());
    }
}
