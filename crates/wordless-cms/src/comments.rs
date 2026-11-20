//! Comments — port of v1 `src/modules/comments/` (service.ts, repo.ts, types.ts).
//!
//! - New comments are `pending` by default. A logged-in user who can
//!   `moderate_comments` gets their comment `approved` immediately.
//! - Spam guard (no external service in M2): a filled honeypot field, or
//!   more links than `comment_max_links` (option, default 2), forces the
//!   comment to `spam` (honeypot) or `pending` (too many links).
//! - Comments only on posts whose comment_status is `open`.
//! - comment_count on wp_posts counts approved comments only and is kept
//!   in sync on every create / status change / delete.
//!
//! Stored values in `wp_comments.comment_approved`: '1' (approved),
//! '0' (pending), 'spam', 'trash'.

use serde::{Deserialize, Serialize};
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, not_found, WordlessError};
use wordless_kernel::time::now_iso;

use crate::common::{count_total, page_of, per_page_of, total_pages};
use crate::posts::find_post_by_id;

/// Comment status. Serialises to the v1 strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommentStatus {
    Approved,
    Pending,
    Spam,
    Trash,
}

impl CommentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            CommentStatus::Approved => "approved",
            CommentStatus::Pending => "pending",
            CommentStatus::Spam => "spam",
            CommentStatus::Trash => "trash",
        }
    }

    /// See `PostStatus::from_str` for why this is not `std::str::FromStr`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<CommentStatus> {
        match s {
            "approved" => Some(CommentStatus::Approved),
            "pending" => Some(CommentStatus::Pending),
            "spam" => Some(CommentStatus::Spam),
            "trash" => Some(CommentStatus::Trash),
            _ => None,
        }
    }
}

pub fn is_comment_status(value: &str) -> bool {
    CommentStatus::from_str(value).is_some()
}

/// Stored value in wp_comments.comment_approved.
pub fn to_stored_status(status: CommentStatus) -> &'static str {
    match status {
        CommentStatus::Approved => "1",
        CommentStatus::Pending => "0",
        CommentStatus::Spam => "spam",
        CommentStatus::Trash => "trash",
    }
}

pub fn from_stored_status(value: &str) -> CommentStatus {
    match value {
        "1" => CommentStatus::Approved,
        "0" => CommentStatus::Pending,
        "spam" => CommentStatus::Spam,
        "trash" => CommentStatus::Trash,
        _ => CommentStatus::Pending,
    }
}

/// A comment, serialised with the v1 camelCase field names.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: i64,
    pub post_id: i64,
    pub author: String,
    pub author_email: String,
    pub author_url: String,
    pub author_ip: String,
    pub date: String,
    pub date_gmt: String,
    pub content: String,
    pub status: CommentStatus,
    pub parent_id: i64,
    pub user_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<Comment>>,
}

pub fn row_to_comment(row: &Row) -> Comment {
    Comment {
        id: row_int(row, "comment_ID"),
        post_id: row_int(row, "comment_post_ID"),
        author: row_text(row, "comment_author"),
        author_email: row_text(row, "comment_author_email"),
        author_url: row_text(row, "comment_author_url"),
        author_ip: row_text(row, "comment_author_IP"),
        date: row_text(row, "comment_date"),
        date_gmt: row_text(row, "comment_date_gmt"),
        content: row_text(row, "comment_content"),
        status: from_stored_status(&row_text(row, "comment_approved")),
        parent_id: row_int(row, "comment_parent"),
        user_id: row_int(row, "user_id"),
        children: None,
    }
}

#[derive(Debug, Clone, Default)]
pub struct CreateCommentInput {
    pub post_id: i64,
    pub author: String,
    pub author_email: Option<String>,
    pub author_url: Option<String>,
    pub content: String,
    pub parent_id: Option<i64>,
    pub user_id: Option<i64>,
    pub author_ip: Option<String>,
    /// Honeypot field: must be empty. A filled honeypot means a bot.
    pub honeypot: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ListCommentsOptions {
    pub post_id: Option<i64>,
    pub status: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentListResult {
    pub comments: Vec<Comment>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

// ---------------------------------------------------------------- link counting
// Port of v1 `countLinks` (comments/service.ts). The v1 regexes are:
//   URL_RE    /https?:\/\/[^\s<>"']+/gi
//   ANCHOR_RE /<a\s[^>]*href=/gi
//   anchor href extraction /<a\s[^>]*href=["']([^"']+)["']/gi
// An <a href="https://…"> counts once; plain URLs not already counted
// as an anchor href count once each.

fn is_url_char(c: char) -> bool {
    !(c.is_whitespace() || c == '<' || c == '>' || c == '"' || c == '\'')
}

/// Find the next occurrence of `needle` in `haystack` (ASCII
/// case-insensitive), starting at byte offset `from`. Returns the byte offset.
fn find_ci(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    let mut i = from;
    while i + n.len() <= h.len() {
        if h[i..i + n.len()].eq_ignore_ascii_case(n) {
            return Some(i);
        }
        i += 1;
    }
    None
}

pub fn count_links(content: &str) -> usize {
    // Anchor hrefs: <a\s[^>]*href=["']([^"']+)["']
    let mut anchor_urls: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut cursor = 0;
    while let Some(a_pos) = find_ci(content, "<a", cursor) {
        let after_a = a_pos + 2;
        // `\s` after `<a` (v1 requires whitespace between `a` and attributes).
        if after_a >= content.len() || !content.as_bytes()[after_a].is_ascii_whitespace() {
            cursor = after_a;
            continue;
        }
        // Find the end of the tag.
        let tag_end = content[after_a..]
            .find('>')
            .map(|p| after_a + p)
            .unwrap_or(content.len());
        let tag = &content[after_a..tag_end];
        if let Some(href_pos) = find_ci(tag, "href", 0) {
            let mut j = href_pos + 4;
            let bytes = tag.as_bytes();
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'=' {
                j += 1;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j < bytes.len() && (bytes[j] == b'"' || bytes[j] == b'\'') {
                    let quote = bytes[j];
                    let start = j + 1;
                    if let Some(end) = tag[start..].find(quote as char).map(|p| start + p) {
                        let href = &tag[start..end];
                        if !href.is_empty() {
                            anchor_urls.insert(href.to_string());
                        }
                    }
                }
            }
        }
        cursor = tag_end + 1;
    }
    // Plain URLs: https?://[^\s<>"']+
    let mut plain = 0usize;
    let mut i = 0;
    let bytes = content.as_bytes();
    while i < bytes.len() {
        let rest = &content[i..];
        let lower = rest.len().min(8);
        let prefix = &rest[..lower].to_ascii_lowercase();
        let url_len = if prefix.starts_with("https://") {
            Some(8)
        } else if prefix.starts_with("http://") {
            Some(7)
        } else {
            None
        };
        match url_len {
            Some(scheme_len) => {
                let start = i;
                let mut j = i + scheme_len;
                while j < bytes.len() && is_url_char(content.as_bytes()[j] as char) {
                    // is_url_char works on chars; bytes are ASCII-safe here
                    // because the excluded set is ASCII and multi-byte
                    // UTF-8 bytes are never ASCII whitespace/punct.
                    j += 1;
                }
                let url = &content[start..j];
                if !anchor_urls.contains(url) {
                    plain += 1;
                }
                i = j;
            }
            None => {
                i += 1;
            }
        }
    }
    anchor_urls.len() + plain
}

// ---------------------------------------------------------------- repo

#[allow(clippy::too_many_arguments)]
async fn insert_comment_row(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    author: &str,
    author_email: &str,
    author_url: &str,
    author_ip: &str,
    date: &str,
    content: &str,
    status: CommentStatus,
    parent_id: i64,
    user_id: i64,
) -> Result<i64, WordlessError> {
    let meta = db
        .prepare(
            "INSERT INTO wp_comments
               (site_id, comment_post_ID, comment_author, comment_author_email, comment_author_url,
                comment_author_IP, comment_date, comment_date_gmt, comment_content,
                comment_approved, comment_agent, comment_type, comment_parent, user_id)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, '', 'comment', ?, ?)",
        )
        .bind(vec![
            Param::Integer(site_id),
            Param::Integer(post_id),
            Param::Text(author.to_string()),
            Param::Text(author_email.to_string()),
            Param::Text(author_url.to_string()),
            Param::Text(author_ip.to_string()),
            Param::Text(date.to_string()),
            Param::Text(date.to_string()),
            Param::Text(content.to_string()),
            Param::Text(to_stored_status(status).to_string()),
            Param::Integer(parent_id),
            Param::Integer(user_id),
        ])
        .run()
        .await?;
    Ok(meta.last_row_id.unwrap_or(0))
}

pub async fn find_comment_by_id(db: &dyn Db, site_id: i64, id: i64) -> Result<Option<Comment>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wp_comments WHERE comment_ID = ? AND site_id = ? LIMIT 1")
        .bind(vec![Param::Integer(id), Param::Integer(site_id)])
        .first()
        .await?;
    Ok(row.map(|r| row_to_comment(&r)))
}

async fn count_comments(
    db: &dyn Db,
    where_clause: &str,
    params: &[Param],
) -> Result<i64, WordlessError> {
    let row: Option<Row> = db
        .prepare(&format!(
            "SELECT COUNT(*) AS total FROM wp_comments {where_clause}"
        ))
        .bind(params.to_vec())
        .first()
        .await?;
    Ok(count_total(&row))
}

async fn find_comments(
    db: &dyn Db,
    where_clause: &str,
    params: &[Param],
    limit: i64,
    offset: i64,
) -> Result<Vec<Comment>, WordlessError> {
    let mut all_params = params.to_vec();
    all_params.push(Param::Integer(limit));
    all_params.push(Param::Integer(offset));
    let rows: Vec<Row> = db
        .prepare(&format!(
            "SELECT * FROM wp_comments {where_clause} ORDER BY comment_date ASC, comment_ID ASC LIMIT ? OFFSET ?"
        ))
        .bind(all_params)
        .all()
        .await?;
    Ok(rows.iter().map(row_to_comment).collect())
}

async fn update_comment_status(
    db: &dyn Db,
    site_id: i64,
    id: i64,
    status: CommentStatus,
) -> Result<u64, WordlessError> {
    let meta = db
        .prepare("UPDATE wp_comments SET comment_approved = ? WHERE comment_ID = ? AND site_id = ?")
        .bind(vec![
            Param::Text(to_stored_status(status).to_string()),
            Param::Integer(id),
            Param::Integer(site_id),
        ])
        .run()
        .await?;
    Ok(meta.changes)
}

async fn delete_comment_row(db: &dyn Db, site_id: i64, id: i64) -> Result<u64, WordlessError> {
    // Replies to a deleted comment become top-level (WordPress deletes the
    // whole thread only on explicit trash-empty; M2 keeps replies).
    db.prepare("UPDATE wp_comments SET comment_parent = 0 WHERE comment_parent = ? AND site_id = ?")
        .bind(vec![Param::Integer(id), Param::Integer(site_id)])
        .run()
        .await?;
    let meta = db
        .prepare("DELETE FROM wp_comments WHERE comment_ID = ? AND site_id = ?")
        .bind(vec![Param::Integer(id), Param::Integer(site_id)])
        .run()
        .await?;
    Ok(meta.changes)
}

async fn count_approved_comments_for_post(db: &dyn Db, site_id: i64, post_id: i64) -> Result<i64, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT COUNT(*) AS total FROM wp_comments WHERE comment_post_ID = ? AND comment_approved = '1' AND site_id = ?")
        .bind(vec![Param::Integer(post_id), Param::Integer(site_id)])
        .first()
        .await?;
    Ok(count_total(&row))
}

/// Sync the denormalised comment_count on wp_posts (approved comments only).
async fn sync_post_comment_count(db: &dyn Db, site_id: i64, post_id: i64) -> Result<(), WordlessError> {
    let count = count_approved_comments_for_post(db, site_id, post_id).await?;
    db.prepare("UPDATE wp_posts SET comment_count = ? WHERE ID = ? AND site_id = ?")
        .bind(vec![Param::Integer(count), Param::Integer(post_id), Param::Integer(site_id)])
        .run()
        .await?;
    Ok(())
}

// ---------------------------------------------------------------- service

pub async fn create_comment(
    db: &dyn Db,
    site_id: i64,
    input: CreateCommentInput,
    can_moderate: bool,
) -> Result<Comment, WordlessError> {
    let post = find_post_by_id(db, site_id, input.post_id)
        .await?
        .ok_or_else(|| not_found(&format!("Post {} not found.", input.post_id)))?;
    if post.comment_status != "open" {
        return Err(bad_request("Comments are closed for this post."));
    }
    let author = input.author.trim().to_string();
    if author.is_empty() || author.len() > 100 {
        return Err(bad_request(
            "Field \"author\" is required and must be at most 100 characters.",
        ));
    }
    let content = input.content.trim().to_string();
    if content.is_empty() || content.len() > 10000 {
        return Err(bad_request(
            "Field \"content\" is required and must be at most 10000 characters.",
        ));
    }
    if let Some(parent_id) = input.parent_id {
        if parent_id != 0 {
            let parent = find_comment_by_id(db, site_id, parent_id).await?;
            match parent {
                None => return Err(not_found(&format!("Parent comment {parent_id} not found."))),
                Some(p) if p.post_id != input.post_id => {
                    return Err(bad_request(
                        "A reply must belong to the same post as its parent.",
                    ))
                }
                _ => {}
            }
        }
    }

    let mut status = if can_moderate {
        CommentStatus::Approved
    } else {
        CommentStatus::Pending
    };
    // Honeypot: a bot that fills the hidden field is spam, full stop.
    if let Some(honeypot) = &input.honeypot {
        if !honeypot.trim().is_empty() {
            status = CommentStatus::Spam;
        }
    }
    if status != CommentStatus::Spam {
        // P12: comment_max_links is a per-site option.
        let max_links: i64 =
            wordless_multisite::get_site_option(db, site_id, "comment_max_links", 2).await?;
        let limit = if max_links >= 0 { max_links } else { 2 };
        if count_links(&content) as i64 > limit {
            status = CommentStatus::Pending;
        }
    }

    let now = now_iso();
    let id = insert_comment_row(
        db,
        site_id,
        input.post_id,
        &author,
        input.author_email.as_deref().unwrap_or("").trim(),
        input.author_url.as_deref().unwrap_or("").trim(),
        input.author_ip.as_deref().unwrap_or(""),
        &now,
        &content,
        status,
        input.parent_id.unwrap_or(0),
        input.user_id.unwrap_or(0),
    )
    .await?;
    sync_post_comment_count(db, site_id, input.post_id).await?;
    let created = find_comment_by_id(db, site_id, id).await?;
    created.ok_or_else(|| {
        WordlessError::new(
            "internal_error",
            500,
            "Failed to read back the created comment.",
        )
    })
}

pub async fn get_comment(db: &dyn Db, site_id: i64, id: i64) -> Result<Comment, WordlessError> {
    find_comment_by_id(db, site_id, id)
        .await?
        .ok_or_else(|| not_found(&format!("Comment {id} not found.")))
}

pub async fn list_comments(
    db: &dyn Db,
    site_id: i64,
    opts: ListCommentsOptions,
) -> Result<CommentListResult, WordlessError> {
    let page = page_of(opts.page);
    let per_page = per_page_of(opts.per_page);
    let mut where_parts: Vec<String> = vec!["site_id = ?".to_string()];
    let mut params: Vec<Param> = vec![Param::Integer(site_id)];
    if let Some(post_id) = opts.post_id {
        where_parts.push("comment_post_ID = ?".to_string());
        params.push(Param::Integer(post_id));
    }
    if let Some(status) = &opts.status {
        if !is_comment_status(status) {
            return Err(bad_request(&format!(
                "Unknown comment status \"{status}\"."
            )));
        }
        where_parts.push("comment_approved = ?".to_string());
        params.push(Param::Text(match status.as_str() {
            "approved" => "1".to_string(),
            "pending" => "0".to_string(),
            other => other.to_string(),
        }));
    }
    let where_clause = if where_parts.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_parts.join(" AND "))
    };
    let total = count_comments(db, &where_clause, &params).await?;
    let total_pages = total_pages(total, per_page);
    let comments =
        find_comments(db, &where_clause, &params, per_page, (page - 1) * per_page).await?;
    Ok(CommentListResult {
        comments,
        total,
        page,
        per_page,
        total_pages,
    })
}

/// Nest a flat approved-comment list into threads (children arrays).
/// Pure function, unit-tested directly; `list_approved_threaded` is
/// the async wrapper that fetches the flat list first.
pub(crate) fn thread_comments(comments: Vec<Comment>) -> Vec<Comment> {
    // Index by id, with children pre-filled from the flat list.
    let mut by_id: std::collections::HashMap<i64, Comment> = std::collections::HashMap::new();
    for comment in &comments {
        by_id.insert(
            comment.id,
            Comment {
                children: Some(Vec::new()),
                ..comment.clone()
            },
        );
    }
    for comment in &comments {
        if comment.parent_id != 0 && by_id.contains_key(&comment.parent_id) {
            let child = by_id.get(&comment.id).cloned();
            if let (Some(child), Some(parent)) = (child, by_id.get_mut(&comment.parent_id)) {
                parent.children.get_or_insert_with(Vec::new).push(child);
            }
        }
    }
    // Rebuild from the map so nesting is correct at every depth
    // (the in-place attach above only filled one level on the stored copy).
    fn rebuild(map: &std::collections::HashMap<i64, Comment>, id: i64) -> Option<Comment> {
        let node = map.get(&id)?;
        let mut out = node.clone();
        if let Some(children) = &node.children {
            out.children = Some(children.iter().filter_map(|c| rebuild(map, c.id)).collect());
        }
        Some(out)
    }
    let mut roots: Vec<Comment> = Vec::new();
    for comment in &comments {
        if comment.parent_id == 0 || !by_id.contains_key(&comment.parent_id) {
            if let Some(root) = rebuild(&by_id, comment.id) {
                roots.push(root);
            }
        }
    }
    roots
}

/// Approved comments for a post, nested into threads (children arrays).
pub async fn list_approved_threaded(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
) -> Result<Vec<Comment>, WordlessError> {
    let result = list_comments(
        db,
        site_id,
        ListCommentsOptions {
            post_id: Some(post_id),
            status: Some("approved".to_string()),
            per_page: Some(100),
            ..Default::default()
        },
    )
    .await?;
    Ok(thread_comments(result.comments))
}

pub async fn set_comment_status(
    db: &dyn Db,
    site_id: i64,
    id: i64,
    status: &str,
) -> Result<Comment, WordlessError> {
    let status = CommentStatus::from_str(status)
        .ok_or_else(|| bad_request(&format!("Unknown comment status \"{status}\".")))?;
    let comment = find_comment_by_id(db, site_id, id)
        .await?
        .ok_or_else(|| not_found(&format!("Comment {id} not found.")))?;
    update_comment_status(db, site_id, id, status).await?;
    sync_post_comment_count(db, site_id, comment.post_id).await?;
    let updated = find_comment_by_id(db, site_id, id).await?;
    updated.ok_or_else(|| {
        WordlessError::new(
            "internal_error",
            500,
            "Failed to read back the updated comment.",
        )
    })
}

pub async fn delete_comment(db: &dyn Db, site_id: i64, id: i64) -> Result<bool, WordlessError> {
    let comment = find_comment_by_id(db, site_id, id)
        .await?
        .ok_or_else(|| not_found(&format!("Comment {id} not found.")))?;
    let changes = delete_comment_row(db, site_id, id).await?;
    sync_post_comment_count(db, site_id, comment.post_id).await?;
    Ok(changes > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_links_shapes() {
        // Plain URLs.
        assert_eq!(count_links("see https://example.com/a and http://b.co"), 2);
        // An <a href="https://…"> counts once, not twice.
        assert_eq!(
            count_links("click <a href=\"https://example.com\">here</a> and https://example.com"),
            1
        );
        // Two distinct anchors + one plain URL.
        assert_eq!(
            count_links("<a href='https://a.co'>a</a> <a href=\"https://b.co\">b</a> https://c.co"),
            3
        );
        // No links.
        assert_eq!(count_links("plain text, no links"), 0);
        // Case-insensitive tag/attr.
        assert_eq!(count_links("<A HREF=\"https://x.co\">x</A>"), 1);
    }

    #[test]
    fn stored_status_round_trip() {
        assert_eq!(to_stored_status(CommentStatus::Approved), "1");
        assert_eq!(to_stored_status(CommentStatus::Pending), "0");
        assert_eq!(from_stored_status("1"), CommentStatus::Approved);
        assert_eq!(from_stored_status("spam"), CommentStatus::Spam);
        assert_eq!(from_stored_status("weird"), CommentStatus::Pending);
    }
}
