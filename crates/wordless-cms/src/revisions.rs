//! Revisions — port of v1 `src/modules/revisions/` (service.ts, repo.ts, types.ts).
//!
//! Snapshots of a post stored as `wp_posts` rows, exactly like
//! WordPress: post_type 'revision', post_status 'inherit', post_parent =
//! the post id. Autosaves are revisions whose post_name ends in
//! '-autosave-v1' (one per post + author, updated in place).

use serde::Serialize;
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, not_found, WordlessError};
use wordless_kernel::time::now_iso;

use crate::posts::{find_post_by_id, update_post_row, Post};

/// A revision row, serialised with the v1 camelCase field names.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Revision {
    pub id: i64,
    pub post_id: i64,
    pub author_id: i64,
    pub date: String,
    pub title: String,
    pub content: String,
    pub excerpt: String,
    pub slug: String,
    /// 'inherit' for real revisions; autosaves are also 'inherit'.
    pub status: String,
    pub is_autosave: bool,
}

pub fn row_to_revision(row: &Row) -> Revision {
    let slug = row_text(row, "post_name");
    Revision {
        id: row_int(row, "ID"),
        post_id: row_int(row, "post_parent"),
        author_id: row_int(row, "post_author"),
        date: row_text(row, "post_date"),
        title: row_text(row, "post_title"),
        content: row_text(row, "post_content"),
        excerpt: row_text(row, "post_excerpt"),
        slug: slug.clone(),
        status: row_text(row, "post_status"),
        is_autosave: slug.ends_with("-autosave-v1"),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionDiff {
    /// Number of lines present in `to` but not in `from`.
    pub added: i64,
    /// Number of lines present in `from` but not in `to`.
    pub removed: i64,
    /// First differing added lines (max 10).
    pub added_lines: Vec<String>,
    /// First differing removed lines (max 10).
    pub removed_lines: Vec<String>,
    /// Whether the diff is exact (LCS) or the set-based fallback.
    pub exact: bool,
    pub from_id: i64,
    pub to_id: i64,
}

// ---------------------------------------------------------------- repo

pub async fn insert_revision_row(
    db: &dyn Db,
    post: &Post,
    slug: &str,
) -> Result<i64, WordlessError> {
    let date = if post.modified.is_empty() {
        post.date.clone()
    } else {
        post.modified.clone()
    };
    let date_gmt = if post.modified_gmt.is_empty() {
        post.date_gmt.clone()
    } else {
        post.modified_gmt.clone()
    };
    let meta = db
        .prepare(
            "INSERT INTO wp_posts
               (site_id, post_author, post_date, post_date_gmt, post_content, post_title, post_excerpt,
                post_status, post_name, post_modified, post_modified_gmt, post_parent,
                post_type, post_mime_type)
             VALUES (?, ?, ?, ?, ?, ?, ?, 'inherit', ?, ?, ?, ?, 'revision', '')",
        )
        .bind(vec![
            Param::Integer(post.site_id),
            Param::Integer(post.author_id),
            Param::Text(date.clone()),
            Param::Text(date_gmt),
            Param::Text(post.content.clone()),
            Param::Text(post.title.clone()),
            Param::Text(post.excerpt.clone()),
            Param::Text(slug.to_string()),
            Param::Text(date.clone()),
            Param::Text(date),
            Param::Integer(post.id),
        ])
        .run()
        .await?;
    Ok(meta.last_row_id.unwrap_or(0))
}

pub async fn find_revisions_by_post(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    limit: i64,
) -> Result<Vec<Revision>, WordlessError> {
    let rows: Vec<Row> = db
        .prepare(
            "SELECT * FROM wp_posts
              WHERE post_parent = ? AND post_type = 'revision' AND site_id = ?
              ORDER BY post_date DESC, ID DESC LIMIT ?",
        )
        .bind(vec![Param::Integer(post_id), Param::Integer(site_id), Param::Integer(limit)])
        .all()
        .await?;
    Ok(rows.iter().map(row_to_revision).collect())
}

pub async fn find_revision_by_id(db: &dyn Db, site_id: i64, id: i64) -> Result<Option<Revision>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wp_posts WHERE ID = ? AND post_type = 'revision' AND site_id = ? LIMIT 1")
        .bind(vec![Param::Integer(id), Param::Integer(site_id)])
        .first()
        .await?;
    Ok(row.map(|r| row_to_revision(&r)))
}

pub async fn find_autosave(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    author_id: i64,
) -> Result<Option<Revision>, WordlessError> {
    let row: Option<Row> = db
        .prepare(
            "SELECT * FROM wp_posts
              WHERE post_parent = ? AND post_type = 'revision' AND post_author = ?
                AND post_name = ? AND site_id = ? LIMIT 1",
        )
        .bind(vec![
            Param::Integer(post_id),
            Param::Integer(author_id),
            Param::Text(format!("{post_id}-autosave-v1")),
            Param::Integer(site_id),
        ])
        .first()
        .await?;
    Ok(row.map(|r| row_to_revision(&r)))
}

pub async fn update_revision_row(
    db: &dyn Db,
    site_id: i64,
    id: i64,
    title: &str,
    content: &str,
    excerpt: &str,
    date: &str,
) -> Result<u64, WordlessError> {
    let meta = db
        .prepare(
            "UPDATE wp_posts SET post_title = ?, post_content = ?, post_excerpt = ?,
               post_date = ?, post_date_gmt = ?, post_modified = ?, post_modified_gmt = ?
             WHERE ID = ? AND post_type = 'revision' AND site_id = ?",
        )
        .bind(vec![
            Param::Text(title.to_string()),
            Param::Text(content.to_string()),
            Param::Text(excerpt.to_string()),
            Param::Text(date.to_string()),
            Param::Text(date.to_string()),
            Param::Text(date.to_string()),
            Param::Text(date.to_string()),
            Param::Integer(id),
            Param::Integer(site_id),
        ])
        .run()
        .await?;
    Ok(meta.changes)
}

pub async fn count_revisions(db: &dyn Db, site_id: i64, post_id: i64) -> Result<i64, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT COUNT(*) AS total FROM wp_posts WHERE post_parent = ? AND post_type = 'revision' AND site_id = ?")
        .bind(vec![Param::Integer(post_id), Param::Integer(site_id)])
        .first()
        .await?;
    Ok(crate::common::count_total(&row))
}

// ---------------------------------------------------------------- service

/// Snapshot `post` as a new revision row. Returns the revision.
pub async fn create_revision(db: &dyn Db, site_id: i64, post: &Post) -> Result<Revision, WordlessError> {
    // A revision inherits its post's site; callers pass post.site_id.
    let version = count_revisions(db, site_id, post.id).await? + 1;
    let slug = format!("{}-revision-v{version}", post.id);
    let id = insert_revision_row(db, post, &slug).await?;
    let revision = find_revision_by_id(db, site_id, id).await?;
    revision.ok_or_else(|| {
        WordlessError::new(
            "internal_error",
            500,
            "Failed to read back the created revision.",
        )
    })
}

pub async fn list_revisions(db: &dyn Db, site_id: i64, post_id: i64) -> Result<Vec<Revision>, WordlessError> {
    let post = find_post_by_id(db, site_id, post_id).await?;
    if post.is_none() {
        return Err(not_found(&format!("Post {post_id} not found.")));
    }
    find_revisions_by_post(db, site_id, post_id, 100).await
}

pub async fn get_revision(db: &dyn Db, site_id: i64, id: i64) -> Result<Revision, WordlessError> {
    find_revision_by_id(db, site_id, id)
        .await?
        .ok_or_else(|| not_found(&format!("Revision {id} not found.")))
}

pub async fn save_autosave(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    author_id: i64,
    title: Option<&str>,
    content: Option<&str>,
    excerpt: Option<&str>,
) -> Result<Revision, WordlessError> {
    let post = find_post_by_id(db, site_id, post_id)
        .await?
        .ok_or_else(|| not_found(&format!("Post {post_id} not found.")))?;
    let now = now_iso();
    let title = title.unwrap_or(&post.title).to_string();
    let content = content.unwrap_or(&post.content).to_string();
    let excerpt = excerpt.unwrap_or(&post.excerpt).to_string();
    let existing = find_autosave(db, site_id, post_id, author_id).await?;
    if let Some(autosave) = existing {
        update_revision_row(db, site_id, autosave.id, &title, &content, &excerpt, &now).await?;
        let updated = find_revision_by_id(db, site_id, autosave.id).await?;
        return updated.ok_or_else(|| {
            WordlessError::new("internal_error", 500, "Failed to read back the autosave.")
        });
    }
    let snapshot = Post {
        author_id,
        title,
        content,
        excerpt,
        ..post.clone()
    };
    let id = insert_revision_row(db, &snapshot, &format!("{post_id}-autosave-v1")).await?;
    let created = find_revision_by_id(db, site_id, id).await?;
    created.ok_or_else(|| {
        WordlessError::new("internal_error", 500, "Failed to read back the autosave.")
    })
}

pub async fn restore_revision(db: &dyn Db, site_id: i64, revision_id: i64) -> Result<Post, WordlessError> {
    let revision = find_revision_by_id(db, site_id, revision_id)
        .await?
        .ok_or_else(|| not_found(&format!("Revision {revision_id} not found.")))?;
    let post = find_post_by_id(db, site_id, revision.post_id)
        .await?
        .ok_or_else(|| not_found(&format!("Post {} not found.", revision.post_id)))?;
    // Snapshot the pre-restore content first, so the restore itself is undoable.
    create_revision(db, site_id, &post).await?;
    let now = now_iso();
    update_post_row(
        db,
        site_id,
        post.id,
        &[
            "post_title = ?".to_string(),
            "post_content = ?".to_string(),
            "post_excerpt = ?".to_string(),
            "post_modified = ?".to_string(),
            "post_modified_gmt = ?".to_string(),
        ],
        &[
            Param::Text(revision.title.clone()),
            Param::Text(revision.content.clone()),
            Param::Text(revision.excerpt.clone()),
            Param::Text(now.clone()),
            Param::Text(now),
        ],
    )
    .await?;
    let restored = find_post_by_id(db, site_id, post.id).await?;
    restored.ok_or_else(|| {
        WordlessError::new(
            "internal_error",
            500,
            "Failed to read back the restored post.",
        )
    })
}

fn split_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n').collect()
}

const LCS_LINE_LIMIT: usize = 1000;
const DIFF_SAMPLE: usize = 10;

/// Line diff between two texts. For inputs up to 1000 lines each, an LCS
/// diff gives exact added/removed lines in order. Above that, a set-based
/// comparison is used (counts may under-count repeated lines); the method
/// is reported honestly in the result via `exact: false`.
/// Port of v1 `diffLines` (`revisions/service.ts`).
pub fn diff_lines(from: &str, to: &str) -> RevisionDiff {
    let a = split_lines(from);
    let b = split_lines(to);
    if a.len() > LCS_LINE_LIMIT || b.len() > LCS_LINE_LIMIT {
        let set_a: std::collections::HashSet<&str> = a.iter().copied().collect();
        let set_b: std::collections::HashSet<&str> = b.iter().copied().collect();
        let added_lines: Vec<String> = b
            .iter()
            .filter(|line| !set_a.contains(*line))
            .take(DIFF_SAMPLE)
            .map(|s| s.to_string())
            .collect();
        let removed_lines: Vec<String> = a
            .iter()
            .filter(|line| !set_b.contains(*line))
            .take(DIFF_SAMPLE)
            .map(|s| s.to_string())
            .collect();
        return RevisionDiff {
            added: b.iter().filter(|line| !set_a.contains(*line)).count() as i64,
            removed: a.iter().filter(|line| !set_b.contains(*line)).count() as i64,
            added_lines,
            removed_lines,
            exact: false,
            from_id: 0,
            to_id: 0,
        };
    }
    // LCS dynamic-programming table (a.len()+1) x (b.len()+1).
    let m = a.len();
    let n = b.len();
    let mut dp: Vec<Vec<i64>> = vec![vec![0; n + 1]; m + 1];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let mut added_lines: Vec<String> = Vec::new();
    let mut removed_lines: Vec<String> = Vec::new();
    let mut added = 0i64;
    let mut removed = 0i64;
    let mut i = 0;
    let mut j = 0;
    while i < m && j < n {
        if a[i] == b[j] {
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            removed += 1;
            if removed_lines.len() < DIFF_SAMPLE {
                removed_lines.push(a[i].to_string());
            }
            i += 1;
        } else {
            added += 1;
            if added_lines.len() < DIFF_SAMPLE {
                added_lines.push(b[j].to_string());
            }
            j += 1;
        }
    }
    while i < m {
        removed += 1;
        if removed_lines.len() < DIFF_SAMPLE {
            removed_lines.push(a[i].to_string());
        }
        i += 1;
    }
    while j < n {
        added += 1;
        if added_lines.len() < DIFF_SAMPLE {
            added_lines.push(b[j].to_string());
        }
        j += 1;
    }
    RevisionDiff {
        added,
        removed,
        added_lines,
        removed_lines,
        exact: true,
        from_id: 0,
        to_id: 0,
    }
}

pub async fn diff_revisions(
    db: &dyn Db,
    site_id: i64,
    from_id: i64,
    to_id: i64,
) -> Result<RevisionDiff, WordlessError> {
    let from = find_revision_by_id(db, site_id, from_id)
        .await?
        .ok_or_else(|| not_found(&format!("Revision {from_id} not found.")))?;
    let to = find_revision_by_id(db, site_id, to_id)
        .await?
        .ok_or_else(|| not_found(&format!("Revision {to_id} not found.")))?;
    if from.post_id != to.post_id {
        return Err(bad_request("Cannot diff revisions of different posts."));
    }
    let mut diff = diff_lines(&from.content, &to.content);
    diff.from_id = from_id;
    diff.to_id = to_id;
    Ok(diff)
}

// Re-export for the render crate's convenience (v1 `revisions/types.ts`
// re-exports `Post` the same way).
pub use crate::posts::Post as RevisionPost;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_lines_exact() {
        let d = diff_lines("a\nb\nc", "a\nx\nc\nd");
        assert!(d.exact);
        assert_eq!(d.added, 2);
        assert_eq!(d.removed, 1);
        assert_eq!(d.added_lines, vec!["x".to_string(), "d".to_string()]);
        assert_eq!(d.removed_lines, vec!["b".to_string()]);
    }

    #[test]
    fn diff_lines_empty_and_identical() {
        let d = diff_lines("", "");
        assert_eq!(d.added, 0);
        assert_eq!(d.removed, 0);
        let d = diff_lines("a\nb", "a\nb");
        assert_eq!(d.added, 0);
        assert_eq!(d.removed, 0);
    }

    #[test]
    fn diff_lines_fallback_over_limit() {
        let big = (0..1001)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let d = diff_lines(&big, "other");
        assert!(!d.exact);
        assert_eq!(d.added, 1);
        assert_eq!(d.removed, 1001);
    }
}
