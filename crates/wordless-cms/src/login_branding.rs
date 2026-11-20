//! Login-page branding (P4 login customiser — NEW in v2, no v1
//! equivalent; v1's login card is hardcoded in the SPA).
//!
//! Read path: public `GET /api/login-branding` returns exactly these
//! values, read from `wp_options` (JSON-encoded, like every option).
//! Write path: the Settings screen writes the same keys through the
//! existing `PUT /api/settings` (see `crate::settings` — the branding
//! fields are part of the settings patch; unknown-key behaviour is
//! unchanged for everything else). There is no `PUT /api/options/:key`
//! route in v1 or v2, so the settings patch is the write path.
//!
//! Option keys (all `wl_login_*`):
//!
//! | Field (wire, camelCase) | Option key | Default (v1 login look) |
//! |---|---|---|
//! | `logo` | `wl_login_logo` | `""` (built-in LogoMark) |
//! | `accent` | `wl_login_accent` | `#0d9488` (teal accent) |
//! | `deep` | `wl_login_deep` | `#07332e` (teal sidebar) |
//! | `bgMode` | `wl_login_bg_mode` | `"soft"` |
//! | `bgImage` | `wl_login_bg_image` | `""` (no image) |
//! | `layout` | `wl_login_layout` | `"card"` |
//! | `heading` | `wl_login_heading` | `"Welcome back"` |
//! | `tagline` | `wl_login_tagline` | `"The autonomous website operating system"` (v1 brand line) |
//! | `showSocial` | `wl_login_show_social` | `false` (v1 login has no social links) |
//! | `showBackLink` | `wl_login_show_back_link` | `true` (v1 login has "← Back to the site") |
//! | `customCss` | `wl_login_custom_css` | `""` |
//!
//! The defaults reproduce the current v1 login look when nothing is
//! stored. A stored value that fails validation (bad hex colour,
//! wrong JSON type) reads as its default rather than breaking the
//! public login page — branding must never 500 the login screen.

use serde::Serialize;
use wordless_db::Db;
use wordless_kernel::error::{bad_request, WordlessError};

use crate::options::{get_option, set_option};

pub const LOGIN_LOGO_KEY: &str = "wl_login_logo";
pub const LOGIN_ACCENT_KEY: &str = "wl_login_accent";
pub const LOGIN_DEEP_KEY: &str = "wl_login_deep";
pub const LOGIN_BG_MODE_KEY: &str = "wl_login_bg_mode";
pub const LOGIN_BG_IMAGE_KEY: &str = "wl_login_bg_image";
pub const LOGIN_LAYOUT_KEY: &str = "wl_login_layout";
pub const LOGIN_HEADING_KEY: &str = "wl_login_heading";
pub const LOGIN_TAGLINE_KEY: &str = "wl_login_tagline";
pub const LOGIN_SHOW_SOCIAL_KEY: &str = "wl_login_show_social";
pub const LOGIN_SHOW_BACK_LINK_KEY: &str = "wl_login_show_back_link";
pub const LOGIN_CUSTOM_CSS_KEY: &str = "wl_login_custom_css";

pub const DEFAULT_LOGIN_ACCENT: &str = "#0d9488";
pub const DEFAULT_LOGIN_DEEP: &str = "#07332e";
pub const DEFAULT_LOGIN_BG_MODE: &str = "soft";
pub const DEFAULT_LOGIN_LAYOUT: &str = "card";
pub const DEFAULT_LOGIN_HEADING: &str = "Welcome back";
pub const DEFAULT_LOGIN_TAGLINE: &str = "The autonomous website operating system";

/// The public login-branding payload (camelCase wire format).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginBranding {
    pub logo: String,
    pub accent: String,
    pub deep: String,
    pub bg_mode: String,
    pub bg_image: String,
    pub layout: String,
    pub heading: String,
    pub tagline: String,
    pub show_social: bool,
    pub show_back_link: bool,
    pub custom_css: String,
}

pub fn default_login_branding() -> LoginBranding {
    LoginBranding {
        logo: String::new(),
        accent: DEFAULT_LOGIN_ACCENT.to_string(),
        deep: DEFAULT_LOGIN_DEEP.to_string(),
        bg_mode: DEFAULT_LOGIN_BG_MODE.to_string(),
        bg_image: String::new(),
        layout: DEFAULT_LOGIN_LAYOUT.to_string(),
        heading: DEFAULT_LOGIN_HEADING.to_string(),
        tagline: DEFAULT_LOGIN_TAGLINE.to_string(),
        show_social: false,
        show_back_link: true,
        custom_css: String::new(),
    }
}

/// `#rgb` / `#rrggbb`, case-insensitive.
pub fn is_valid_hex_colour(value: &str) -> bool {
    let hex = value.strip_prefix('#').unwrap_or("");
    (hex.len() == 3 || hex.len() == 6) && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

fn as_text(value: &serde_json::Value, fallback: &str) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        _ => fallback.to_string(),
    }
}

fn as_bool(value: &serde_json::Value, fallback: bool) -> bool {
    match value {
        serde_json::Value::Bool(b) => *b,
        _ => fallback,
    }
}

/// Read the login branding, defaulting every missing or invalid
/// value (a bad stored value never breaks the login page).
pub async fn get_login_branding(db: &dyn Db, site_id: i64) -> Result<LoginBranding, WordlessError> {
    let defaults = default_login_branding();
    let logo = as_text(
        &wordless_multisite::get_site_option::<serde_json::Value>(db, site_id, LOGIN_LOGO_KEY, serde_json::json!("")).await?,
        "",
    );
    let accent_raw = as_text(
        &get_option::<serde_json::Value>(
            db,
            LOGIN_ACCENT_KEY,
            serde_json::json!(DEFAULT_LOGIN_ACCENT),
        )
        .await?,
        DEFAULT_LOGIN_ACCENT,
    );
    let accent = if is_valid_hex_colour(&accent_raw) {
        accent_raw
    } else {
        DEFAULT_LOGIN_ACCENT.to_string()
    };
    let deep_raw = as_text(
        &wordless_multisite::get_site_option::<serde_json::Value>(db, site_id, LOGIN_DEEP_KEY, serde_json::json!(DEFAULT_LOGIN_DEEP))
            .await?,
        DEFAULT_LOGIN_DEEP,
    );
    let deep = if is_valid_hex_colour(&deep_raw) {
        deep_raw
    } else {
        DEFAULT_LOGIN_DEEP.to_string()
    };
    let bg_mode = as_text(
        &get_option::<serde_json::Value>(
            db,
            LOGIN_BG_MODE_KEY,
            serde_json::json!(DEFAULT_LOGIN_BG_MODE),
        )
        .await?,
        DEFAULT_LOGIN_BG_MODE,
    );
    let bg_image = as_text(
        &wordless_multisite::get_site_option::<serde_json::Value>(db, site_id, LOGIN_BG_IMAGE_KEY, serde_json::json!("")).await?,
        "",
    );
    let layout = as_text(
        &get_option::<serde_json::Value>(
            db,
            LOGIN_LAYOUT_KEY,
            serde_json::json!(DEFAULT_LOGIN_LAYOUT),
        )
        .await?,
        DEFAULT_LOGIN_LAYOUT,
    );
    let heading = as_text(
        &get_option::<serde_json::Value>(
            db,
            LOGIN_HEADING_KEY,
            serde_json::json!(DEFAULT_LOGIN_HEADING),
        )
        .await?,
        DEFAULT_LOGIN_HEADING,
    );
    let tagline = as_text(
        &get_option::<serde_json::Value>(
            db,
            LOGIN_TAGLINE_KEY,
            serde_json::json!(DEFAULT_LOGIN_TAGLINE),
        )
        .await?,
        DEFAULT_LOGIN_TAGLINE,
    );
    let show_social = as_bool(
        &wordless_multisite::get_site_option::<serde_json::Value>(db, site_id, LOGIN_SHOW_SOCIAL_KEY, serde_json::json!(false))
            .await?,
        false,
    );
    let show_back_link = as_bool(
        &wordless_multisite::get_site_option::<serde_json::Value>(db, site_id, LOGIN_SHOW_BACK_LINK_KEY, serde_json::json!(true))
            .await?,
        true,
    );
    let custom_css = as_text(
        &wordless_multisite::get_site_option::<serde_json::Value>(db, site_id, LOGIN_CUSTOM_CSS_KEY, serde_json::json!("")).await?,
        "",
    );
    Ok(LoginBranding {
        logo,
        accent,
        deep,
        bg_mode: if bg_mode.is_empty() {
            defaults.bg_mode.clone()
        } else {
            bg_mode
        },
        bg_image,
        layout: if layout.is_empty() {
            defaults.layout.clone()
        } else {
            layout
        },
        heading: if heading.is_empty() {
            defaults.heading.clone()
        } else {
            heading
        },
        tagline,
        show_social,
        show_back_link,
        custom_css,
    })
}

/// A validated login-branding patch, as accepted inside the settings
/// patch (`PUT /api/settings`). `None` fields were absent.
#[derive(Debug, Clone, Default)]
pub struct LoginBrandingPatch {
    pub logo: Option<String>,
    pub accent: Option<String>,
    pub deep: Option<String>,
    pub bg_mode: Option<String>,
    pub bg_image: Option<String>,
    pub layout: Option<String>,
    pub heading: Option<String>,
    pub tagline: Option<String>,
    pub show_social: Option<bool>,
    pub show_back_link: Option<bool>,
    pub custom_css: Option<String>,
}

fn require_str_field(
    obj: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    max: usize,
) -> Result<Option<String>, WordlessError> {
    let Some(v) = obj.get(field) else {
        return Ok(None);
    };
    match v.as_str() {
        Some(s) => {
            if s.len() > max {
                return Err(bad_request(&format!(
                    "Field \"{field}\" must be at most {max} characters."
                )));
            }
            Ok(Some(s.to_string()))
        }
        None => Err(bad_request(&format!("Field \"{field}\" must be a string."))),
    }
}

fn require_bool_field(
    obj: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<Option<bool>, WordlessError> {
    let Some(v) = obj.get(field) else {
        return Ok(None);
    };
    match v.as_bool() {
        Some(b) => Ok(Some(b)),
        None => Err(bad_request(&format!(
            "Field \"{field}\" must be a boolean."
        ))),
    }
}

/// Validate the login-branding keys of a settings-patch object.
/// Unknown keys are the settings module's business, not this
/// function's — it only looks at the eleven branding fields.
pub fn validate_login_branding_patch(
    patch: &serde_json::Value,
) -> Result<LoginBrandingPatch, WordlessError> {
    let mut out = LoginBrandingPatch::default();
    let Some(obj) = patch.as_object() else {
        return Ok(out);
    };
    out.logo = require_str_field(obj, "loginLogo", 2000)?;
    if let Some(accent) = require_str_field(obj, "loginAccent", 7)? {
        if !is_valid_hex_colour(&accent) {
            return Err(bad_request(
                "Field \"loginAccent\" must be a hex colour, e.g. \"#0d9488\".",
            ));
        }
        out.accent = Some(accent);
    }
    if let Some(deep) = require_str_field(obj, "loginDeep", 7)? {
        if !is_valid_hex_colour(&deep) {
            return Err(bad_request(
                "Field \"loginDeep\" must be a hex colour, e.g. \"#07332e\".",
            ));
        }
        out.deep = Some(deep);
    }
    out.bg_mode = require_str_field(obj, "loginBgMode", 50)?;
    out.bg_image = require_str_field(obj, "loginBgImage", 2000)?;
    out.layout = require_str_field(obj, "loginLayout", 50)?;
    out.heading = require_str_field(obj, "loginHeading", 200)?;
    out.tagline = require_str_field(obj, "loginTagline", 500)?;
    out.show_social = require_bool_field(obj, "loginShowSocial")?;
    out.show_back_link = require_bool_field(obj, "loginShowBackLink")?;
    out.custom_css = require_str_field(obj, "loginCustomCss", 20000)?;
    Ok(out)
}

/// Store a validated branding patch under the `wl_login_*` keys.
pub async fn apply_login_branding_patch(
    db: &dyn Db,
    site_id: i64,
    patch: &LoginBrandingPatch,
) -> Result<(), WordlessError> {
    if let Some(v) = &patch.logo {
        wordless_multisite::set_site_option(db, site_id, LOGIN_LOGO_KEY, v).await?;
    }
    if let Some(v) = &patch.accent {
        wordless_multisite::set_site_option(db, site_id, LOGIN_ACCENT_KEY, v).await?;
    }
    if let Some(v) = &patch.deep {
        wordless_multisite::set_site_option(db, site_id, LOGIN_DEEP_KEY, v).await?;
    }
    if let Some(v) = &patch.bg_mode {
        wordless_multisite::set_site_option(db, site_id, LOGIN_BG_MODE_KEY, v).await?;
    }
    if let Some(v) = &patch.bg_image {
        wordless_multisite::set_site_option(db, site_id, LOGIN_BG_IMAGE_KEY, v).await?;
    }
    if let Some(v) = &patch.layout {
        wordless_multisite::set_site_option(db, site_id, LOGIN_LAYOUT_KEY, v).await?;
    }
    if let Some(v) = &patch.heading {
        wordless_multisite::set_site_option(db, site_id, LOGIN_HEADING_KEY, v).await?;
    }
    if let Some(v) = &patch.tagline {
        wordless_multisite::set_site_option(db, site_id, LOGIN_TAGLINE_KEY, v).await?;
    }
    if let Some(v) = patch.show_social {
        wordless_multisite::set_site_option(db, site_id, LOGIN_SHOW_SOCIAL_KEY, &v).await?;
    }
    if let Some(v) = patch.show_back_link {
        wordless_multisite::set_site_option(db, site_id, LOGIN_SHOW_BACK_LINK_KEY, &v).await?;
    }
    if let Some(v) = &patch.custom_css {
        wordless_multisite::set_site_option(db, site_id, LOGIN_CUSTOM_CSS_KEY, v).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_db::TestDb;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        futures_executor::block_on(f)
    }

    #[test]
    fn defaults_reproduce_the_v1_login_look() {
        let db = TestDb::new();
        let branding = block_on(get_login_branding(&db, 1)).unwrap();
        assert_eq!(branding, default_login_branding());
        assert_eq!(branding.accent, "#0d9488");
        assert_eq!(branding.deep, "#07332e");
        assert_eq!(branding.bg_mode, "soft");
        assert_eq!(branding.layout, "card");
        assert_eq!(branding.heading, "Welcome back");
        assert_eq!(branding.tagline, "The autonomous website operating system");
        assert!(!branding.show_social);
        assert!(branding.show_back_link);
        assert_eq!(branding.logo, "");
        assert_eq!(branding.bg_image, "");
        assert_eq!(branding.custom_css, "");
    }

    #[test]
    fn stored_values_round_trip_and_bad_values_fall_back() {
        let db = TestDb::new();
        let patch = LoginBrandingPatch {
            logo: Some("https://example.com/logo.png".to_string()),
            accent: Some("#2563eb".to_string()),
            deep: Some("#12275c".to_string()),
            bg_mode: Some("image".to_string()),
            bg_image: Some("https://example.com/bg.jpg".to_string()),
            layout: Some("split".to_string()),
            heading: Some("Sign in".to_string()),
            tagline: Some("Your site, your rules.".to_string()),
            show_social: Some(true),
            show_back_link: Some(false),
            custom_css: Some(".login-card { border-radius: 24px; }".to_string()),
        };
        block_on(apply_login_branding_patch(&db, 1, &patch)).unwrap();
        let branding = block_on(get_login_branding(&db, 1)).unwrap();
        assert_eq!(branding.logo, "https://example.com/logo.png");
        assert_eq!(branding.accent, "#2563eb");
        assert_eq!(branding.deep, "#12275c");
        assert_eq!(branding.bg_mode, "image");
        assert_eq!(branding.bg_image, "https://example.com/bg.jpg");
        assert_eq!(branding.layout, "split");
        assert_eq!(branding.heading, "Sign in");
        assert_eq!(branding.tagline, "Your site, your rules.");
        assert!(branding.show_social);
        assert!(!branding.show_back_link);
        assert_eq!(branding.custom_css, ".login-card { border-radius: 24px; }");

        // A bad stored colour / wrong-typed flag reads as its default
        // instead of breaking the login page.
        block_on(set_option(&db, LOGIN_ACCENT_KEY, &"not-a-colour")).unwrap();
        block_on(set_option(&db, LOGIN_SHOW_SOCIAL_KEY, &"yes")).unwrap();
        let branding = block_on(get_login_branding(&db, 1)).unwrap();
        assert_eq!(branding.accent, "#0d9488");
        assert!(!branding.show_social);
    }

    #[test]
    fn patch_validation_accepts_and_rejects() {
        use serde_json::json;
        let ok = validate_login_branding_patch(&json!({
            "loginAccent": "#0d9488", "loginDeep": "#07332e",
            "loginShowSocial": true, "loginBgMode": "gradient",
        }))
        .unwrap();
        assert_eq!(ok.accent.as_deref(), Some("#0d9488"));
        assert_eq!(ok.show_social, Some(true));
        assert!(validate_login_branding_patch(&json!({ "loginAccent": "red" })).is_err());
        assert!(validate_login_branding_patch(&json!({ "loginAccent": "#12345" })).is_err());
        assert!(validate_login_branding_patch(&json!({ "loginShowSocial": "yes" })).is_err());
        assert!(validate_login_branding_patch(&json!({ "loginHeading": 42 })).is_err());
        // Fields outside the branding set are not this function's business.
        assert!(validate_login_branding_patch(&json!({ "siteTitle": "X" })).is_ok());
        // 3-digit hex is accepted.
        assert!(validate_login_branding_patch(&json!({ "loginAccent": "#abc" })).is_ok());
    }

    #[test]
    fn hex_colour_shapes() {
        assert!(is_valid_hex_colour("#0d9488"));
        assert!(is_valid_hex_colour("#ABC"));
        assert!(!is_valid_hex_colour("0d9488"));
        assert!(!is_valid_hex_colour("#12345"));
        assert!(!is_valid_hex_colour("#gggggg"));
        assert!(!is_valid_hex_colour(""));
    }
}
