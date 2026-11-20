//! Direct messaging: two-participant threads with read states.
//!
//! A thread is created on demand between two users who may message
//! each other (friends, or shared group members — see `visibility`)
//! and never across a block. Messages are plain text (max 2,000
//! chars). Read state is per-participant `last_read_at`; unread
//! counts only messages from the other participant after that mark.

use wordless_db::row::{row_int, row_opt_text, row_text};
use wordless_db::{Db, Param};
use wordless_kernel::error::{bad_request, forbidden, not_found, WordlessError};
use wordless_kernel::time::now_iso;

use crate::helpers::{are_friends, is_blocked, share_group, user_basic};
use crate::notifications;
use crate::types::{Message, Notification, Thread};
use crate::visibility::can_message;

const MAX_BODY_LEN: usize = 2000;

/// Find the existing thread between two users, or create one.
/// Refuses across a block and when the pair may not message.
pub async fn get_or_create_thread(
    db: &dyn Db,
    me: i64,
    other: i64,
) -> Result<(Thread, Option<Notification>), WordlessError> {
    if user_basic(db, other).await?.is_none() {
        return Err(not_found("User not found."));
    }
    let blocked = is_blocked(db, me, other).await?;
    let friends = are_friends(db, me, other).await?;
    let shared = share_group(db, me, other).await?;
    if !can_message(me, other, friends, shared, blocked) {
        return Err(if blocked {
            not_found("User not found.")
        } else {
            forbidden("You can message friends or members of a group you share.")
        });
    }
    if let Some(t) = find_thread(db, me, other).await? {
        return Ok((t, None));
    }
    let now = now_iso();
    let meta = db
        .exec(
            "INSERT INTO wl_message_threads (created_at, updated_at) VALUES (?, ?)",
            &[Param::Text(now.clone()), Param::Text(now.clone())],
        )
        .await?;
    let id = meta.last_row_id.unwrap_or(0);
    for uid in [me, other] {
        db.exec(
            "INSERT INTO wl_thread_participants (thread_id, user_id, last_read_at, created_at) \
             VALUES (?, ?, NULL, ?)",
            &[
                Param::Integer(id),
                Param::Integer(uid),
                Param::Text(now.clone()),
            ],
        )
        .await?;
    }
    let thread = get_thread(db, me, id)
        .await?
        .ok_or_else(|| not_found("Thread not found."))?;
    Ok((thread, None))
}

/// The thread between two users, if one exists (exactly two
/// participants, both of them).
async fn find_thread(db: &dyn Db, a: i64, b: i64) -> Result<Option<Thread>, WordlessError> {
    let row = db
        .query_one(
            "SELECT t.id FROM wl_message_threads t \
             JOIN wl_thread_participants p1 ON p1.thread_id = t.id AND p1.user_id = ? \
             JOIN wl_thread_participants p2 ON p2.thread_id = t.id AND p2.user_id = ? \
             WHERE (SELECT COUNT(*) FROM wl_thread_participants p WHERE p.thread_id = t.id) = 2 \
             LIMIT 1",
            &[Param::Integer(a), Param::Integer(b)],
        )
        .await?;
    match row {
        None => Ok(None),
        Some(r) => get_thread(db, a, row_int(&r, "id")).await,
    }
}

/// List the caller's threads, newest activity first.
pub async fn list_threads(
    db: &dyn Db,
    me: i64,
    page: i64,
    per_page: i64,
) -> Result<Vec<Thread>, WordlessError> {
    let rows = db
        .query_all(
            "SELECT t.id, t.updated_at, \
             o.user_id AS other_id, COALESCE(u.user_login, '') AS other_login, \
             COALESCE(u.display_name, '') AS other_name, \
             (SELECT m.body FROM wl_messages m WHERE m.thread_id = t.id ORDER BY m.created_at DESC, m.id DESC LIMIT 1) AS last_body, \
             (SELECT m.created_at FROM wl_messages m WHERE m.thread_id = t.id ORDER BY m.created_at DESC, m.id DESC LIMIT 1) AS last_at, \
             (SELECT COUNT(*) FROM wl_messages m WHERE m.thread_id = t.id AND m.sender_id != ? \
               AND (p.last_read_at IS NULL OR m.created_at > p.last_read_at)) AS unread \
             FROM wl_message_threads t \
             JOIN wl_thread_participants p ON p.thread_id = t.id AND p.user_id = ? \
             JOIN wl_thread_participants o ON o.thread_id = t.id AND o.user_id != ? \
             LEFT JOIN wp_users u ON u.ID = o.user_id \
             ORDER BY t.updated_at DESC, t.id DESC LIMIT ? OFFSET ?",
            &[
                Param::Integer(me),
                Param::Integer(me),
                Param::Integer(me),
                Param::Integer(per_page),
                Param::Integer((page - 1) * per_page),
            ],
        )
        .await?;
    Ok(rows.iter().map(row_to_thread).collect())
}

/// Read one thread (must be a participant) with its messages.
pub async fn get_thread(
    db: &dyn Db,
    me: i64,
    thread_id: i64,
) -> Result<Option<Thread>, WordlessError> {
    let row = db
        .query_one(
            "SELECT t.id, t.updated_at, \
             o.user_id AS other_id, COALESCE(u.user_login, '') AS other_login, \
             COALESCE(u.display_name, '') AS other_name, \
             (SELECT m.body FROM wl_messages m WHERE m.thread_id = t.id ORDER BY m.created_at DESC, m.id DESC LIMIT 1) AS last_body, \
             (SELECT m.created_at FROM wl_messages m WHERE m.thread_id = t.id ORDER BY m.created_at DESC, m.id DESC LIMIT 1) AS last_at, \
             (SELECT COUNT(*) FROM wl_messages m WHERE m.thread_id = t.id AND m.sender_id != ? \
               AND (p.last_read_at IS NULL OR m.created_at > p.last_read_at)) AS unread \
             FROM wl_message_threads t \
             JOIN wl_thread_participants p ON p.thread_id = t.id AND p.user_id = ? \
             JOIN wl_thread_participants o ON o.thread_id = t.id AND o.user_id != ? \
             LEFT JOIN wp_users u ON u.ID = o.user_id \
             WHERE t.id = ? LIMIT 1",
            &[
                Param::Integer(me),
                Param::Integer(me),
                Param::Integer(me),
                Param::Integer(thread_id),
            ],
        )
        .await?;
    Ok(row.map(|r| row_to_thread(&r)))
}

/// Messages of a thread, oldest first (must be a participant).
pub async fn list_messages(
    db: &dyn Db,
    me: i64,
    thread_id: i64,
    page: i64,
    per_page: i64,
) -> Result<Vec<Message>, WordlessError> {
    if !is_participant(db, me, thread_id).await? {
        return Err(not_found("Thread not found."));
    }
    let rows = db
        .query_all(
            "SELECT m.id, m.thread_id, m.sender_id, m.body, m.created_at, \
             COALESCE(u.display_name, '') AS sender_name \
             FROM wl_messages m LEFT JOIN wp_users u ON u.ID = m.sender_id \
             WHERE m.thread_id = ? ORDER BY m.created_at ASC, m.id ASC LIMIT ? OFFSET ?",
            &[
                Param::Integer(thread_id),
                Param::Integer(per_page),
                Param::Integer((page - 1) * per_page),
            ],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| Message {
            id: row_int(r, "id"),
            thread_id: row_int(r, "thread_id"),
            sender_id: row_int(r, "sender_id"),
            sender_name: row_text(r, "sender_name"),
            body: row_text(r, "body"),
            created_at: row_text(r, "created_at"),
        })
        .collect())
}

/// Send a message. Returns the message and the notification created
/// for the other participant (for the worker's email copy).
pub async fn send(
    db: &dyn Db,
    me: i64,
    thread_id: i64,
    body: &str,
) -> Result<(Message, Option<Notification>), WordlessError> {
    let body = body.trim();
    if body.is_empty() {
        return Err(bad_request("Message body is required."));
    }
    if body.chars().count() > MAX_BODY_LEN {
        return Err(bad_request("Message is too long."));
    }
    let thread = get_thread(db, me, thread_id)
        .await?
        .ok_or_else(|| not_found("Thread not found."))?;
    let other = thread.other_user_id;
    if is_blocked(db, me, other).await? {
        return Err(not_found("Thread not found."));
    }
    let now = now_iso();
    let meta = db
        .exec(
            "INSERT INTO wl_messages (thread_id, sender_id, body, created_at) VALUES (?, ?, ?, ?)",
            &[
                Param::Integer(thread_id),
                Param::Integer(me),
                Param::Text(body.to_string()),
                Param::Text(now.clone()),
            ],
        )
        .await?;
    let id = meta.last_row_id.unwrap_or(0);
    db.exec(
        "UPDATE wl_message_threads SET updated_at = ? WHERE id = ?",
        &[Param::Text(now.clone()), Param::Integer(thread_id)],
    )
    .await?;
    // Sending marks the thread read for the sender.
    db.exec(
        "UPDATE wl_thread_participants SET last_read_at = ? WHERE thread_id = ? AND user_id = ?",
        &[
            Param::Text(now),
            Param::Integer(thread_id),
            Param::Integer(me),
        ],
    )
    .await?;
    let notif = notifications::create(db, other, "message", me, "thread", thread_id).await?;
    let msg = Message {
        id,
        thread_id,
        sender_id: me,
        sender_name: String::new(),
        body: body.to_string(),
        created_at: now_iso(),
    };
    Ok((msg, notif))
}

/// Mark a thread read for the caller.
pub async fn mark_read(db: &dyn Db, me: i64, thread_id: i64) -> Result<(), WordlessError> {
    if !is_participant(db, me, thread_id).await? {
        return Err(not_found("Thread not found."));
    }
    db.exec(
        "UPDATE wl_thread_participants SET last_read_at = ? WHERE thread_id = ? AND user_id = ?",
        &[
            Param::Text(now_iso()),
            Param::Integer(thread_id),
            Param::Integer(me),
        ],
    )
    .await?;
    Ok(())
}

async fn is_participant(db: &dyn Db, user_id: i64, thread_id: i64) -> Result<bool, WordlessError> {
    let row = db
        .query_one(
            "SELECT 1 AS x FROM wl_thread_participants WHERE thread_id = ? AND user_id = ? LIMIT 1",
            &[Param::Integer(thread_id), Param::Integer(user_id)],
        )
        .await?;
    Ok(row.is_some())
}

fn row_to_thread(row: &wordless_db::Row) -> Thread {
    Thread {
        id: row_int(row, "id"),
        other_user_id: row_int(row, "other_id"),
        other_login: row_text(row, "other_login"),
        other_display_name: row_text(row, "other_name"),
        last_message: row_opt_text(row, "last_body"),
        last_message_at: row_opt_text(row, "last_at"),
        unread_count: row_int(row, "unread"),
        updated_at: row_text(row, "updated_at"),
    }
}
