//! Activity feed: posts, likes, mentions, group activity.
//!
//! Feed visibility is enforced in SQL (one statement, indexed — D1
//! row limits apply): an item is visible to the viewer when it is
//! `public`; `friends` and the viewer is the actor's friend; `group`
//! and the viewer is a member of the item's group; `private` only to
//! the actor. Blocked actors never appear; hidden/deleted items never
//! appear in any feed (the admin moderation list reads them directly).
//!
//! Mentions: `@login` tokens in posted content notify the mentioned
//! user (once per item) and create a `mention` activity item is NOT
//! created — the notification carries it. Only existing, unblocked
//! users are notified.

use wordless_db::row::{row_int, row_opt_text, row_text};
use wordless_db::{Db, Param};
use wordless_kernel::error::{bad_request, forbidden, not_found, WordlessError};
use wordless_kernel::time::now_iso;

use crate::helpers::{group_membership, is_blocked, user_id_by_login};
use crate::notifications;
use crate::types::{ActivityItem, Notification};

const MAX_CONTENT_LEN: usize = 5000;

/// Post an activity item. `visibility` must be public/friends/group/
/// private; `group` items require `group_id` and membership of that
/// group (the item's visibility is forced to `group`).
pub async fn post(
    db: &dyn Db,
    actor: i64,
    content: &str,
    visibility: &str,
    group_id: Option<i64>,
) -> Result<(ActivityItem, Vec<Notification>), WordlessError> {
    let content = content.trim();
    if content.is_empty() {
        return Err(bad_request("Activity content is required."));
    }
    if content.chars().count() > MAX_CONTENT_LEN {
        return Err(bad_request("Activity content is too long."));
    }
    let visibility = match (visibility, group_id) {
        (_, Some(_)) => "group",
        ("public" | "friends" | "private", None) => visibility,
        _ => return Err(bad_request("Invalid visibility.")),
    };
    if let Some(gid) = group_id {
        match group_membership(db, gid, actor).await? {
            Some((_, s)) if s == "member" => {}
            _ => return Err(forbidden("You are not a member of this group.")),
        }
        let grow = db
            .query_one(
                "SELECT 1 AS x FROM wl_groups WHERE id = ? LIMIT 1",
                &[Param::Integer(gid)],
            )
            .await?;
        if grow.is_none() {
            return Err(not_found("Group not found."));
        }
    }
    let now = now_iso();
    let meta = db
        .exec(
            "INSERT INTO wl_activity_items \
             (actor_id, type, object_type, object_id, content, visibility, group_id, status, created_at, updated_at) \
             VALUES (?, 'post', '', 0, ?, ?, ?, 'visible', ?, ?)",
            &[
                Param::Integer(actor),
                Param::Text(content.to_string()),
                Param::Text(visibility.to_string()),
                match group_id {
                    Some(g) => Param::Integer(g),
                    None => Param::Null,
                },
                Param::Text(now.clone()),
                Param::Text(now),
            ],
        )
        .await?;
    let id = meta.last_row_id.unwrap_or(0);
    let item = get_item(db, id, actor)
        .await?
        .ok_or_else(|| not_found("Activity item not found."))?;
    let mut notifs = Vec::new();
    for mentioned in find_mentions(content) {
        if let Some(uid) = user_id_by_login(db, &mentioned).await? {
            if uid == actor || is_blocked(db, actor, uid).await? {
                continue;
            }
            if let Some(n) =
                notifications::create(db, uid, "activity_mention", actor, "activity", id).await?
            {
                notifs.push(n);
            }
        }
    }
    Ok((item, notifs))
}

/// `GET /api/social/activity` — the viewer's feed: friends, public,
/// own, and group memberships, newest first. `actor_id` narrows to one
/// member's profile feed (visibility still enforced).
pub async fn feed(
    db: &dyn Db,
    viewer: i64,
    actor_id: Option<i64>,
    page: i64,
    per_page: i64,
) -> Result<Vec<ActivityItem>, WordlessError> {
    let mut sql = String::from(
        "SELECT a.id, a.actor_id, a.type, a.object_type, a.object_id, a.content, \
         a.visibility, a.group_id, a.status, a.created_at, a.updated_at, \
         COALESCE(u.display_name, '') AS actor_name, COALESCE(u.user_login, '') AS actor_login, \
         COALESCE(g.name, '') AS group_name, \
         (SELECT COUNT(*) FROM wl_activity_likes l WHERE l.item_id = a.id) AS like_count, \
         EXISTS (SELECT 1 FROM wl_activity_likes l2 WHERE l2.item_id = a.id AND l2.user_id = ?) AS liked \
         FROM wl_activity_items a \
         JOIN wp_users u ON u.ID = a.actor_id \
         LEFT JOIN wl_groups g ON g.id = a.group_id \
         WHERE a.status = 'visible' \
         AND NOT EXISTS (SELECT 1 FROM wl_friendships b WHERE b.status = 'blocked' \
           AND ((b.requester_id = ? AND b.addressee_id = a.actor_id) \
           OR (b.requester_id = a.actor_id AND b.addressee_id = ?))) \
         AND (a.visibility = 'public' OR a.actor_id = ? \
           OR (a.visibility = 'friends' AND EXISTS (SELECT 1 FROM wl_friendships f \
             WHERE f.status = 'accepted' \
             AND ((f.requester_id = ? AND f.addressee_id = a.actor_id) \
             OR (f.requester_id = a.actor_id AND f.addressee_id = ?)))) \
           OR (a.visibility = 'group' AND a.group_id IS NOT NULL \
             AND EXISTS (SELECT 1 FROM wl_group_members gm \
             WHERE gm.group_id = a.group_id AND gm.user_id = ? AND gm.status = 'member')))",
    );
    let mut params: Vec<Param> = vec![
        Param::Integer(viewer),
        Param::Integer(viewer),
        Param::Integer(viewer),
        Param::Integer(viewer),
        Param::Integer(viewer),
        Param::Integer(viewer),
        Param::Integer(viewer),
    ];
    if let Some(aid) = actor_id {
        sql.push_str(" AND a.actor_id = ?");
        params.push(Param::Integer(aid));
    }
    sql.push_str(" ORDER BY a.created_at DESC, a.id DESC LIMIT ? OFFSET ?");
    params.push(Param::Integer(per_page));
    params.push(Param::Integer((page - 1) * per_page));
    let rows = db.query_all(&sql, &params).await?;
    Ok(rows.iter().map(|r| row_to_item(r, viewer)).collect())
}

/// `GET /api/social/groups/:id/activity` — one group's activity.
/// Public groups show to anyone; private/hidden to members/admins.
pub async fn group_feed(
    db: &dyn Db,
    viewer: i64,
    group_id: i64,
    page: i64,
    per_page: i64,
    is_admin: bool,
) -> Result<Vec<ActivityItem>, WordlessError> {
    let grow = db
        .query_one(
            "SELECT visibility FROM wl_groups WHERE id = ? LIMIT 1",
            &[Param::Integer(group_id)],
        )
        .await?;
    let Some(g) = grow else {
        return Err(not_found("Group not found."));
    };
    let visibility = row_text(&g, "visibility");
    let member = group_membership(db, group_id, viewer).await?;
    let is_member = matches!(member, Some((_, ref s)) if s == "member");
    if !crate::visibility::can_view_group_content(&visibility, is_member, is_admin) {
        return Err(forbidden("This group's activity is members-only."));
    }
    let rows = db
        .query_all(
            "SELECT a.id, a.actor_id, a.type, a.object_type, a.object_id, a.content, \
             a.visibility, a.group_id, a.status, a.created_at, a.updated_at, \
             COALESCE(u.display_name, '') AS actor_name, COALESCE(u.user_login, '') AS actor_login, \
             COALESCE(g.name, '') AS group_name, \
             (SELECT COUNT(*) FROM wl_activity_likes l WHERE l.item_id = a.id) AS like_count, \
             EXISTS (SELECT 1 FROM wl_activity_likes l2 WHERE l2.item_id = a.id AND l2.user_id = ?) AS liked \
             FROM wl_activity_items a \
             JOIN wp_users u ON u.ID = a.actor_id \
             LEFT JOIN wl_groups g ON g.id = a.group_id \
             WHERE a.status = 'visible' AND a.group_id = ? \
             AND NOT EXISTS (SELECT 1 FROM wl_friendships b WHERE b.status = 'blocked' \
               AND ((b.requester_id = ? AND b.addressee_id = a.actor_id) \
               OR (b.requester_id = a.actor_id AND b.addressee_id = ?))) \
             ORDER BY a.created_at DESC, a.id DESC LIMIT ? OFFSET ?",
            &[
                Param::Integer(viewer),
                Param::Integer(group_id),
                Param::Integer(viewer),
                Param::Integer(viewer),
                Param::Integer(per_page),
                Param::Integer((page - 1) * per_page),
            ],
        )
        .await?;
    Ok(rows.iter().map(|r| row_to_item(r, viewer)).collect())
}

/// Like an item (idempotent — a second like is a no-op success).
/// Returns the notification for the actor, if one was created.
pub async fn like(
    db: &dyn Db,
    user_id: i64,
    item_id: i64,
) -> Result<Option<Notification>, WordlessError> {
    let item = get_item(db, item_id, user_id)
        .await?
        .ok_or_else(|| not_found("Activity item not found."))?;
    if item.status != "visible" {
        return Err(not_found("Activity item not found."));
    }
    if is_blocked(db, user_id, item.actor_id).await? {
        return Err(not_found("Activity item not found."));
    }
    // Liking requires being able to see the item: re-check visibility
    // through the feed query (a viewer who cannot see it cannot like it).
    let visible = feed_contains(db, user_id, item_id).await?;
    if !visible {
        return Err(forbidden("You cannot like this item."));
    }
    let now = now_iso();
    db.exec(
        "INSERT OR IGNORE INTO wl_activity_likes (item_id, user_id, created_at) VALUES (?, ?, ?)",
        &[
            Param::Integer(item_id),
            Param::Integer(user_id),
            Param::Text(now),
        ],
    )
    .await?;
    notifications::create(
        db,
        item.actor_id,
        "activity_like",
        user_id,
        "activity",
        item_id,
    )
    .await
}

/// Unlike an item (idempotent).
pub async fn unlike(db: &dyn Db, user_id: i64, item_id: i64) -> Result<(), WordlessError> {
    let item = get_item(db, item_id, user_id)
        .await?
        .ok_or_else(|| not_found("Activity item not found."))?;
    if item.status != "visible" {
        return Err(not_found("Activity item not found."));
    }
    db.exec(
        "DELETE FROM wl_activity_likes WHERE item_id = ? AND user_id = ?",
        &[Param::Integer(item_id), Param::Integer(user_id)],
    )
    .await?;
    Ok(())
}

/// Moderator hide: marks the item hidden everywhere it appeared
/// (feed, profile, group). Only the actor's own delete is separate.
pub async fn hide(
    db: &dyn Db,
    moderator: i64,
    item_id: i64,
) -> Result<ActivityItem, WordlessError> {
    let item = get_item(db, item_id, moderator)
        .await?
        .ok_or_else(|| not_found("Activity item not found."))?;
    if item.status == "deleted" {
        return Err(not_found("Activity item not found."));
    }
    db.exec(
        "UPDATE wl_activity_items SET status = 'hidden', hidden_by = ?, updated_at = ? WHERE id = ?",
        &[
            Param::Integer(moderator),
            Param::Text(now_iso()),
            Param::Integer(item_id),
        ],
    )
    .await?;
    get_item(db, item_id, moderator)
        .await?
        .ok_or_else(|| not_found("Activity item not found."))
}

/// The actor deletes their own item (soft delete — the row stays for
/// audit, but never renders).
pub async fn delete_own(db: &dyn Db, actor: i64, item_id: i64) -> Result<(), WordlessError> {
    let item = get_item(db, item_id, actor)
        .await?
        .ok_or_else(|| not_found("Activity item not found."))?;
    if item.actor_id != actor {
        return Err(forbidden("You can only delete your own activity."));
    }
    db.exec(
        "UPDATE wl_activity_items SET status = 'deleted', updated_at = ? WHERE id = ?",
        &[Param::Text(now_iso()), Param::Integer(item_id)],
    )
    .await?;
    Ok(())
}

/// Admin moderation list: every item including hidden ones, newest first.
pub async fn admin_list(
    db: &dyn Db,
    page: i64,
    per_page: i64,
) -> Result<Vec<ActivityItem>, WordlessError> {
    let rows = db
        .query_all(
            "SELECT a.id, a.actor_id, a.type, a.object_type, a.object_id, a.content, \
             a.visibility, a.group_id, a.status, a.created_at, a.updated_at, \
             COALESCE(u.display_name, '') AS actor_name, COALESCE(u.user_login, '') AS actor_login, \
             COALESCE(g.name, '') AS group_name, \
             (SELECT COUNT(*) FROM wl_activity_likes l WHERE l.item_id = a.id) AS like_count, \
             0 AS liked \
             FROM wl_activity_items a \
             JOIN wp_users u ON u.ID = a.actor_id \
             LEFT JOIN wl_groups g ON g.id = a.group_id \
             WHERE a.status != 'deleted' \
             ORDER BY a.created_at DESC, a.id DESC LIMIT ? OFFSET ?",
            &[
                Param::Integer(per_page),
                Param::Integer((page - 1) * per_page),
            ],
        )
        .await?;
    Ok(rows.iter().map(|r| row_to_item(r, 0)).collect())
}

/// Fetch one item with like counts (no visibility check — callers
/// enforce it). `viewer` fills `liked_by_me`.
pub async fn get_item(
    db: &dyn Db,
    item_id: i64,
    viewer: i64,
) -> Result<Option<ActivityItem>, WordlessError> {
    let row = db
        .query_one(
            "SELECT a.id, a.actor_id, a.type, a.object_type, a.object_id, a.content, \
             a.visibility, a.group_id, a.status, a.created_at, a.updated_at, \
             COALESCE(u.display_name, '') AS actor_name, COALESCE(u.user_login, '') AS actor_login, \
             COALESCE(g.name, '') AS group_name, \
             (SELECT COUNT(*) FROM wl_activity_likes l WHERE l.item_id = a.id) AS like_count, \
             EXISTS (SELECT 1 FROM wl_activity_likes l2 WHERE l2.item_id = a.id AND l2.user_id = ?) AS liked \
             FROM wl_activity_items a \
             JOIN wp_users u ON u.ID = a.actor_id \
             LEFT JOIN wl_groups g ON g.id = a.group_id \
             WHERE a.id = ? LIMIT 1",
            &[Param::Integer(viewer), Param::Integer(item_id)],
        )
        .await?;
    Ok(row.map(|r| row_to_item(&r, viewer)))
}

/// True when `item_id` appears in `viewer`'s feed (visibility check
/// for like/unlike without fetching a page).
async fn feed_contains(db: &dyn Db, viewer: i64, item_id: i64) -> Result<bool, WordlessError> {
    let row = db
        .query_one(
            "SELECT 1 AS x FROM wl_activity_items a WHERE a.id = ? AND a.status = 'visible' \
             AND NOT EXISTS (SELECT 1 FROM wl_friendships b WHERE b.status = 'blocked' \
               AND ((b.requester_id = ? AND b.addressee_id = a.actor_id) \
               OR (b.requester_id = a.actor_id AND b.addressee_id = ?))) \
             AND (a.visibility = 'public' OR a.actor_id = ? \
               OR (a.visibility = 'friends' AND EXISTS (SELECT 1 FROM wl_friendships f \
                 WHERE f.status = 'accepted' \
                 AND ((f.requester_id = ? AND f.addressee_id = a.actor_id) \
                 OR (f.requester_id = a.actor_id AND f.addressee_id = ?)))) \
               OR (a.visibility = 'group' AND a.group_id IS NOT NULL \
                 AND EXISTS (SELECT 1 FROM wl_group_members gm \
                 WHERE gm.group_id = a.group_id AND gm.user_id = ? AND gm.status = 'member'))) LIMIT 1",
            &[
                Param::Integer(item_id),
                Param::Integer(viewer),
                Param::Integer(viewer),
                Param::Integer(viewer),
                Param::Integer(viewer),
                Param::Integer(viewer),
                Param::Integer(viewer),
            ],
        )
        .await?;
    Ok(row.is_some())
}

fn row_to_item(row: &wordless_db::Row, _viewer: i64) -> ActivityItem {
    let group_name = row_text(row, "group_name");
    ActivityItem {
        id: row_int(row, "id"),
        actor_id: row_int(row, "actor_id"),
        actor_name: row_text(row, "actor_name"),
        actor_login: row_text(row, "actor_login"),
        item_type: row_text(row, "type"),
        object_type: row_text(row, "object_type"),
        object_id: row_int(row, "object_id"),
        content: row_text(row, "content"),
        visibility: row_text(row, "visibility"),
        group_id: row_opt_text(row, "group_id").and_then(|v| v.parse().ok()),
        group_name: if group_name.is_empty() {
            None
        } else {
            Some(group_name)
        },
        status: row_text(row, "status"),
        like_count: row_int(row, "like_count"),
        liked_by_me: row_int(row, "liked") == 1,
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

/// Extract `@login` mentions (deduped, order of first appearance).
/// Logins are word characters, dots, dashes, underscores.
fn find_mentions(content: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' && (i == 0 || !is_login_char(bytes[i - 1])) {
            let mut j = i + 1;
            while j < bytes.len() && is_login_char(bytes[j]) {
                j += 1;
            }
            if j > i + 1 {
                let name = &content[i + 1..j];
                if !out.iter().any(|n| n == name) {
                    out.push(name.to_string());
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

fn is_login_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_'
}
