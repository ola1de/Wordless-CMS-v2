//! Analytics (port of v1 `analytics.ts`) — config only, honest
//! reporting.
//!
//! No tracking snippet is injected unless the admin configures a
//! provider (GA4 measurement ID or Plausible domain) in the
//! integrations config. The status endpoint reports configured/not
//! and what the snippet does; there is no analytics data in Wordless
//! itself — page views, visitors and funnels live in the provider's
//! dashboard, never invented here. Supported snippets, exactly as
//! each provider documents them:
//! * GA4: the gtag.js loader + config call for the measurement ID.
//! * Plausible: the deferred script tag with `data-domain`.

use serde::{Deserialize, Serialize};
use wordless_db::Db;
use wordless_kernel::WordlessError;

use crate::{
    config::get_integration_config,
    util::{escape_html, site_url, url_host},
};

/// Analytics provider (v1 `AnalyticsProvider`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnalyticsProvider {
    None,
    Ga4,
    Plausible,
}

/// Status + snippet for the analytics integration (v1
/// `AnalyticsStatus`).
#[derive(Debug, Clone, Serialize)]
pub struct AnalyticsStatus {
    pub provider: AnalyticsProvider,
    #[serde(rename = "measurementId")]
    pub measurement_id: String,
    pub configured: bool,
    pub detail: String,
    /// The exact snippet injected into public pages (null when not
    /// configured).
    pub snippet: Option<String>,
}

async fn read_analytics_config(db: &dyn Db) -> Result<(AnalyticsProvider, String), WordlessError> {
    let config = get_integration_config(db, "analytics").await?;
    let provider = match config.get("provider").and_then(|v| v.as_str()) {
        Some("ga4") => AnalyticsProvider::Ga4,
        Some("plausible") => AnalyticsProvider::Plausible,
        _ => AnalyticsProvider::None,
    };
    let measurement_id = config
        .get("measurementId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    Ok((provider, measurement_id))
}

/// The snippet injected into public pages, or `None` when
/// unconfigured.
///
/// # Errors
/// Returns `Err` only on a database failure.
pub async fn build_analytics_snippet(db: &dyn Db) -> Result<Option<String>, WordlessError> {
    let (provider, measurement_id) = read_analytics_config(db).await?;
    if provider == AnalyticsProvider::None || measurement_id.is_empty() {
        return Ok(None);
    }
    let id = escape_html(&measurement_id);
    Ok(Some(match provider {
        AnalyticsProvider::Ga4 => format!(
            "<script async src=\"https://www.googletagmanager.com/gtag/js?id={id}\"></script>\n<script>window.dataLayer=window.dataLayer||[];function gtag(){{dataLayer.push(arguments);}}gtag('js',new Date());gtag('config','{id}');</script>"
        ),
        AnalyticsProvider::Plausible => format!(
            "<script defer data-domain=\"{id}\" src=\"https://plausible.io/js/script.js\"></script>"
        ),
        AnalyticsProvider::None => unreachable!(),
    }))
}

/// Full analytics status (configured state + detail + snippet).
///
/// # Errors
/// Returns `Err` only on a database failure.
pub async fn get_analytics_status(db: &dyn Db) -> Result<AnalyticsStatus, WordlessError> {
    let (provider, measurement_id) = read_analytics_config(db).await?;
    if provider == AnalyticsProvider::None || measurement_id.is_empty() {
        return Ok(AnalyticsStatus {
            provider: AnalyticsProvider::None,
            measurement_id: String::new(),
            configured: false,
            detail: "No analytics provider is configured — public pages carry no tracking snippet."
                .to_string(),
            snippet: None,
        });
    }
    let detail = match provider {
        AnalyticsProvider::Ga4 => format!(
            "Google Analytics 4 is configured (measurement ID {measurement_id}). Page views are counted by Google, not by Wordless."
        ),
        AnalyticsProvider::Plausible => format!(
            "Plausible is configured (domain {measurement_id}). Page views are counted by Plausible, not by Wordless."
        ),
        AnalyticsProvider::None => unreachable!(),
    };
    let snippet = build_analytics_snippet(db).await?;
    Ok(AnalyticsStatus {
        provider,
        measurement_id,
        configured: true,
        detail,
        snippet,
    })
}

/// The domain Plausible expects (the site host) — used as the
/// default measurementId hint.
///
/// # Errors
/// Returns `Err` only on a database failure.
pub async fn analytics_domain_hint(db: &dyn Db) -> Result<String, WordlessError> {
    let base = site_url(db).await?;
    Ok(url_host(&base))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{block_on, SqliteDb};
    use serde_json::json;
    use wordless_db::options::set_option;

    fn set_config(db: &SqliteDb, patch: serde_json::Value) {
        block_on(crate::config::set_integration_config(
            db,
            "analytics",
            &patch.as_object().unwrap().clone(),
        ))
        .unwrap();
    }

    #[test]
    fn reports_not_configured_until_provider_and_id_are_set() {
        let db = SqliteDb::new();
        let status = block_on(get_analytics_status(&db)).unwrap();
        assert!(!status.configured);
        assert_eq!(status.provider, AnalyticsProvider::None);
        assert!(status.snippet.is_none());
        set_config(&db, json!({ "provider": "ga4" }));
        assert!(!block_on(get_analytics_status(&db)).unwrap().configured);
    }

    #[test]
    fn builds_the_ga4_snippet() {
        let db = SqliteDb::new();
        set_config(
            &db,
            json!({ "provider": "ga4", "measurementId": "G-TEST123" }),
        );
        let status = block_on(get_analytics_status(&db)).unwrap();
        assert!(status.configured);
        let snippet = status.snippet.unwrap();
        assert!(
            snippet.contains("googletagmanager.com/gtag/js?id=G-TEST123"),
            "{snippet}"
        );
        assert!(snippet.contains("gtag('config','G-TEST123')"), "{snippet}");
    }

    #[test]
    fn builds_the_plausible_snippet_with_the_domain() {
        let db = SqliteDb::new();
        set_config(
            &db,
            json!({ "provider": "plausible", "measurementId": "example.com" }),
        );
        let snippet = block_on(build_analytics_snippet(&db)).unwrap().unwrap();
        assert_eq!(
            snippet,
            "<script defer data-domain=\"example.com\" src=\"https://plausible.io/js/script.js\"></script>"
        );
    }

    #[test]
    fn domain_hint_is_the_site_host() {
        let db = SqliteDb::new();
        block_on(set_option(
            &db,
            "siteurl",
            &"https://mysite.example.com".to_string(),
        ))
        .unwrap();
        assert_eq!(
            block_on(analytics_domain_hint(&db)).unwrap(),
            "mysite.example.com"
        );
    }
}
