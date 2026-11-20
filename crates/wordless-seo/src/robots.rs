//! Configurable robots.txt (port of v1 `src/modules/seo/robots.ts`).
//!
//! Stored as the wp_options value `wl_robots_txt` (JSON string). When
//! unset, the M2 default is served (allow everything, sitemap line).
//!
//! Validation (enforced on write): at most 10,000 characters; every
//! non-empty, non-comment line must be a known directive
//! (`User-agent:`, `Allow:`, `Disallow:`, `Sitemap:`, `Crawl-delay:`).
//! A rule that would block the admin or the API is rejected — a typo in
//! robots.txt must never lock the site owner out of their own admin:
//! `Disallow: /` (blocks everything) and any `Disallow:` whose path is
//! `/admin`, `/api`, or a prefix of either (e.g. `/a`) is refused.
//! `Sitemap:` lines are left as written; the default sitemap line is
//! only added when the stored text has none.

use wordless_db::Db;
use wordless_kernel::error::{bad_request, WordlessError};

use wordless_seo_core::site::site_url;

pub const ROBOTS_OPTION: &str = "wl_robots_txt";
const MAX_ROBOTS_CHARS: usize = 10000;

const KNOWN_DIRECTIVES: [&str; 5] = [
    "user-agent:",
    "allow:",
    "disallow:",
    "sitemap:",
    "crawl-delay:",
];

/// Does a Disallow path block `target` (/admin or /api)? Robots
/// matching is prefix-based.
fn blocks_path(disallow_path: &str, target: &str) -> bool {
    let p = disallow_path.trim();
    if p.is_empty() {
        return false;
    }
    target.starts_with(p)
}

/// Validate robots.txt text; 400 with v1's message on the first bad line.
pub fn validate_robots_txt(text: &str) -> Result<(), WordlessError> {
    if text.len() > MAX_ROBOTS_CHARS {
        return Err(bad_request(&format!(
            "robots.txt is too long (max {MAX_ROBOTS_CHARS} characters)."
        )));
    }
    for raw_line in text.split('\n') {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        let known = KNOWN_DIRECTIVES.iter().find(|d| lower.starts_with(*d));
        let Some(known) = known else {
            return Err(bad_request(&format!(
                "Unknown robots.txt directive in line: \"{}\".",
                line.chars().take(80).collect::<String>()
            )));
        };
        if *known == "disallow:" {
            let path = line["disallow:".len()..].trim();
            if blocks_path(path, "/admin") || blocks_path(path, "/api") {
                return Err(bad_request(&format!(
                    "robots.txt must not block /admin or /api (refused: \"{}\").",
                    line.chars().take(80).collect::<String>()
                )));
            }
        }
    }
    Ok(())
}

/// The robots.txt that is actually served (stored text, or the default).
/// P10: the default lists every sitemap file (main, index, image,
/// video) so crawlers find them without guessing. A stored file that
/// already has any `Sitemap:` line is left exactly as written.
/// P12: robots.txt is a per-site option.
pub async fn get_robots_txt(db: &dyn Db, site_id: i64) -> Result<String, WordlessError> {
    let base = site_url(db, site_id).await?;
    let stored: Option<String> = wordless_multisite::get_site_option(db, site_id, ROBOTS_OPTION, None)
        .await
        .map_err(WordlessError::from)?;
    let default_lines = format!(
        "User-agent: *\nAllow: /\nSitemap: {base}/sitemap.xml\nSitemap: {base}/sitemap-index.xml\nSitemap: {base}/sitemap-images.xml\nSitemap: {base}/sitemap-videos.xml\n"
    );
    match stored {
        None => Ok(default_lines),
        Some(stored) if stored.trim().is_empty() => Ok(default_lines),
        Some(stored) => {
            let text = if stored.ends_with('\n') {
                stored
            } else {
                format!("{stored}\n")
            };
            // A `Sitemap:` line anywhere (case-insensitive, line start,
            // optional leading whitespace) suppresses the default line.
            let has_sitemap = text
                .lines()
                .any(|l| l.trim_start().to_ascii_lowercase().starts_with("sitemap:"));
            if has_sitemap {
                Ok(text)
            } else {
                Ok(format!(
                    "{text}Sitemap: {base}/sitemap.xml\nSitemap: {base}/sitemap-index.xml\nSitemap: {base}/sitemap-images.xml\nSitemap: {base}/sitemap-videos.xml\n"
                ))
            }
        }
    }
}

/// Validate, store, and return the served text. An empty string clears
/// the stored value (the default is then served).
pub async fn set_robots_txt(db: &dyn Db, site_id: i64, text: &str) -> Result<String, WordlessError> {
    validate_robots_txt(text)?;
    let trimmed = text.trim();
    wordless_multisite::set_site_option(db, site_id, ROBOTS_OPTION, &trimmed.to_string())
        .await
        .map_err(WordlessError::from)?;
    get_robots_txt(db, site_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SqliteDb;
    use futures_executor::block_on;

    #[test]
    fn serves_the_default_and_round_trips_a_custom_file() {
        let db = SqliteDb::new();
        let def = block_on(get_robots_txt(&db, 1)).unwrap();
        assert!(def.contains("User-agent: *"));
        assert!(def.contains("Sitemap: http://localhost:8787/sitemap.xml"));
        assert!(def.contains("Sitemap: http://localhost:8787/sitemap-index.xml"));
        assert!(def.contains("Sitemap: http://localhost:8787/sitemap-images.xml"));
        assert!(def.contains("Sitemap: http://localhost:8787/sitemap-videos.xml"));
        let custom = block_on(set_robots_txt(&db, 1, "User-agent: *\nDisallow: /private/\n")).unwrap();
        assert!(custom.contains("Disallow: /private/"));
        assert!(custom.contains("Sitemap: http://localhost:8787/sitemap.xml"));
        assert_eq!(block_on(get_robots_txt(&db, 1)).unwrap(), custom);
        let cleared = block_on(set_robots_txt(&db, 1, "")).unwrap();
        assert!(cleared.contains("Allow: /"));
    }

    #[test]
    fn rejects_rules_that_would_block_the_admin_or_the_api() {
        let db = SqliteDb::new();
        let err = |r: Result<String, WordlessError>| r.unwrap_err().message;
        assert!(err(block_on(set_robots_txt(
            &db, 1,
            "User-agent: *\nDisallow: /\n"
        )))
        .contains("/admin"));
        assert!(err(block_on(set_robots_txt(
            &db, 1,
            "User-agent: *\nDisallow: /admin\n"
        )))
        .contains("/admin"));
        assert!(err(block_on(set_robots_txt(
            &db, 1,
            "User-agent: *\nDisallow: /api\n"
        )))
        .contains("/api"));
        assert!(err(block_on(set_robots_txt(
            &db, 1,
            "User-agent: *\nDisallow: /a\n"
        )))
        .contains("/admin"));
        assert!(err(block_on(set_robots_txt(
            &db, 1,
            "User-agent: *\nDisallow: /private\nBanana: yes\n"
        )))
        .contains("Unknown robots.txt directive"));
        // But a rule that merely shares a prefix is fine.
        let ok = block_on(set_robots_txt(
            &db, 1,
            "User-agent: *\nDisallow: /apiary/\nDisallow: /administrator/\n",
        ))
        .unwrap();
        assert!(ok.contains("Disallow: /apiary/"));
    }
}
