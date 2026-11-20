//! Redirect manager + 404 log (port of v1
//! `src/modules/seo/redirects.ts` + `src/modules/seo/notfound.ts`).
//!
//! Redirects: admin CRUD over `wl_redirects`. Slug changes already
//! create redirects automatically (M2); this module is the manual
//! side: list / create / update / delete. Both paths must start with
//! "/"; the old path must differ from the new path; the new path may
//! be an absolute http(s) URL (off-site redirect) or a site path.
//! Creating a redirect for an old path that already exists is a 400
//! (the old path is the unique key) — update it instead.
//!
//! 404 log: public 404s are recorded per path (query strings and
//! fragments are stripped before storage — a 404 URL must never
//! smuggle a token or password into the log). One row per path, with
//! a hit counter and first/last seen timestamps. The table is capped
//! at 500 paths: after an insert, the oldest rows beyond 500 (by
//! last_seen) are pruned. Asset and admin paths are not logged
//! (favicon, theme CSS, /api/*, /admin*): those are not content 404s.

use serde::{Deserialize, Serialize};
use wordless_cms::permalinks::{find_redirect_by_path, find_redirect_by_slug};
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param};
use wordless_kernel::error::{bad_request, not_found, WordlessError};
use wordless_kernel::time::now_iso;
use wordless_seo_core::not_found::normalize_404_path;

// ---------------------------------------------------------------- redirects

/// One redirect row (v1 `RedirectRecord`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedirectRecord {
    pub id: i64,
    pub old_path: String,
    pub new_path: String,
    pub post_id: i64,
    pub created_at: String,
}

/// Paginated redirect list (v1 `RedirectList`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedirectList {
    pub redirects: Vec<RedirectRecord>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

fn as_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn row_to_redirect(row: &serde_json::Value) -> RedirectRecord {
    RedirectRecord {
        id: row_int(row, "id"),
        old_path: row_text(row, "old_path"),
        new_path: row_text(row, "new_path"),
        post_id: row_int(row, "post_id"),
        created_at: row_text(row, "created_at"),
    }
}

fn normalize_path(path: &str) -> String {
    let p = path.trim();
    if p == "/" {
        "/".to_string()
    } else {
        p.trim_end_matches('/').to_string()
    }
}

fn require_site_path(value: &str, field: &str) -> Result<String, WordlessError> {
    if value.trim().is_empty() {
        return Err(bad_request(&format!(
            "Field \"{field}\" must be a non-empty path starting with \"/\"."
        )));
    }
    if !value.starts_with('/') {
        return Err(bad_request(&format!(
            "Field \"{field}\" must start with \"/\"."
        )));
    }
    Ok(normalize_path(value))
}

fn require_target(value: &str, field: &str) -> Result<String, WordlessError> {
    if value.trim().is_empty() {
        return Err(bad_request(&format!(
            "Field \"{field}\" must be a non-empty path or URL."
        )));
    }
    let v = value.trim();
    if v.starts_with('/') {
        return Ok(normalize_path(v));
    }
    let lower = v.to_ascii_lowercase();
    let rest = if let Some(r) = lower.strip_prefix("http://") {
        r
    } else if let Some(r) = lower.strip_prefix("https://") {
        r
    } else {
        return Err(bad_request(&format!(
            "Field \"{field}\" must be a site path or an absolute http(s) URL."
        )));
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() || v.contains(char::is_whitespace) {
        return Err(bad_request(&format!(
            "Field \"{field}\" must be a site path or an absolute http(s) URL."
        )));
    }
    Ok(v.to_string())
}

/// List redirects, newest first. Page ≥ 1, per-page clamped 1–100,
/// default 20.
pub async fn list_redirects(
    db: &dyn Db,
    site_id: i64,
    page: Option<i64>,
    per_page: Option<i64>,
) -> Result<RedirectList, WordlessError> {
    let page = page.unwrap_or(1).max(1);
    let per_page = per_page.unwrap_or(20).clamp(1, 100);
    let total_row = db
        .query_one("SELECT COUNT(*) AS n FROM wl_redirects WHERE site_id = ?", &[Param::Integer(site_id)])
        .await
        .map_err(WordlessError::from)?;
    let total = total_row.map(|r| row_int(&r, "n")).unwrap_or(0);
    let rows = db
        .query_all(
            "SELECT * FROM wl_redirects WHERE site_id = ? ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?",
            &[
                Param::Integer(site_id),
                Param::Integer(per_page),
                Param::Integer((page - 1) * per_page),
            ],
        )
        .await
        .map_err(WordlessError::from)?;
    Ok(RedirectList {
        redirects: rows.iter().map(row_to_redirect).collect(),
        total,
        page,
        per_page,
        total_pages: ((total + per_page - 1) / per_page.max(1)).max(1),
    })
}

/// Create a redirect (old path is the unique key).
pub async fn create_redirect(
    db: &dyn Db,
    site_id: i64,
    old_path: &str,
    new_path: &str,
) -> Result<RedirectRecord, WordlessError> {
    let old_path = require_site_path(old_path, "oldPath")?;
    let new_path = require_target(new_path, "newPath")?;
    if old_path == new_path {
        return Err(bad_request(
            "A redirect cannot point at itself (\"oldPath\" equals \"newPath\").",
        ));
    }
    let existing = db
        .query_one(
            "SELECT id FROM wl_redirects WHERE old_path = ? AND site_id = ? LIMIT 1",
            &[Param::Text(old_path.clone()), Param::Integer(site_id)],
        )
        .await
        .map_err(WordlessError::from)?;
    if existing.is_some() {
        return Err(bad_request(&format!(
            "A redirect for \"{old_path}\" already exists — update it instead."
        )));
    }
    let meta = db
        .exec(
            "INSERT INTO wl_redirects (site_id, old_path, new_path, post_id, created_at) VALUES (?, ?, ?, 0, ?)",
            &[
                Param::Integer(site_id),
                Param::Text(old_path),
                Param::Text(new_path),
                Param::Text(now_iso()),
            ],
        )
        .await
        .map_err(WordlessError::from)?;
    let row = db
        .query_one(
            "SELECT * FROM wl_redirects WHERE id = ? AND site_id = ? LIMIT 1",
            &[Param::Integer(meta.last_row_id.unwrap_or(0)), Param::Integer(site_id)],
        )
        .await
        .map_err(WordlessError::from)?;
    match row {
        Some(row) => Ok(row_to_redirect(&row)),
        None => Err(WordlessError::new(
            "internal_error",
            500,
            "Failed to read back the created redirect.",
        )),
    }
}

/// Update a redirect's target. 404 when the id does not exist.
pub async fn update_redirect(
    db: &dyn Db,
    site_id: i64,
    id: i64,
    new_path: &str,
) -> Result<RedirectRecord, WordlessError> {
    let existing = db
        .query_one(
            "SELECT * FROM wl_redirects WHERE id = ? AND site_id = ? LIMIT 1",
            &[Param::Integer(id), Param::Integer(site_id)],
        )
        .await
        .map_err(WordlessError::from)?;
    let Some(existing) = existing else {
        return Err(not_found(&format!("Redirect {id} not found.")));
    };
    let new_path = require_target(new_path, "newPath")?;
    let old_path = as_text(&existing["old_path"]);
    if old_path == new_path {
        return Err(bad_request(
            "A redirect cannot point at itself (\"oldPath\" equals \"newPath\").",
        ));
    }
    db.exec(
        "UPDATE wl_redirects SET new_path = ? WHERE id = ? AND site_id = ?",
        &[Param::Text(new_path), Param::Integer(id), Param::Integer(site_id)],
    )
    .await
    .map_err(WordlessError::from)?;
    let row = db
        .query_one(
            "SELECT * FROM wl_redirects WHERE id = ? AND site_id = ? LIMIT 1",
            &[Param::Integer(id), Param::Integer(site_id)],
        )
        .await
        .map_err(WordlessError::from)?;
    match row {
        Some(row) => Ok(row_to_redirect(&row)),
        None => Err(WordlessError::new(
            "internal_error",
            500,
            "Failed to read back the updated redirect.",
        )),
    }
}

/// Delete a redirect. 404 when the id does not exist.
pub async fn delete_redirect(db: &dyn Db, site_id: i64, id: i64) -> Result<bool, WordlessError> {
    let existing = db
        .query_one(
            "SELECT id FROM wl_redirects WHERE id = ? AND site_id = ? LIMIT 1",
            &[Param::Integer(id), Param::Integer(site_id)],
        )
        .await
        .map_err(WordlessError::from)?;
    if existing.is_none() {
        return Err(not_found(&format!("Redirect {id} not found.")));
    }
    let meta = db
        .exec(
            "DELETE FROM wl_redirects WHERE id = ? AND site_id = ?",
            &[Param::Integer(id), Param::Integer(site_id)],
        )
        .await
        .map_err(WordlessError::from)?;
    Ok(meta.changes > 0)
}

/// Apply redirects for a public request path, exactly as v1's
/// `resolvePath` does: an exact `old_path` match first, then a match
/// on the trailing slug. Returns `(target, 301)` when one applies.
pub async fn apply_redirect(
    db: &dyn Db,
    site_id: i64,
    path: &str,
) -> Result<Option<(String, u16)>, WordlessError> {
    if let Some(target) = find_redirect_by_path(db, site_id, path).await? {
        return Ok(Some((target, 301)));
    }
    let slug = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    if !slug.is_empty() && slug != path {
        if let Some(target) = find_redirect_by_slug(db, site_id, slug).await? {
            return Ok(Some((target, 301)));
        }
    }
    Ok(None)
}

// ---------------------------------------------------------------- 404 log
// (Write side — normalize + log — lives in wordless-seo-core::not_found;
// this module keeps the read side: list + delete.)

/// One 404-log row (v1 `NotFoundEntry`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotFoundEntry {
    pub path: String,
    pub count: i64,
    pub referrer: String,
    pub first_seen: String,
    pub last_seen: String,
}

/// Paginated 404 list (v1 `NotFoundList`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotFoundList {
    pub entries: Vec<NotFoundEntry>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

/// List 404-log entries, most-hit first. Page ≥ 1, per-page clamped
/// 1–100, default 20.
pub async fn list_not_found(
    db: &dyn Db,
    site_id: i64,
    page: Option<i64>,
    per_page: Option<i64>,
) -> Result<NotFoundList, WordlessError> {
    let page = page.unwrap_or(1).max(1);
    let per_page = per_page.unwrap_or(20).clamp(1, 100);
    let total_row = db
        .query_one("SELECT COUNT(*) AS n FROM wl_404_log WHERE site_id = ?", &[Param::Integer(site_id)])
        .await
        .map_err(WordlessError::from)?;
    let total = total_row.map(|r| row_int(&r, "n")).unwrap_or(0);
    let rows = db
        .query_all(
            "SELECT * FROM wl_404_log WHERE site_id = ? ORDER BY count DESC, last_seen DESC LIMIT ? OFFSET ?",
            &[
                Param::Integer(site_id),
                Param::Integer(per_page),
                Param::Integer((page - 1) * per_page),
            ],
        )
        .await
        .map_err(WordlessError::from)?;
    Ok(NotFoundList {
        entries: rows
            .iter()
            .map(|row| NotFoundEntry {
                path: row_text(row, "path"),
                count: row_int(row, "count"),
                referrer: row_text(row, "referrer"),
                first_seen: row_text(row, "first_seen"),
                last_seen: row_text(row, "last_seen"),
            })
            .collect(),
        total,
        page,
        per_page,
        total_pages: ((total + per_page - 1) / per_page.max(1)).max(1),
    })
}

/// Delete one 404-log entry (the raw path is normalised first).
/// Returns false when the normalised path is not logged or absent.
pub async fn delete_not_found(db: &dyn Db, site_id: i64, raw_path: &str) -> Result<bool, WordlessError> {
    let Some(path) = normalize_404_path(raw_path) else {
        return Ok(false);
    };
    let meta = db
        .exec(
            "DELETE FROM wl_404_log WHERE path = ? AND site_id = ?",
            &[Param::Text(path), Param::Integer(site_id)],
        )
        .await
        .map_err(WordlessError::from)?;
    Ok(meta.changes > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SqliteDb;
    use futures_executor::block_on;
    use wordless_seo_core::not_found::{log_not_found, normalize_404_path};

    #[test]
    fn creates_lists_updates_and_deletes_redirects() {
        let db = SqliteDb::new();
        let created = block_on(create_redirect(&db, 1, "/old", "/new")).unwrap();
        assert_eq!(created.old_path, "/old");
        assert_eq!(block_on(list_redirects(&db, 1, None, None)).unwrap().total, 1);
        let updated = block_on(update_redirect(&db, 1, created.id, "/newer")).unwrap();
        assert_eq!(updated.new_path, "/newer");
        assert!(block_on(delete_redirect(&db, 1, created.id)).unwrap());
        assert_eq!(block_on(list_redirects(&db, 1, None, None)).unwrap().total, 0);
    }

    #[test]
    fn validates_paths_and_duplicates() {
        let db = SqliteDb::new();
        block_on(create_redirect(&db, 1, "/old", "/new")).unwrap();
        let err = |r: Result<RedirectRecord, WordlessError>| r.unwrap_err().message;
        assert!(err(block_on(create_redirect(&db, 1, "/old", "/other"))).contains("already exists"));
        assert!(err(block_on(create_redirect(&db, 1, "no-slash", "/x"))).contains("start with \"/\""));
        assert!(err(block_on(create_redirect(&db, 1, "/same", "/same"))).contains("point at itself"));
        assert!(
            err(block_on(create_redirect(&db, 1, "/x", "javascript:alert(1)")))
                .contains("site path or an absolute")
        );
        let offsite = block_on(create_redirect(
            &db, 1,
            "/away",
            "https://example.org/elsewhere",
        ))
        .unwrap();
        assert_eq!(offsite.new_path, "https://example.org/elsewhere");
        assert!(block_on(update_redirect(&db, 1, 9999, "/x"))
            .unwrap_err()
            .message
            .contains("not found"));
    }

    #[test]
    fn applies_exact_and_trailing_slug_redirects() {
        let db = SqliteDb::new();
        block_on(create_redirect(&db, 1, "/old-post", "/new-post")).unwrap();
        assert_eq!(
            block_on(apply_redirect(&db, 1, "/old-post")).unwrap(),
            Some(("/new-post".to_string(), 301))
        );
        // Trailing-slug match (v1 resolvePath's redirect-by-slug step).
        assert_eq!(
            block_on(apply_redirect(&db, 1, "/2026/11/old-post")).unwrap(),
            Some(("/new-post".to_string(), 301))
        );
        assert_eq!(block_on(apply_redirect(&db, 1, "/nothing")).unwrap(), None);
    }

    #[test]
    fn strips_query_strings_and_fragments_counts_repeats() {
        assert_eq!(
            normalize_404_path("/old-page?token=abc&x=1").as_deref(),
            Some("/old-page")
        );
        assert_eq!(
            normalize_404_path("/old-page#frag").as_deref(),
            Some("/old-page")
        );
        assert_eq!(normalize_404_path("/favicon.ico"), None);
        assert_eq!(normalize_404_path("/api/posts"), None);
        assert_eq!(normalize_404_path("/admin"), None);
        let db = SqliteDb::new();
        block_on(log_not_found(
            &db, 1,
            "/gone?secret=hunter2",
            "https://example.test/",
        ));
        block_on(log_not_found(&db, 1, "/gone", ""));
        block_on(log_not_found(&db, 1, "/other", ""));
        let list = block_on(list_not_found(&db, 1, None, None)).unwrap();
        assert_eq!(list.total, 2);
        assert_eq!(list.entries[0].path, "/gone");
        assert_eq!(list.entries[0].count, 2);
        assert_eq!(list.entries[0].referrer, "https://example.test/");
        assert!(block_on(delete_not_found(&db, 1, "/gone?x=1")).unwrap());
        assert_eq!(block_on(list_not_found(&db, 1, None, None)).unwrap().total, 1);
    }

    #[test]
    fn prunes_to_the_500_most_recently_seen_paths() {
        let db = SqliteDb::new();
        for i in 0..505 {
            block_on(log_not_found(&db, 1, &format!("/missing-{i}"), ""));
        }
        let list = block_on(list_not_found(&db, 1, None, Some(100))).unwrap();
        assert_eq!(list.total, 500);
    }
}
