//! Integration config (port of v1 `config.ts`) — non-secret settings
//! only.
//!
//! Stored as `wp_options` `wl_integration_<id>` (JSON). **Secrets are
//! NEVER stored here:** any key that looks like a secret (`secret`,
//! `apiKey`, `token`, `password`, `private`, `passwd`, …) is rejected
//! with a 400 that tells the admin to use `wrangler secret put`
//! instead. This is enforced in code, not just in the docs — a config
//! write carrying a secret can never land in D1. Only the fields
//! listed in [`allowed_fields`] for an integration may be stored;
//! everything else is rejected too.

use serde_json::{Map, Value};
use wordless_db::{
    options::{get_option, set_option},
    Db,
};
use wordless_kernel::{
    error::{bad_request, not_found},
    WordlessError,
};

use crate::util::is_valid_http_url;

/// The 8 integration ids, in registry order (v1 `registry.ts`).
pub const INTEGRATION_IDS: [&str; 8] = [
    "email",
    "paystack",
    "flutterwave",
    "stripe",
    "analytics",
    "social",
    "cloudflare",
    "marketplace",
];

/// Display name for an integration id (v1 registry names).
pub fn integration_name(id: &str) -> Option<&'static str> {
    match id {
        "email" => Some("Email"),
        "paystack" => Some("Paystack"),
        "flutterwave" => Some("Flutterwave"),
        "stripe" => Some("Stripe"),
        "analytics" => Some("Analytics"),
        "social" => Some("Social sharing"),
        "cloudflare" => Some("Cloudflare"),
        "marketplace" => Some("Marketplace"),
        _ => None,
    }
}

/// True when `id` is one of the 8 known integrations.
pub fn is_known_integration(id: &str) -> bool {
    integration_name(id).is_some()
}

/// Fields each integration is allowed to store (everything else is
/// rejected). Port of v1 `ALLOWED_FIELDS`.
pub fn allowed_fields(id: &str) -> &'static [&'static str] {
    match id {
        "email" => &["from", "apiUrl"],
        "paystack" | "flutterwave" | "stripe" => &["mode", "currency"],
        "analytics" => &["provider", "measurementId"],
        _ => &[],
    }
}

/// Option key for an integration's config.
pub fn integration_option_key(id: &str) -> String {
    format!("wl_integration_{id}")
}

/// Port of v1 `SECRET_FIELD_RE` =
/// `/(secret|api[-_]?key|token|password|private|passwd)/i`, without
/// the regex crate: the key is lowercased and checked for the same
/// substrings.
pub fn looks_like_secret(key: &str) -> bool {
    let lower = key.to_lowercase();
    lower.contains("secret")
        || lower.contains("apikey")
        || lower.contains("api_key")
        || lower.contains("api-key")
        || lower.contains("token")
        || lower.contains("password")
        || lower.contains("private")
        || lower.contains("passwd")
}

/// Read an integration's stored config (non-secret fields only).
/// Unknown integration → 404.
///
/// # Errors
/// Returns `Err` only on a database failure.
pub async fn get_integration_config(
    db: &dyn Db,
    id: &str,
) -> Result<Map<String, Value>, WordlessError> {
    if !is_known_integration(id) {
        return Err(not_found(&format!("Integration \"{id}\" does not exist.")));
    }
    let value: Value = get_option(db, &integration_option_key(id), Value::Object(Map::new()))
        .await
        .map_err(WordlessError::from)?;
    Ok(value.as_object().cloned().unwrap_or_default())
}

/// Merge `patch` into an integration's stored config and return the
/// merged config. Rejects secret-looking fields, fields that are not
/// settings for the integration, and invalid values — with v1's
/// messages. Nothing is written when validation fails.
///
/// # Errors
/// * 404 when the integration does not exist.
/// * 400 on a secret-looking field, a disallowed field, or an invalid
///   value.
/// * 500 on a database failure.
pub async fn set_integration_config(
    db: &dyn Db,
    id: &str,
    patch: &Map<String, Value>,
) -> Result<Map<String, Value>, WordlessError> {
    let name = match integration_name(id) {
        Some(n) => n,
        None => return Err(not_found(&format!("Integration \"{id}\" does not exist."))),
    };
    let allowed = allowed_fields(id);
    for key in patch.keys() {
        if looks_like_secret(key) {
            return Err(bad_request(&format!(
                "Field \"{key}\" looks like a secret. Secrets are never stored in the database — set them with `wrangler secret put` (see docs/integrations/integrations.md)."
            )));
        }
        if !allowed.contains(&key.as_str()) {
            let allowed_list = if allowed.is_empty() {
                "(none)".to_string()
            } else {
                allowed.join(", ")
            };
            return Err(bad_request(&format!(
                "Field \"{key}\" is not a setting for the {name} integration. Allowed: {allowed_list}."
            )));
        }
    }
    // Per-integration value validation (v1 messages).
    if let Some(mode) = patch.get("mode") {
        if mode.as_str() != Some("test") && mode.as_str() != Some("live") {
            return Err(bad_request("Field \"mode\" must be \"test\" or \"live\"."));
        }
    }
    if let Some(currency) = patch.get("currency") {
        let ok = currency
            .as_str()
            .map(|s| s.len() == 3 && s.chars().all(|c| c.is_ascii_alphabetic()))
            .unwrap_or(false);
        if !ok {
            return Err(bad_request(
                "Field \"currency\" must be a 3-letter currency code.",
            ));
        }
    }
    if let Some(provider) = patch.get("provider") {
        let ok = matches!(
            provider.as_str(),
            Some("none") | Some("ga4") | Some("plausible")
        );
        if !ok {
            return Err(bad_request(
                "Field \"provider\" must be \"none\", \"ga4\" or \"plausible\".",
            ));
        }
    }
    if let Some(mid) = patch.get("measurementId") {
        if !mid.is_string() {
            return Err(bad_request("Field \"measurementId\" must be a string."));
        }
    }
    if let Some(from) = patch.get("from") {
        if !from.is_string() {
            return Err(bad_request("Field \"from\" must be a string."));
        }
    }
    if let Some(api_url) = patch.get("apiUrl") {
        match api_url.as_str() {
            None => {
                return Err(bad_request("Field \"apiUrl\" must be a string."));
            }
            Some(s) => {
                let trimmed = s.trim();
                if !trimmed.is_empty() && !is_valid_http_url(trimmed) {
                    return Err(bad_request(
                        "Field \"apiUrl\" must be a valid absolute http(s) URL or empty.",
                    ));
                }
            }
        }
    }
    let mut merged = get_integration_config(db, id).await?;
    for (k, v) in patch {
        merged.insert(k.clone(), v.clone());
    }
    set_option(
        db,
        &integration_option_key(id),
        &Value::Object(merged.clone()),
    )
    .await
    .map_err(WordlessError::from)?;
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{block_on, SqliteDb};
    use serde_json::json;

    fn patch(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn stores_and_returns_non_secret_settings() {
        let db = SqliteDb::new();
        let saved = block_on(set_integration_config(
            &db,
            "analytics",
            &patch(json!({ "provider": "ga4", "measurementId": "G-TEST123" })),
        ))
        .unwrap();
        assert_eq!(saved.get("provider"), Some(&json!("ga4")));
        assert_eq!(saved.get("measurementId"), Some(&json!("G-TEST123")));
        let read = block_on(get_integration_config(&db, "analytics")).unwrap();
        assert_eq!(read.get("provider"), Some(&json!("ga4")));
    }

    #[test]
    fn rejects_any_field_that_looks_like_a_secret() {
        let db = SqliteDb::new();
        for key in [
            "apiKey",
            "secret",
            "token",
            "password",
            "privateKey",
            "PAYSTACK_SECRET_KEY",
        ] {
            let mut p = Map::new();
            p.insert(key.to_string(), json!("sk_live_abc"));
            let err = block_on(set_integration_config(&db, "email", &p)).unwrap_err();
            assert_eq!(err.status, 400, "key {key}");
            assert!(
                err.message.contains("never stored in the database"),
                "{}",
                err.message
            );
        }
        // Nothing was written.
        assert!(block_on(get_integration_config(&db, "email"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn rejects_fields_that_are_not_settings() {
        let db = SqliteDb::new();
        let err = block_on(set_integration_config(
            &db,
            "social",
            &patch(json!({ "provider": "ga4" })),
        ))
        .unwrap_err();
        assert!(err.message.contains("not a setting"), "{}", err.message);
        let err = block_on(set_integration_config(
            &db,
            "email",
            &patch(json!({ "mode": "live" })),
        ))
        .unwrap_err();
        assert!(err.message.contains("not a setting"), "{}", err.message);
    }

    #[test]
    fn rejects_invalid_values() {
        let db = SqliteDb::new();
        let err = block_on(set_integration_config(
            &db,
            "paystack",
            &patch(json!({ "mode": "production" })),
        ))
        .unwrap_err();
        assert!(err.message.contains("test"), "{}", err.message);
        assert!(err.message.contains("live"), "{}", err.message);
        let err = block_on(set_integration_config(
            &db,
            "analytics",
            &patch(json!({ "provider": "mixpanel" })),
        ))
        .unwrap_err();
        assert!(err.message.contains("none"), "{}", err.message);
        assert!(err.message.contains("ga4"), "{}", err.message);
        assert!(err.message.contains("plausible"), "{}", err.message);
        let err = block_on(set_integration_config(
            &db,
            "email",
            &patch(json!({ "apiUrl": "not a url" })),
        ))
        .unwrap_err();
        assert!(err.message.contains("valid absolute"), "{}", err.message);
        // A valid absolute URL and an empty URL are both accepted.
        block_on(set_integration_config(
            &db,
            "email",
            &patch(json!({ "apiUrl": "https://api.example.com/send" })),
        ))
        .unwrap();
        block_on(set_integration_config(
            &db,
            "email",
            &patch(json!({ "apiUrl": "" })),
        ))
        .unwrap();
    }

    #[test]
    fn unknown_integration_is_404() {
        let db = SqliteDb::new();
        let err = block_on(get_integration_config(&db, "nope")).unwrap_err();
        assert_eq!(err.status, 404);
        assert!(err.message.contains("does not exist"), "{}", err.message);
        let err = block_on(set_integration_config(&db, "nope", &patch(json!({})))).unwrap_err();
        assert_eq!(err.status, 404);
    }
}
