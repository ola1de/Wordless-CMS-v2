//! Site base URL helper (moved verbatim from `wordless-seo::sitemap`,
//! 2026-11-20 SEO split).
//!
//! The helper is tiny but shared three ways — the main Worker renders
//! it into page heads, the school Worker builds course URLs with it,
//! and the SEO Worker uses it for sitemaps / llms.txt / robots.txt —
//! so it lives in the shared core crate instead of any one Worker.

use wordless_db::Db;
use wordless_kernel::error::WordlessError;

/// Site base URL (option `siteurl`, default `http://localhost:8787`,
/// trailing slashes stripped) — same as v1 `feeds/service.ts`.
/// P12: the site URL is a per-site option.
pub async fn site_url(db: &dyn Db, site_id: i64) -> Result<String, WordlessError> {
    let url: String = wordless_multisite::get_site_option(db, site_id, "siteurl", "http://localhost:8787".to_string())
        .await
        .map_err(WordlessError::from)?;
    Ok(url.trim_end_matches('/').to_string())
}
