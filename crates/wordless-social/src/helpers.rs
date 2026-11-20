//! Shared DB lookups for the social module: user basics, friendship
//! state, blocks, group membership. Every function is a small indexed
//! query; the feed queries in `activity` do their own joins so the
//! feed stays a single statement.

use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param};
use wordless_kernel::error::WordlessError;

/// Minimal `wp_users` row (id, login, display name).
pub struct UserBasic {
    pub id: i64,
    pub login: String,
    pub display_name: String,
    pub email: String,
}

pub async fn user_basic(db: &dyn Db, user_id: i64) -> Result<Option<UserBasic>, WordlessError> {
    let row = db
        .query_one(
            "SELECT ID, user_login, display_name, user_email FROM wp_users WHERE ID = ? LIMIT 1",
            &[Param::Integer(user_id)],
        )
        .await?;
    Ok(row.map(|r| UserBasic {
        id: row_int(&r, "ID"),
        login: row_text(&r, "user_login"),
        display_name: row_text(&r, "display_name"),
        email: row_text(&r, "user_email"),
    }))
}

pub async fn user_id_by_login(db: &dyn Db, login: &str) -> Result<Option<i64>, WordlessError> {
    let row = db
        .query_one(
            "SELECT ID FROM wp_users WHERE user_login = ? LIMIT 1",
            &[Param::Text(login.to_string())],
        )
        .await?;
    Ok(row.map(|r| row_int(&r, "ID")))
}

/// True when either user has blocked the other (a `blocked` row in
/// either direction). Never returns an error for a missing table in
/// tests that only apply 0001 — callers run after migration 0013.
pub async fn is_blocked(db: &dyn Db, a: i64, b: i64) -> Result<bool, WordlessError> {
    let row = db
        .query_one(
            "SELECT 1 AS x FROM wl_friendships WHERE status = 'blocked' \
             AND ((requester_id = ? AND addressee_id = ?) \
             OR (requester_id = ? AND addressee_id = ?)) LIMIT 1",
            &[
                Param::Integer(a),
                Param::Integer(b),
                Param::Integer(b),
                Param::Integer(a),
            ],
        )
        .await?;
    Ok(row.is_some())
}

/// True when the two users are mutual accepted friends.
pub async fn are_friends(db: &dyn Db, a: i64, b: i64) -> Result<bool, WordlessError> {
    let row = db
        .query_one(
            "SELECT 1 AS x FROM wl_friendships WHERE status = 'accepted' \
             AND ((requester_id = ? AND addressee_id = ?) \
             OR (requester_id = ? AND addressee_id = ?)) LIMIT 1",
            &[
                Param::Integer(a),
                Param::Integer(b),
                Param::Integer(b),
                Param::Integer(a),
            ],
        )
        .await?;
    Ok(row.is_some())
}

/// Friendship state from `viewer`'s perspective toward `other`:
/// `none`, `pending_sent`, `pending_received`, `accepted`, `blocked`.
pub async fn friend_status(db: &dyn Db, viewer: i64, other: i64) -> Result<String, WordlessError> {
    let row = db
        .query_one(
            "SELECT requester_id, status FROM wl_friendships \
             WHERE (requester_id = ? AND addressee_id = ?) \
             OR (requester_id = ? AND addressee_id = ?) LIMIT 1",
            &[
                Param::Integer(viewer),
                Param::Integer(other),
                Param::Integer(other),
                Param::Integer(viewer),
            ],
        )
        .await?;
    Ok(match row {
        None => "none".to_string(),
        Some(r) => {
            let status = row_text(&r, "status");
            if status == "blocked" {
                return Ok("blocked".to_string());
            }
            if status == "accepted" {
                return Ok("accepted".to_string());
            }
            if row_int(&r, "requester_id") == viewer {
                "pending_sent".to_string()
            } else {
                "pending_received".to_string()
            }
        }
    })
}

/// (role, status) of a user's membership of a group, if any.
pub async fn group_membership(
    db: &dyn Db,
    group_id: i64,
    user_id: i64,
) -> Result<Option<(String, String)>, WordlessError> {
    let row = db
        .query_one(
            "SELECT role, status FROM wl_group_members WHERE group_id = ? AND user_id = ? LIMIT 1",
            &[Param::Integer(group_id), Param::Integer(user_id)],
        )
        .await?;
    Ok(row.map(|r| (row_text(&r, "role"), row_text(&r, "status"))))
}

/// True when both users are active members of at least one common group.
pub async fn share_group(db: &dyn Db, a: i64, b: i64) -> Result<bool, WordlessError> {
    let row = db
        .query_one(
            "SELECT 1 AS x FROM wl_group_members m1 \
             JOIN wl_group_members m2 ON m2.group_id = m1.group_id \
             WHERE m1.user_id = ? AND m2.user_id = ? \
             AND m1.status = 'member' AND m2.status = 'member' LIMIT 1",
            &[Param::Integer(a), Param::Integer(b)],
        )
        .await?;
    Ok(row.is_some())
}
