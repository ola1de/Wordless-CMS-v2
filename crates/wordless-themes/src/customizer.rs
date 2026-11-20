//! Customizer (P2) — port of the variations + preview half of v1
//! `src/modules/themes/service.ts` (+ `types.ts`).
//!
//! Variations are partial theme.json `settings` objects, stored as
//! options (`wl_theme_variation_<slug>`) and merged over the theme's
//! own settings when the stylesheet is generated. Preview tokens are
//! the `wl_theme_preview` option (30-minute expiry): an unsaved
//! variation override for one render, resolved by `resolve_variation`
//! (the `/theme.css` wiring helper — v1 `resolveVariation` in
//! `src/index.ts`).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wordless_db::Db;
use wordless_kernel::crypto::random_bytes;
use wordless_kernel::error::{bad_request, WordlessError};
use wordless_kernel::time::{iso_from_ms, ms_from_iso, now_iso, now_ms};

use crate::registry::{delete_option, get_option_json, get_theme, set_option_json};
use crate::themejson::validate_theme_settings;

/// Option key holding the current preview token.
pub const PREVIEW_OPTION: &str = "wl_theme_preview";
/// Preview token lifetime: 30 minutes.
pub const PREVIEW_TTL_MS: u64 = 30 * 60 * 1000;

/// A customizer variation: a partial theme.json `settings` object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemeVariation {
    pub name: String,
    /// Partial settings, merged over the theme's own settings.
    pub settings: Value,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

/// A short-lived preview token: unsaved variation overrides for one render.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemePreview {
    pub token: String,
    pub slug: String,
    pub settings: Value,
    #[serde(rename = "expiresAt")]
    pub expires_at: String,
}

/// Option key for a theme's saved variation.
pub fn variation_option_key(slug: &str) -> String {
    format!("wl_theme_variation_{slug}")
}

/// Get a theme's saved variation, or `None` when there is none.
pub async fn get_variation(
    db: &dyn Db,
    site_id: i64,
    slug: &str,
) -> Result<Option<ThemeVariation>, WordlessError> {
    // Throws when the theme is not installed, same as v1.
    get_theme(db, slug).await?;
    match get_option_json(db, site_id, &variation_option_key(slug)).await? {
        Some(value) => Ok(serde_json::from_value(value).ok()),
        None => Ok(None),
    }
}

/// Remove a saved variation (the theme renders with its own theme.json
/// again). Returns true when a variation was stored.
pub async fn delete_variation(db: &dyn Db, site_id: i64, slug: &str) -> Result<bool, WordlessError> {
    get_theme(db, slug).await?;
    delete_option(db, site_id, &variation_option_key(slug)).await
}

/// Save a variation for a block theme. Validates the partial settings
/// with the same rules as theme.json itself.
pub async fn save_variation(
    db: &dyn Db,
    site_id: i64,
    slug: &str,
    name: Option<&str>,
    settings: &Value,
) -> Result<ThemeVariation, WordlessError> {
    let theme = get_theme(db, slug).await?;
    if theme.kind != "block" {
        return Err(bad_request(
            "Variations apply to block themes only (classic themes have no theme.json).",
        ));
    }
    if !settings.is_object() {
        return Err(bad_request("A variation needs a settings object."));
    }
    let errors = validate_theme_settings(settings, 3);
    if !errors.is_empty() {
        let detail = errors
            .iter()
            .map(|e| format!("{}: {}", e.path, e.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(bad_request(&format!(
            "Variation settings are invalid — {detail}"
        )));
    }
    let variation = ThemeVariation {
        name: match name {
            Some(n) if !n.trim().is_empty() => n.trim().to_string(),
            _ => "Custom".to_string(),
        },
        settings: settings.clone(),
        updated_at: now_iso(),
    };
    set_option_json(
        db,
        site_id,
        &variation_option_key(slug),
        &serde_json::to_value(&variation)
            .map_err(|e| WordlessError::new("internal_error", 500, &e.to_string()))?,
    )
    .await?;
    Ok(variation)
}

/// Create a preview token for a block theme's unsaved variation
/// overrides. The token is 16 random bytes as 32 lowercase hex chars
/// (same shape as v1's `crypto.getRandomValues` token).
pub async fn create_preview(
    db: &dyn Db,
    site_id: i64,
    slug: &str,
    settings: &Value,
) -> Result<ThemePreview, WordlessError> {
    let theme = get_theme(db, slug).await?;
    if theme.kind != "block" {
        return Err(bad_request(
            "Previews apply to block themes only (classic themes have no theme.json).",
        ));
    }
    let errors = validate_theme_settings(settings, 3);
    if !errors.is_empty() {
        let detail = errors
            .iter()
            .map(|e| format!("{}: {}", e.path, e.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(bad_request(&format!(
            "Preview settings are invalid — {detail}"
        )));
    }
    let bytes =
        random_bytes(16).map_err(|e| WordlessError::new("internal_error", 500, &e.to_string()))?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let preview = ThemePreview {
        token,
        slug: slug.to_string(),
        settings: settings.clone(),
        expires_at: iso_from_ms(now_ms() + PREVIEW_TTL_MS),
    };
    set_option_json(
        db,
        site_id,
        PREVIEW_OPTION,
        &serde_json::to_value(&preview)
            .map_err(|e| WordlessError::new("internal_error", 500, &e.to_string()))?,
    )
    .await?;
    Ok(preview)
}

/// Resolve a preview token to its (slug, settings), or `None` when
/// missing or expired.
pub async fn get_preview(
    db: &dyn Db,
    site_id: i64,
    token: &str,
) -> Result<Option<(String, Value)>, WordlessError> {
    if token.is_empty() {
        return Ok(None);
    }
    let Some(value) = get_option_json(db, site_id, PREVIEW_OPTION).await? else {
        return Ok(None);
    };
    let Ok(preview) = serde_json::from_value::<ThemePreview>(value) else {
        return Ok(None);
    };
    if preview.token != token {
        return Ok(None);
    }
    let expired = ms_from_iso(&preview.expires_at)
        .map(|exp| exp <= now_ms())
        .unwrap_or(true);
    if expired {
        return Ok(None);
    }
    Ok(Some((preview.slug, preview.settings)))
}

/// Resolve the variation overrides for a render, the way v1's
/// `resolveVariation` does for `/theme.css`: a `?wl_preview=<token>`
/// preview token wins when it resolves; otherwise the active theme's
/// saved variation applies; otherwise there is none.
pub async fn resolve_variation(
    db: &dyn Db,
    site_id: i64,
    preview_token: Option<&str>,
) -> Result<Option<Value>, WordlessError> {
    if let Some(token) = preview_token {
        if !token.is_empty() {
            if let Some((_slug, settings)) = get_preview(db, site_id, token).await? {
                return Ok(Some(settings));
            }
        }
    }
    let slug = crate::registry::get_active_theme_slug(db, site_id).await?;
    match get_variation(db, site_id, &slug).await? {
        Some(variation) => Ok(Some(variation.settings)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::DEFAULT_THEME_SLUG;
    use crate::test_support::{FakeDb, FakeStore};
    use futures_executor::block_on;
    use serde_json::json;

    #[test]
    fn saves_loads_and_applies_a_variation() {
        let db = FakeDb::new();
        let store = FakeStore::new();
        let variation = block_on(save_variation(
            &db, 1,
            "wordless-default",
            Some("Red"),
            &json!({ "color": { "palette": [{ "slug": "primary", "color": "#FF0000", "name": "Primary" }] } }),
        ))
        .unwrap();
        assert_eq!(variation.name, "Red");
        assert_eq!(
            block_on(get_variation(&db, 1, "wordless-default"))
                .unwrap()
                .unwrap(),
            variation
        );
        let loaded = block_on(crate::registry::load_active_theme(
            &db, 1,
            &store,
            Some(&variation.settings),
        ))
        .unwrap();
        assert!(
            loaded
                .stylesheet
                .contains("--wp--preset--color--primary: #FF0000;"),
            "{}",
            loaded.stylesheet
        );
        assert!(
            loaded
                .stylesheet
                .contains("--wp--preset--color--base: #FFFFFF;"),
            "{}",
            loaded.stylesheet
        );
        // The stylesheet hash must change with the variation, or the live
        // site's /theme.css?v=<hash> link would keep serving cached CSS.
        let unvaried = block_on(crate::registry::load_active_theme(&db, 1, &store, None)).unwrap();
        assert_ne!(loaded.stylesheet_hash, unvaried.stylesheet_hash);
    }

    #[test]
    fn rejects_invalid_variation() {
        let db = FakeDb::new();
        let err = block_on(save_variation(
            &db, 1,
            "wordless-default",
            None,
            &json!({ "color": { "palette": [{ "slug": "x", "name": "X" }] } }),
        ))
        .unwrap_err();
        assert!(err.message.contains("invalid"), "{}", err.message);
    }

    #[test]
    fn rejects_variation_on_classic_theme() {
        let db = FakeDb::new();
        let store = FakeStore::new();
        // Import a classic theme first (reuse the registry import).
        let zip = {
            use std::io::Write;
            use zip::write::{SimpleFileOptions, ZipWriter};
            let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
            let opts =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            writer.start_file("style.css", opts).unwrap();
            writer
                .write_all(b"/*\nTheme Name: Classic Sample\nVersion: 1.0.0\nText Domain: classic-sample\n*/\nbody { background: #fff; }\n")
                .unwrap();
            writer.start_file("index.php", opts).unwrap();
            writer
                .write_all(b"<?php\nget_header();\nthe_content();\nget_footer();\n")
                .unwrap();
            writer.finish().unwrap().into_inner()
        };
        block_on(crate::registry::import_classic_theme(
            &db,
            &store,
            "classic-sample.zip",
            &zip,
            Some(&crate::test_converter::TestConverter),
        ))
        .unwrap();
        let err = block_on(save_variation(&db, 1, "classic-sample", None, &json!({}))).unwrap_err();
        assert!(err.message.contains("block themes only"), "{}", err.message);
    }

    #[test]
    fn creates_preview_token_and_honours_expiry() {
        let db = FakeDb::new();
        let preview = block_on(create_preview(
            &db, 1,
            "wordless-default",
            &json!({ "color": { "palette": [{ "slug": "primary", "color": "#00FF00", "name": "Primary" }] } }),
        ))
        .unwrap();
        assert_eq!(preview.token.len(), 32);
        assert!(preview.token.bytes().all(|b| b.is_ascii_hexdigit()));
        let resolved = block_on(get_preview(&db, 1, &preview.token)).unwrap();
        assert_eq!(
            resolved.as_ref().map(|(s, _)| s.as_str()),
            Some("wordless-default")
        );
        assert!(block_on(get_preview(&db, 1, "deadbeef")).unwrap().is_none());
        // Expire it by hand and confirm it no longer resolves.
        let mut expired = preview.clone();
        expired.expires_at = "2020-01-01T00:00:00.000Z".to_string();
        block_on(set_option_json(
            &db, 1,
            PREVIEW_OPTION,
            &serde_json::to_value(&expired).unwrap(),
        ))
        .unwrap();
        assert!(block_on(get_preview(&db, 1, &preview.token))
            .unwrap()
            .is_none());
    }

    #[test]
    fn resolve_variation_prefers_preview_then_saved_then_none() {
        let db = FakeDb::new();
        assert!(block_on(resolve_variation(&db, 1, None)).unwrap().is_none());
        block_on(save_variation(
            &db, 1,
            "wordless-default",
            Some("Red"),
            &json!({ "color": { "palette": [{ "slug": "primary", "color": "#FF0000", "name": "Primary" }] } }),
        ))
        .unwrap();
        let saved = block_on(resolve_variation(&db, 1, None)).unwrap().unwrap();
        assert_eq!(saved["color"]["palette"][0]["color"], "#FF0000");
        let preview = block_on(create_preview(
            &db, 1,
            "wordless-default",
            &json!({ "color": { "palette": [{ "slug": "primary", "color": "#00FF00", "name": "Primary" }] } }),
        ))
        .unwrap();
        let via_preview = block_on(resolve_variation(&db, 1, Some(&preview.token)))
            .unwrap()
            .unwrap();
        assert_eq!(via_preview["color"]["palette"][0]["color"], "#00FF00");
        // An unknown token falls back to the saved variation.
        let fallback = block_on(resolve_variation(&db, 1, Some("deadbeef")))
            .unwrap()
            .unwrap();
        assert_eq!(fallback["color"]["palette"][0]["color"], "#FF0000");
        assert_eq!(DEFAULT_THEME_SLUG, "wordless-default");
    }

    #[test]
    fn delete_variation_removes_saved_variation() {
        let db = FakeDb::new();
        block_on(save_variation(
            &db, 1,
            "wordless-default",
            None,
            &json!({ "color": { "palette": [{ "slug": "primary", "color": "#FF0000", "name": "Primary" }] } }),
        ))
        .unwrap();
        assert!(block_on(delete_variation(&db, 1, "wordless-default")).unwrap());
        assert!(block_on(get_variation(&db, 1, "wordless-default"))
            .unwrap()
            .is_none());
        assert!(!block_on(delete_variation(&db, 1, "wordless-default")).unwrap());
    }
}
