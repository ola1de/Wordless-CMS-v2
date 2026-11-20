//! Site settings (Milestone 3) — a validated view over `wp_options`.
//! Port of v1 `src/modules/settings/service.ts`.
//!
//! Keys (all stored as JSON in wp_options):
//! - blogname / blogdescription: site title / tagline
//! - posts_per_page: int 1–100 (used by public list rendering)
//! - permalink_structure: plain | postname | day-name | month-name
//! - timezone_string: IANA zone name or "UTC"
//! - default_comment_status: "open" | "closed" — comment_status for newly
//!   created posts (see posts/service)
//! - comment_max_links: int 0–10 — comments with more links stay pending

use serde::Serialize;
use wordless_db::Db;
use wordless_kernel::error::{bad_request, WordlessError};

use crate::login_branding::{
    apply_login_branding_patch, validate_login_branding_patch, LoginBrandingPatch,
};
use crate::permalinks::PermalinkStructure;
use crate::timezones::IANA_TIMEZONES;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteSettings {
    pub site_title: String,
    pub site_tagline: String,
    pub posts_per_page: i64,
    pub permalink_structure: PermalinkStructure,
    pub timezone: String,
    pub default_comment_status: String,
    pub comment_max_links: i64,
}

pub fn default_settings() -> SiteSettings {
    SiteSettings {
        site_title: "Wordless".to_string(),
        site_tagline: String::new(),
        posts_per_page: 10,
        permalink_structure: PermalinkStructure::Postname,
        timezone: "UTC".to_string(),
        default_comment_status: "open".to_string(),
        comment_max_links: 2,
    }
}

fn as_int(value: &serde_json::Value, fallback: i64) -> i64 {
    match value {
        serde_json::Value::Number(n) => n.as_i64().unwrap_or(fallback),
        _ => fallback,
    }
}

fn as_text(value: &serde_json::Value, fallback: &str) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        _ => fallback.to_string(),
    }
}

/// P12: every settings key is a per-site option (site 1 reads
/// `wp_options` exactly as before; other sites read
/// `wl_site_options` with fallback to `wp_options`).
pub async fn get_settings(db: &dyn Db, site_id: i64) -> Result<SiteSettings, WordlessError> {
    let defaults = default_settings();
    let title = as_text(
        &wordless_multisite::get_site_option::<serde_json::Value>(db, site_id, "blogname", serde_json::json!(defaults.site_title))
            .await?,
        &defaults.site_title,
    );
    let tagline = as_text(
        &wordless_multisite::get_site_option::<serde_json::Value>(db, site_id, "blogdescription", serde_json::json!("")).await?,
        "",
    );
    let per_page_raw =
        wordless_multisite::get_site_option::<serde_json::Value>(db, site_id, "posts_per_page", serde_json::json!(10)).await?;
    let per_page = as_int(&per_page_raw, 10).clamp(1, 100);
    let structure_raw = as_text(
        &wordless_multisite::get_site_option::<serde_json::Value>(db, site_id, "permalink_structure", serde_json::json!("postname"))
            .await?,
        "postname",
    );
    let timezone = as_text(
        &wordless_multisite::get_site_option::<serde_json::Value>(db, site_id, "timezone_string", serde_json::json!("UTC")).await?,
        "UTC",
    );
    let comment_status_raw = as_text(
        &wordless_multisite::get_site_option::<serde_json::Value>(db, site_id, "default_comment_status", serde_json::json!("open"))
            .await?,
        "open",
    );
    let max_links_raw =
        wordless_multisite::get_site_option::<serde_json::Value>(db, site_id, "comment_max_links", serde_json::json!(2)).await?;
    Ok(SiteSettings {
        site_title: if title.is_empty() {
            defaults.site_title
        } else {
            title
        },
        site_tagline: tagline,
        posts_per_page: per_page,
        permalink_structure: PermalinkStructure::from_str(&structure_raw)
            .unwrap_or(PermalinkStructure::Postname),
        timezone: if timezone.is_empty() {
            "UTC".to_string()
        } else {
            timezone
        },
        default_comment_status: if comment_status_raw == "closed" {
            "closed".to_string()
        } else {
            "open".to_string()
        },
        comment_max_links: as_int(&max_links_raw, 2).clamp(0, 10),
    })
}

/// A validated, cleaned settings patch (v1 `validateSettings`).
#[derive(Debug, Clone, Default)]
pub struct SettingsPatch {
    pub site_title: Option<String>,
    pub site_tagline: Option<String>,
    pub posts_per_page: Option<i64>,
    pub permalink_structure: Option<PermalinkStructure>,
    pub timezone: Option<String>,
    pub default_comment_status: Option<String>,
    pub comment_max_links: Option<i64>,
    /// P4 login customiser: the eleven branding fields, stored under
    /// the `wl_login_*` keys (see `crate::login_branding`). Additive
    /// to the v1 settings contract — the seven v1 fields behave
    /// exactly as before, and unknown keys are still ignored.
    pub login_branding: LoginBrandingPatch,
}

/// Validate a partial settings patch; returns the cleaned values to store.
/// Takes the raw JSON patch exactly as the route receives it, so the
/// type checks (`typeof x !== "string"`, integer checks) match v1.
pub fn validate_settings(patch: &serde_json::Value) -> Result<SettingsPatch, WordlessError> {
    let mut out = SettingsPatch::default();
    let obj = match patch.as_object() {
        Some(o) => o,
        None => return Ok(out),
    };
    if let Some(v) = obj.get("siteTitle") {
        match v.as_str() {
            Some(s) if !s.trim().is_empty() => {
                if s.trim().len() > 200 {
                    return Err(bad_request(
                        "Field \"siteTitle\" must be at most 200 characters.",
                    ));
                }
                out.site_title = Some(s.trim().to_string());
            }
            _ => {
                return Err(bad_request(
                    "Field \"siteTitle\" must be a non-empty string.",
                ))
            }
        }
    }
    if let Some(v) = obj.get("siteTagline") {
        match v.as_str() {
            Some(s) => {
                if s.len() > 500 {
                    return Err(bad_request(
                        "Field \"siteTagline\" must be at most 500 characters.",
                    ));
                }
                out.site_tagline = Some(s.to_string());
            }
            None => return Err(bad_request("Field \"siteTagline\" must be a string.")),
        }
    }
    if let Some(v) = obj.get("postsPerPage") {
        match v.as_i64() {
            Some(n) if v.is_i64() => {
                if !(1..=100).contains(&n) {
                    return Err(bad_request(
                        "Field \"postsPerPage\" must be between 1 and 100.",
                    ));
                }
                out.posts_per_page = Some(n);
            }
            _ => return Err(bad_request("Field \"postsPerPage\" must be an integer.")),
        }
    }
    if let Some(v) = obj.get("permalinkStructure") {
        match v.as_str().and_then(PermalinkStructure::from_str) {
            Some(s) => out.permalink_structure = Some(s),
            None => {
                return Err(bad_request(
                    "Field \"permalinkStructure\" must be one of plain, postname, day-name, month-name.",
                ))
            }
        }
    }
    if let Some(v) = obj.get("timezone") {
        match v.as_str() {
            Some(s) if !s.trim().is_empty() => {
                let tz = s.trim();
                if tz.len() > 100 {
                    return Err(bad_request(
                        "Field \"timezone\" must be at most 100 characters.",
                    ));
                }
                if !is_valid_timezone(tz) {
                    return Err(bad_request(
                        "Field \"timezone\" is not a valid IANA timezone name (e.g. \"Africa/Lagos\" or \"UTC\").",
                    ));
                }
                out.timezone = Some(tz.to_string());
            }
            _ => {
                return Err(bad_request(
                    "Field \"timezone\" must be a non-empty string.",
                ))
            }
        }
    }
    if let Some(v) = obj.get("defaultCommentStatus") {
        match v.as_str() {
            Some("open") => out.default_comment_status = Some("open".to_string()),
            Some("closed") => out.default_comment_status = Some("closed".to_string()),
            _ => {
                return Err(bad_request(
                    "Field \"defaultCommentStatus\" must be \"open\" or \"closed\".",
                ))
            }
        }
    }
    if let Some(v) = obj.get("commentMaxLinks") {
        match v.as_i64() {
            Some(n) if v.is_i64() => {
                if !(0..=10).contains(&n) {
                    return Err(bad_request(
                        "Field \"commentMaxLinks\" must be between 0 and 10.",
                    ));
                }
                out.comment_max_links = Some(n);
            }
            _ => return Err(bad_request("Field \"commentMaxLinks\" must be an integer.")),
        }
    }
    // P4 login customiser: the branding fields ride the same patch.
    out.login_branding = validate_login_branding_patch(patch)?;
    Ok(out)
}

/// v1 validates timezones with `Intl.DateTimeFormat`, which accepts any
/// real IANA name and rejects nonsense. This list is the Rust
/// equivalent (generated from the system tz database; see
/// `crate::timezones`).
pub fn is_valid_timezone(name: &str) -> bool {
    IANA_TIMEZONES.contains(&name)
}

pub async fn update_settings(
    db: &dyn Db,
    site_id: i64,
    patch: &serde_json::Value,
) -> Result<SiteSettings, WordlessError> {
    let clean = validate_settings(patch)?;
    if let Some(v) = &clean.site_title {
        wordless_multisite::set_site_option(db, site_id, "blogname", v).await?;
    }
    if let Some(v) = &clean.site_tagline {
        wordless_multisite::set_site_option(db, site_id, "blogdescription", v).await?;
    }
    if let Some(v) = clean.posts_per_page {
        wordless_multisite::set_site_option(db, site_id, "posts_per_page", &v).await?;
    }
    if let Some(v) = &clean.permalink_structure {
        wordless_multisite::set_site_option(db, site_id, "permalink_structure", &v.as_str()).await?;
    }
    if let Some(v) = &clean.timezone {
        wordless_multisite::set_site_option(db, site_id, "timezone_string", v).await?;
    }
    if let Some(v) = &clean.default_comment_status {
        wordless_multisite::set_site_option(db, site_id, "default_comment_status", v).await?;
    }
    if let Some(v) = clean.comment_max_links {
        wordless_multisite::set_site_option(db, site_id, "comment_max_links", &v).await?;
    }
    apply_login_branding_patch(db, site_id, &clean.login_branding).await?;
    get_settings(db, site_id).await
}

/// Posts-per-page for public list rendering (clamped 1–100, default 10).
pub async fn get_posts_per_page(db: &dyn Db, site_id: i64) -> Result<i64, WordlessError> {
    let raw = wordless_multisite::get_site_option::<serde_json::Value>(db, site_id, "posts_per_page", serde_json::json!(10)).await?;
    Ok(as_int(&raw, 10).clamp(1, 100))
}

/// Default comment_status for newly created posts ("open" | "closed").
pub async fn get_default_comment_status(db: &dyn Db, site_id: i64) -> Result<String, WordlessError> {
    let raw =
        wordless_multisite::get_site_option::<serde_json::Value>(db, site_id, "default_comment_status", serde_json::json!("open"))
            .await?;
    let s = as_text(&raw, "open");
    Ok(if s == "closed" {
        "closed".to_string()
    } else {
        "open".to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn timezone_list_accepts_real_and_rejects_nonsense() {
        assert!(is_valid_timezone("Africa/Lagos"));
        assert!(is_valid_timezone("UTC"));
        assert!(is_valid_timezone("America/New_York"));
        assert!(is_valid_timezone("Europe/London"));
        assert!(!is_valid_timezone("Mars/Olympus"));
        assert!(!is_valid_timezone("Not/AZone"));
        assert!(!is_valid_timezone(""));
    }

    #[test]
    fn validate_settings_accepts_and_rejects() {
        let ok =
            validate_settings(&json!({ "siteTitle": "  My Site ", "postsPerPage": 5 })).unwrap();
        assert_eq!(ok.site_title.as_deref(), Some("My Site"));
        assert_eq!(ok.posts_per_page, Some(5));
        assert!(validate_settings(&json!({ "siteTitle": "" })).is_err());
        assert!(validate_settings(&json!({ "postsPerPage": 0 })).is_err());
        assert!(validate_settings(&json!({ "postsPerPage": 101 })).is_err());
        assert!(validate_settings(&json!({ "postsPerPage": 2.5 })).is_err());
        assert!(validate_settings(&json!({ "timezone": "Mars/Olympus" })).is_err());
        assert!(validate_settings(&json!({ "timezone": "Africa/Lagos" })).is_ok());
        assert!(validate_settings(&json!({ "permalinkStructure": "day-name" })).is_ok());
        assert!(validate_settings(&json!({ "permalinkStructure": "bogus" })).is_err());
        assert!(validate_settings(&json!({ "commentMaxLinks": 11 })).is_err());
        // Unknown keys are ignored, exactly as v1 does.
        assert!(validate_settings(&json!({ "surprise": true })).is_ok());
    }
}
