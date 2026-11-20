//! In-app notifications. Every notification is a `wl_notifications`
//! row; the worker sends the email copy best-effort through the
//! integrations mailer (mock provider only logs — never presented as
//! delivered). Types: `friend_request`, `friend_accepted`,
//! `activity_like`, `activity_mention`, `group_join_request`,
//! `group_approved`, `group_invite`, `message`.

use wordless_db::row::{row_int, row_opt_text, row_text};
use wordless_db::{Db, Param};
use wordless_kernel::error::WordlessError;
use wordless_kernel::time::now_iso;

use crate::helpers::user_basic;
use crate::types::Notification;

/// Create one notification row (no-op when `user_id` is 0 or the
/// recipient is the actor — nobody notifies themselves).
pub async fn create(
    db: &dyn Db,
    user_id: i64,
    notif_type: &str,
    actor_id: i64,
    object_type: &str,
    object_id: i64,
) -> Result<Option<Notification>, WordlessError> {
    if user_id == 0 || user_id == actor_id {
        return Ok(None);
    }
    let now = now_iso();
    let meta = db
        .exec(
            "INSERT INTO wl_notifications (user_id, type, actor_id, object_type, object_id, created_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
            &[
                Param::Integer(user_id),
                Param::Text(notif_type.to_string()),
                Param::Integer(actor_id),
                Param::Text(object_type.to_string()),
                Param::Integer(object_id),
                Param::Text(now),
            ],
        )
        .await?;
    let id = meta.last_row_id.unwrap_or(0);
    get(db, id).await
}

pub async fn get(db: &dyn Db, id: i64) -> Result<Option<Notification>, WordlessError> {
    let row = db
        .query_one(
            "SELECT n.*, COALESCE(u.display_name, '') AS actor_name \
             FROM wl_notifications n LEFT JOIN wp_users u ON u.ID = n.actor_id \
             WHERE n.id = ? LIMIT 1",
            &[Param::Integer(id)],
        )
        .await?;
    Ok(row.map(|r| row_to_notification(&r)))
}

/// List a user's notifications, newest first. `unread_only` filters
/// to unread; returns (items, unread_count, total).
pub async fn list(
    db: &dyn Db,
    user_id: i64,
    unread_only: bool,
    page: i64,
    per_page: i64,
) -> Result<(Vec<Notification>, i64, i64), WordlessError> {
    let unread = unread_count(db, user_id).await?;
    let total_row = db
        .query_one(
            if unread_only {
                "SELECT COUNT(*) AS c FROM wl_notifications WHERE user_id = ? AND read_at IS NULL"
            } else {
                "SELECT COUNT(*) AS c FROM wl_notifications WHERE user_id = ?"
            },
            &[Param::Integer(user_id)],
        )
        .await?;
    let total = total_row.map(|r| row_int(&r, "c")).unwrap_or(0);
    let sql = if unread_only {
        "SELECT n.*, COALESCE(u.display_name, '') AS actor_name \
         FROM wl_notifications n LEFT JOIN wp_users u ON u.ID = n.actor_id \
         WHERE n.user_id = ? AND n.read_at IS NULL \
         ORDER BY n.created_at DESC, n.id DESC LIMIT ? OFFSET ?"
    } else {
        "SELECT n.*, COALESCE(u.display_name, '') AS actor_name \
         FROM wl_notifications n LEFT JOIN wp_users u ON u.ID = n.actor_id \
         WHERE n.user_id = ? ORDER BY n.created_at DESC, n.id DESC LIMIT ? OFFSET ?"
    };
    let rows = db
        .query_all(
            sql,
            &[
                Param::Integer(user_id),
                Param::Integer(per_page),
                Param::Integer((page - 1) * per_page),
            ],
        )
        .await?;
    Ok((
        rows.iter().map(row_to_notification).collect(),
        unread,
        total,
    ))
}

pub async fn unread_count(db: &dyn Db, user_id: i64) -> Result<i64, WordlessError> {
    let row = db
        .query_one(
            "SELECT COUNT(*) AS c FROM wl_notifications WHERE user_id = ? AND read_at IS NULL",
            &[Param::Integer(user_id)],
        )
        .await?;
    Ok(row.map(|r| row_int(&r, "c")).unwrap_or(0))
}

/// Mark one notification read (only when it belongs to `user_id`).
/// Returns true when a row changed.
pub async fn mark_read(db: &dyn Db, user_id: i64, id: i64) -> Result<bool, WordlessError> {
    let meta = db
        .exec(
            "UPDATE wl_notifications SET read_at = ? WHERE id = ? AND user_id = ? AND read_at IS NULL",
            &[
                Param::Text(now_iso()),
                Param::Integer(id),
                Param::Integer(user_id),
            ],
        )
        .await?;
    Ok(meta.changes > 0)
}

/// Mark all of a user's notifications read. Returns rows changed.
pub async fn mark_all_read(db: &dyn Db, user_id: i64) -> Result<i64, WordlessError> {
    let meta = db
        .exec(
            "UPDATE wl_notifications SET read_at = ? WHERE user_id = ? AND read_at IS NULL",
            &[Param::Text(now_iso()), Param::Integer(user_id)],
        )
        .await?;
    Ok(meta.changes as i64)
}

fn row_to_notification(row: &wordless_db::Row) -> Notification {
    Notification {
        id: row_int(row, "id"),
        user_id: row_int(row, "user_id"),
        notif_type: row_text(row, "type"),
        actor_id: row_int(row, "actor_id"),
        actor_name: row_text(row, "actor_name"),
        object_type: row_text(row, "object_type"),
        object_id: row_int(row, "object_id"),
        read_at: row_opt_text(row, "read_at"),
        created_at: row_text(row, "created_at"),
    }
}

/// Look up a notification recipient's email (for the worker's
/// best-effort email copy). Returns None when the user is missing.
pub async fn recipient_email(db: &dyn Db, user_id: i64) -> Result<Option<String>, WordlessError> {
    Ok(user_basic(db, user_id).await?.map(|u| u.email))
}
