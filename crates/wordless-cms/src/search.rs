//! Search service (v1) — port of v1 `src/modules/search/service.ts`.
//!
//! LIKE-based matching on post_title / post_content, case-insensitive
//! (SQLite LIKE is case-insensitive for ASCII). Title matches rank first,
//! then newest first. Only published posts and pages are searchable.
//!
//! NOTE: this is NOT full-text search. LIKE scans the posts table, so it
//! is fine for small sites and slow for very large ones; an FTS5 index is
//! a later optimisation. Do not describe this as full-text.

use serde::Serialize;
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, WordlessError};

use crate::common::{count_total, page_of, per_page_of, total_pages};
use crate::posts::{row_to_post, Post};

#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    pub post_type: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub query: String,
    pub posts: Vec<Post>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

/// Escape LIKE wildcards so a query matches literally.
pub fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

pub async fn search_posts(
    db: &dyn Db,
    site_id: i64,
    query: &str,
    opts: SearchOptions,
) -> Result<SearchResult, WordlessError> {
    let q = query.trim().to_string();
    if q.is_empty() || q.len() > 200 {
        return Err(bad_request(
            "Query parameter \"q\" is required and must be 1–200 characters.",
        ));
    }
    let page = page_of(opts.page);
    let per_page = per_page_of(opts.per_page);
    let like = format!("%{}%", escape_like(&q));
    let type_filter = if opts.post_type.is_some() {
        " AND post_type = ?"
    } else {
        ""
    };
    let mut type_params: Vec<Param> = Vec::new();
    if let Some(t) = &opts.post_type {
        type_params.push(Param::Text(t.to_lowercase()));
    }

    let where_clause = format!(
        "WHERE site_id = ? AND post_status = 'publish' AND post_type IN ('post', 'page'){type_filter} AND (post_title LIKE ? ESCAPE '\\' OR post_content LIKE ? ESCAPE '\\')"
    );
    let mut count_params = vec![Param::Integer(site_id)];
    count_params.extend(type_params.clone());
    count_params.push(Param::Text(like.clone()));
    count_params.push(Param::Text(like.clone()));
    let count_row: Option<Row> = db
        .prepare(&format!(
            "SELECT COUNT(*) AS total FROM wp_posts {where_clause}"
        ))
        .bind(count_params)
        .first()
        .await?;
    let total = count_total(&count_row);
    let total_pages = total_pages(total, per_page);

    let mut row_params = vec![Param::Integer(site_id)];
    row_params.extend(type_params);
    row_params.push(Param::Text(like.clone()));
    row_params.push(Param::Text(like.clone()));
    row_params.push(Param::Text(like));
    row_params.push(Param::Integer(per_page));
    row_params.push(Param::Integer((page - 1) * per_page));
    let rows: Vec<Row> = db
        .prepare(&format!(
            "SELECT * FROM wp_posts {where_clause}
              ORDER BY CASE WHEN post_title LIKE ? ESCAPE '\\' THEN 0 ELSE 1 END,
                       post_date DESC, ID DESC
              LIMIT ? OFFSET ?"
        ))
        .bind(row_params)
        .all()
        .await?;
    Ok(SearchResult {
        query: q,
        posts: rows.iter().map(row_to_post).collect(),
        total,
        page,
        per_page,
        total_pages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_like_shapes() {
        assert_eq!(escape_like("100%_x\\y"), "100\\%\\_x\\\\y");
        assert_eq!(escape_like("plain"), "plain");
    }
}
