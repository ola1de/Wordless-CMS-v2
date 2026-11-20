//! Activity log — append-only audit trail in `wl_audit_log`.
//! Port of v1 `src/modules/activity/service.ts`.
//!
//! NOTE: the table is `wl_audit_log`, not `wl_activity_log` — that is
//! the v1 name (migration 0001) and the compatibility contract keeps it.
//! Every administrative mutation (posts, comments, terms, menus, auth)
//! should log here with the acting user id (0 = anonymous / system).
//! Reads are admin-only (manage_options); see the /api/activity route.

use serde::Serialize;
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::WordlessError;
use wordless_kernel::time::now_iso;

use crate::common::{count_total, page_of, per_page_of, total_pages};

/// An audit-log row, serialised with the v1 camelCase field names.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    pub id: i64,
    /// 0 = anonymous visitor / system.
    pub actor_id: i64,
    pub action: String,
    pub object_type: Option<String>,
    pub object_id: Option<String>,
    pub details: Option<String>,
    pub created_at: String,
}

pub fn row_to_entry(row: &Row) -> ActivityEntry {
    ActivityEntry {
        id: row_int(row, "id"),
        actor_id: row_int(row, "user_id"),
        action: row_text(row, "action"),
        object_type: wordless_db::row::row_opt_text(row, "object_type"),
        object_id: wordless_db::row::row_opt_text(row, "object_id"),
        details: wordless_db::row::row_opt_text(row, "details"),
        created_at: row_text(row, "created_at"),
    }
}

#[derive(Debug, Clone, Default)]
pub struct LogActivityInput {
    pub actor_id: i64,
    pub action: String,
    pub object_type: Option<String>,
    pub object_id: Option<String>,
    pub details: Option<String>,
    pub ip_hash: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ListActivityOptions {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityListResult {
    pub entries: Vec<ActivityEntry>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

/// Action names follow the pattern `<object>.<verb>` (e.g. `post.create`).
pub async fn log_activity(
    db: &dyn Db,
    input: LogActivityInput,
) -> Result<ActivityEntry, WordlessError> {
    let now = now_iso();
    let meta = db
        .prepare(
            "INSERT INTO wl_audit_log (user_id, action, object_type, object_id, details, ip_hash, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(vec![
            Param::Integer(input.actor_id),
            Param::Text(input.action),
            Param::from(input.object_type),
            Param::from(input.object_id),
            Param::from(input.details),
            Param::from(input.ip_hash),
            Param::Text(now),
        ])
        .run()
        .await?;
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_audit_log WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(meta.last_row_id.unwrap_or(0))])
        .first()
        .await?;
    row.map(|r| row_to_entry(&r)).ok_or_else(|| {
        WordlessError::new(
            "internal_error",
            500,
            "Failed to read back the audit log entry.",
        )
    })
}

/// Best-effort audit logging for route handlers, mirroring the `log()`
/// helper in v1 `src/index.ts`: audit logging must never break the
/// request it records, so failures are swallowed. Returns the entry
/// when the insert succeeded, `None` otherwise.
pub async fn log(
    db: &dyn Db,
    actor_id: i64,
    action: &str,
    object_type: &str,
    object_id: i64,
) -> Option<ActivityEntry> {
    log_activity(
        db,
        LogActivityInput {
            actor_id,
            action: action.to_string(),
            object_type: Some(object_type.to_string()),
            object_id: Some(object_id.to_string()),
            details: None,
            ip_hash: None,
        },
    )
    .await
    .ok()
}

pub async fn list_activity(
    db: &dyn Db,
    opts: ListActivityOptions,
) -> Result<ActivityListResult, WordlessError> {
    let page = page_of(opts.page);
    let per_page = per_page_of(opts.per_page);
    let total_row: Option<Row> = db
        .prepare("SELECT COUNT(*) AS total FROM wl_audit_log")
        .bind(vec![])
        .first()
        .await?;
    let total = count_total(&total_row);
    let total_pages = total_pages(total, per_page);
    let rows: Vec<Row> = db
        .prepare("SELECT * FROM wl_audit_log ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?")
        .bind(vec![
            Param::Integer(per_page),
            Param::Integer((page - 1) * per_page),
        ])
        .all()
        .await?;
    Ok(ActivityListResult {
        entries: rows.iter().map(row_to_entry).collect(),
        total,
        page,
        per_page,
        total_pages,
    })
}
