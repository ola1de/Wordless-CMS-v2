//! Friends — mutual, request/accept (Q11, BuddyPress model).
//!
//! State machine on `wl_friendships` (one row per unordered pair,
//! keyed by the requester): `pending` → `accepted` (addressee
//! accepts) or row deleted (decline / remove). `blocked` overwrites
//! any state in either direction and hides both users from each
//! other (see `visibility`); unblocking deletes the row.

use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param};
use wordless_kernel::error::{bad_request, conflict, not_found, WordlessError};
use wordless_kernel::time::now_iso;

use crate::helpers::{are_friends, friend_status, is_blocked, user_basic};
use crate::notifications;
use crate::types::{FriendEntry, Notification};

/// List friends + pending requests for `user_id`. `pending_only`
/// returns just the requests awaiting this user (incoming) plus the
/// ones they sent (outgoing, marked `incoming: false`).
pub async fn list_friends(
    db: &dyn Db,
    user_id: i64,
    pending_only: bool,
) -> Result<Vec<FriendEntry>, WordlessError> {
    let sql = if pending_only {
        "SELECT f.requester_id, f.addressee_id, f.status, f.created_at, \
         u.ID, u.user_login, u.display_name \
         FROM wl_friendships f \
         JOIN wp_users u ON u.ID = CASE WHEN f.requester_id = ? THEN f.addressee_id ELSE f.requester_id END \
         WHERE f.status = 'pending' AND (f.requester_id = ? OR f.addressee_id = ?) \
         ORDER BY f.created_at DESC"
    } else {
        "SELECT f.requester_id, f.addressee_id, f.status, f.created_at, \
         u.ID, u.user_login, u.display_name \
         FROM wl_friendships f \
         JOIN wp_users u ON u.ID = CASE WHEN f.requester_id = ? THEN f.addressee_id ELSE f.requester_id END \
         WHERE f.status IN ('pending', 'accepted') AND (f.requester_id = ? OR f.addressee_id = ?) \
         ORDER BY f.status ASC, f.created_at DESC"
    };
    let rows = db
        .query_all(
            sql,
            &[
                Param::Integer(user_id),
                Param::Integer(user_id),
                Param::Integer(user_id),
            ],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| {
            let requester = row_int(r, "requester_id");
            FriendEntry {
                user_id: row_int(r, "ID"),
                login: row_text(r, "user_login"),
                display_name: row_text(r, "display_name"),
                status: row_text(r, "status"),
                incoming: requester != user_id,
                created_at: row_text(r, "created_at"),
            }
        })
        .collect())
}

/// Send a friend request from `from` to `to`. Returns the created
/// notification for the addressee (for the worker's email copy).
pub async fn request(
    db: &dyn Db,
    from: i64,
    to: i64,
) -> Result<Option<Notification>, WordlessError> {
    if from == to {
        return Err(bad_request("You cannot friend yourself."));
    }
    if user_basic(db, to).await?.is_none() {
        return Err(not_found("User not found."));
    }
    if is_blocked(db, from, to).await? {
        // Indistinguishable from a missing user — do not leak the block.
        return Err(not_found("User not found."));
    }
    match friend_status(db, from, to).await?.as_str() {
        "accepted" => return Err(conflict("You are already friends.", "already_friends")),
        "pending_sent" => return Err(conflict("Friend request already sent.", "already_sent")),
        "pending_received" => {
            // They already asked — accept instead of duplicating.
            accept(db, from, to).await?;
            return Ok(None);
        }
        _ => {}
    }
    let now = now_iso();
    db.exec(
        "INSERT INTO wl_friendships (requester_id, addressee_id, status, created_at, updated_at) \
         VALUES (?, ?, 'pending', ?, ?)",
        &[
            Param::Integer(from),
            Param::Integer(to),
            Param::Text(now.clone()),
            Param::Text(now),
        ],
    )
    .await?;
    notifications::create(db, to, "friend_request", from, "friendship", from).await
}

/// Accept a pending request: `me` accepts the request sent by `other`.
pub async fn accept(
    db: &dyn Db,
    me: i64,
    other: i64,
) -> Result<Option<Notification>, WordlessError> {
    let row = db
        .query_one(
            "SELECT id FROM wl_friendships WHERE requester_id = ? AND addressee_id = ? AND status = 'pending' LIMIT 1",
            &[Param::Integer(other), Param::Integer(me)],
        )
        .await?;
    let Some(r) = row else {
        return Err(not_found("No pending friend request from this user."));
    };
    let id = row_int(&r, "id");
    db.exec(
        "UPDATE wl_friendships SET status = 'accepted', updated_at = ? WHERE id = ?",
        &[Param::Text(now_iso()), Param::Integer(id)],
    )
    .await?;
    notifications::create(db, other, "friend_accepted", me, "friendship", me).await
}

/// Decline a pending request (deletes the row).
pub async fn decline(db: &dyn Db, me: i64, other: i64) -> Result<(), WordlessError> {
    let meta = db
        .exec(
            "DELETE FROM wl_friendships WHERE requester_id = ? AND addressee_id = ? AND status = 'pending'",
            &[Param::Integer(other), Param::Integer(me)],
        )
        .await?;
    if meta.changes == 0 {
        return Err(not_found("No pending friend request from this user."));
    }
    Ok(())
}

/// Remove a friend (deletes an accepted row, either direction).
pub async fn remove(db: &dyn Db, me: i64, other: i64) -> Result<(), WordlessError> {
    let meta = db
        .exec(
            "DELETE FROM wl_friendships WHERE status = 'accepted' \
             AND ((requester_id = ? AND addressee_id = ?) OR (requester_id = ? AND addressee_id = ?))",
            &[
                Param::Integer(me),
                Param::Integer(other),
                Param::Integer(other),
                Param::Integer(me),
            ],
        )
        .await?;
    if meta.changes == 0 {
        return Err(not_found("You are not friends with this user."));
    }
    Ok(())
}

/// Block `other`: upserts a `blocked` row from `me`, replacing any
/// pending/accepted row in either direction. Also removes `me` from
/// any shared group threads is out of scope — messaging is refused
/// by the block check itself.
pub async fn block(db: &dyn Db, me: i64, other: i64) -> Result<(), WordlessError> {
    if me == other {
        return Err(bad_request("You cannot block yourself."));
    }
    if user_basic(db, other).await?.is_none() {
        return Err(not_found("User not found."));
    }
    let now = now_iso();
    db.exec(
        "DELETE FROM wl_friendships \
         WHERE (requester_id = ? AND addressee_id = ?) OR (requester_id = ? AND addressee_id = ?)",
        &[
            Param::Integer(me),
            Param::Integer(other),
            Param::Integer(other),
            Param::Integer(me),
        ],
    )
    .await?;
    db.exec(
        "INSERT INTO wl_friendships (requester_id, addressee_id, status, created_at, updated_at) \
         VALUES (?, ?, 'blocked', ?, ?)",
        &[
            Param::Integer(me),
            Param::Integer(other),
            Param::Text(now.clone()),
            Param::Text(now),
        ],
    )
    .await?;
    Ok(())
}

/// Unblock: deletes the `blocked` row `me` created. Returns an error
/// when `me` never blocked `other` (so a block cannot be probed).
pub async fn unblock(db: &dyn Db, me: i64, other: i64) -> Result<(), WordlessError> {
    let meta = db
        .exec(
            "DELETE FROM wl_friendships WHERE requester_id = ? AND addressee_id = ? AND status = 'blocked'",
            &[Param::Integer(me), Param::Integer(other)],
        )
        .await?;
    if meta.changes == 0 {
        return Err(not_found("No block to remove."));
    }
    Ok(())
}

/// Guard helper for routes: refuse when the pair is blocked or the
/// target user does not exist. Returns the target's basic row.
pub async fn require_unblocked_target(
    db: &dyn Db,
    me: i64,
    other: i64,
) -> Result<(), WordlessError> {
    if user_basic(db, other).await?.is_none() || is_blocked(db, me, other).await? {
        return Err(not_found("User not found."));
    }
    Ok(())
}

/// Are these two users friends right now (accepted, either direction)?
pub async fn is_friend(db: &dyn Db, a: i64, b: i64) -> Result<bool, WordlessError> {
    are_friends(db, a, b).await
}

/// List the users `user_id` has blocked.
pub async fn list_blocked(db: &dyn Db, user_id: i64) -> Result<Vec<FriendEntry>, WordlessError> {
    let rows = db
        .query_all(
            "SELECT f.addressee_id AS other_id, u.user_login, u.display_name, f.created_at \
             FROM wl_friendships f JOIN wp_users u ON u.ID = f.addressee_id \
             WHERE f.requester_id = ? AND f.status = 'blocked' ORDER BY f.created_at DESC",
            &[Param::Integer(user_id)],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| FriendEntry {
            user_id: row_int(r, "other_id"),
            login: row_text(r, "user_login"),
            display_name: row_text(r, "display_name"),
            status: "blocked".to_string(),
            incoming: false,
            created_at: row_text(r, "created_at"),
        })
        .collect())
}

/// A moderator/admin removes a friendship or block between two users.
pub async fn admin_remove(db: &dyn Db, a: i64, b: i64) -> Result<bool, WordlessError> {
    let meta = db
        .exec(
            "DELETE FROM wl_friendships \
             WHERE (requester_id = ? AND addressee_id = ?) OR (requester_id = ? AND addressee_id = ?)",
            &[
                Param::Integer(a),
                Param::Integer(b),
                Param::Integer(b),
                Param::Integer(a),
            ],
        )
        .await?;
    Ok(meta.changes > 0)
}
