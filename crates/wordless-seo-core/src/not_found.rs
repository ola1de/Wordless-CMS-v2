//! Public 404 logging (moved verbatim from `wordless-seo::redirects`,
//! 2026-11-20 SEO split).
//!
//! The main Worker records every public 404 in `wl_404_log`
//! (`lib.rs`, after routing) — so the write side must stay linked
//! by main. The read side (`list_not_found`, `delete_not_found`)
//! lives in `wordless-seo` behind the `/api/seo/404s` routes, and
//! `delete_not_found` reuses [`normalize_404_path`] from here, so
//! the two sides can never drift on what counts as a loggable path.

use wordless_db::{Db, Param};
use wordless_kernel::error::WordlessError;
use wordless_kernel::time::now_iso;

pub const MAX_404_PATHS: i64 = 500;

const SKIP_PREFIXES: [&str; 7] = [
    "/api/",
    "/admin",
    "/media/",
    "/theme.css",
    "/feed",
    "/sitemap",
    "/robots.txt",
];
const SKIP_EXACT: [&str; 6] = [
    "/favicon.ico",
    "/api",
    "/admin",
    "/sitemap.xml",
    "/robots.txt",
    "/feed",
];

/// Percent-decode a path segment; malformed `%` sequences are left
/// as-is (v1 logs undecodable paths as-is).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
        {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(n) = u8::from_str_radix(hex, 16) {
                out.push(n);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Normalise a request path for the log: strip query string +
/// fragment, decode, collapse duplicate slashes, keep a leading
/// slash, drop a trailing slash (except the root). Returns `None`
/// when the path should not be logged at all.
pub fn normalize_404_path(raw_path: &str) -> Option<String> {
    let without_query = raw_path.split(['?', '#']).next().unwrap_or("");
    let mut path = without_query.trim().to_string();
    if path.is_empty() {
        path = "/".to_string();
    }
    path = percent_decode(&path);
    // Collapse duplicate slashes.
    let mut collapsed = String::with_capacity(path.len());
    let mut prev_slash = false;
    for c in path.chars() {
        if c == '/' {
            if prev_slash {
                continue;
            }
            prev_slash = true;
        } else {
            prev_slash = false;
        }
        collapsed.push(c);
    }
    path = collapsed;
    if !path.starts_with('/') {
        path = format!("/{path}");
    }
    if path.len() > 1 {
        path = path.trim_end_matches('/').to_string();
    }
    if SKIP_EXACT.contains(&path.as_str()) {
        return None;
    }
    if SKIP_PREFIXES.iter().any(|p| path.starts_with(p)) {
        return None;
    }
    Some(path)
}

/// Record one public 404 (fire-and-forget safe: never throws).
/// P12: the log is per-site.
pub async fn log_not_found(db: &dyn Db, site_id: i64, raw_path: &str, referrer: &str) {
    let _ = log_not_found_inner(db, site_id, raw_path, referrer).await;
}

async fn log_not_found_inner(
    db: &dyn Db,
    site_id: i64,
    raw_path: &str,
    referrer: &str,
) -> Result<(), WordlessError> {
    let Some(path) = normalize_404_path(raw_path) else {
        return Ok(());
    };
    let now = now_iso();
    let referrer: String = referrer.chars().take(500).collect();
    let existing = db
        .query_one(
            "SELECT count FROM wl_404_log WHERE path = ? AND site_id = ? LIMIT 1",
            &[Param::Text(path.clone()), Param::Integer(site_id)],
        )
        .await
        .map_err(WordlessError::from)?;
    if existing.is_some() {
        // A repeat hit keeps the first referrer unless a new one is given.
        db.exec(
            "UPDATE wl_404_log SET count = count + 1, last_seen = ?, referrer = CASE WHEN ? = '' THEN referrer ELSE ? END WHERE path = ? AND site_id = ?",
            &[
                Param::Text(now),
                Param::Text(referrer.clone()),
                Param::Text(referrer),
                Param::Text(path),
                Param::Integer(site_id),
            ],
        )
        .await
        .map_err(WordlessError::from)?;
    } else {
        db.exec(
            "INSERT INTO wl_404_log (site_id, path, count, referrer, first_seen, last_seen) VALUES (?, ?, 1, ?, ?, ?)",
            &[
                Param::Integer(site_id),
                Param::Text(path),
                Param::Text(referrer),
                Param::Text(now.clone()),
                Param::Text(now),
            ],
        )
        .await
        .map_err(WordlessError::from)?;
        // Prune to the 500 most recently seen paths.
        db.exec(
            "DELETE FROM wl_404_log WHERE site_id = ? AND path NOT IN (SELECT path FROM wl_404_log WHERE site_id = ? ORDER BY last_seen DESC, count DESC LIMIT ?)",
            &[Param::Integer(site_id), Param::Integer(site_id), Param::Integer(MAX_404_PATHS)],
        )
        .await
        .map_err(WordlessError::from)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SqliteDb;
    use futures_executor::block_on;
    use wordless_db::Db;

    #[test]
    fn normalize_404_path_rules() {
        assert_eq!(
            normalize_404_path("/old-page?token=<redacted>&x=1").as_deref(),
            Some("/old-page")
        );
        assert_eq!(
            normalize_404_path("/old-page#frag").as_deref(),
            Some("/old-page")
        );
        assert_eq!(normalize_404_path("/favicon.ico"), None);
        assert_eq!(normalize_404_path("/api/posts"), None);
        assert_eq!(normalize_404_path("/admin"), None);
    }

    #[test]
    fn log_not_found_counts_repeats() {
        let db = SqliteDb::new();
        block_on(log_not_found(&db, 1, "/missing-page", "https://example.com"));
        block_on(log_not_found(&db, 1, "/missing-page", ""));
        let row = block_on(db.query_one(
            "SELECT count FROM wl_404_log WHERE path = ?",
            &[Param::Text("/missing-page".to_string())],
        ))
        .unwrap()
        .unwrap();
        assert_eq!(row["count"], serde_json::Value::from(2));
    }
}
