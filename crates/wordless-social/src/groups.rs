//! Groups: public / private / hidden, membership with roles,
//! join requests, invites, bans. The creator becomes the first admin.
//! When the last admin leaves, the oldest active member is promoted
//! so a group is never left admin-less.

use wordless_db::row::{row_int, row_opt_text, row_text};
use wordless_db::{Db, Param};
use wordless_kernel::error::{bad_request, conflict, forbidden, not_found, WordlessError};
use wordless_kernel::time::now_iso;

use crate::helpers::group_membership;
use crate::notifications;
use crate::types::{Group, GroupMember, Notification};
use crate::visibility::{can_join_directly, can_view_group};

fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if (c == ' ' || c == '-' || c == '_') && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "group".to_string()
    } else {
        out
    }
}

/// Create a group; the creator is its first admin member.
pub async fn create(
    db: &dyn Db,
    creator: i64,
    name: &str,
    description: &str,
    visibility: &str,
) -> Result<Group, WordlessError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(bad_request("Group name is required."));
    }
    if !["public", "private", "hidden"].contains(&visibility) {
        return Err(bad_request("Invalid group visibility."));
    }
    let mut slug = slugify(name);
    // Unique slug: append -2, -3, ... on collision.
    let mut n = 2;
    while db
        .query_one(
            "SELECT 1 AS x FROM wl_groups WHERE slug = ? LIMIT 1",
            &[Param::Text(slug.clone())],
        )
        .await?
        .is_some()
    {
        slug = format!("{}-{n}", slugify(name));
        n += 1;
    }
    let now = now_iso();
    let meta = db
        .exec(
            "INSERT INTO wl_groups (slug, name, description, visibility, created_by, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            &[
                Param::Text(slug),
                Param::Text(name.to_string()),
                Param::Text(description.trim().to_string()),
                Param::Text(visibility.to_string()),
                Param::Integer(creator),
                Param::Text(now.clone()),
                Param::Text(now.clone()),
            ],
        )
        .await?;
    let id = meta.last_row_id.unwrap_or(0);
    db.exec(
        "INSERT INTO wl_group_members (group_id, user_id, role, status, created_at) \
         VALUES (?, ?, 'admin', 'member', ?)",
        &[
            Param::Integer(id),
            Param::Integer(creator),
            Param::Text(now),
        ],
    )
    .await?;
    get(db, id, creator, false)
        .await?
        .ok_or_else(|| not_found("Group not found."))
}

/// Read one group with the caller's membership. Hidden groups are
/// invisible to non-members (404, not 403 — do not leak existence).
pub async fn get(
    db: &dyn Db,
    group_id: i64,
    viewer: i64,
    is_admin: bool,
) -> Result<Option<Group>, WordlessError> {
    let row = db
        .query_one(
            "SELECT g.*, (SELECT COUNT(*) FROM wl_group_members m \
             WHERE m.group_id = g.id AND m.status = 'member') AS member_count \
             FROM wl_groups g WHERE g.id = ? LIMIT 1",
            &[Param::Integer(group_id)],
        )
        .await?;
    let Some(r) = row else {
        return Ok(None);
    };
    let visibility = row_text(&r, "visibility");
    let member = group_membership(db, group_id, viewer).await?;
    // An invited user must be able to see the group to accept the
    // invite; membership itself still requires accepting it.
    let can_see = matches!(member, Some((_, ref s)) if s == "member" || s == "invited");
    if !can_view_group(viewer, &visibility, can_see, is_admin) {
        return Ok(None);
    }
    Ok(Some(row_to_group(&r, member)))
}

pub async fn get_by_slug(
    db: &dyn Db,
    slug: &str,
    viewer: i64,
    is_admin: bool,
) -> Result<Option<Group>, WordlessError> {
    let row = db
        .query_one(
            "SELECT id FROM wl_groups WHERE slug = ? LIMIT 1",
            &[Param::Text(slug.to_string())],
        )
        .await?;
    match row {
        None => Ok(None),
        Some(r) => get(db, row_int(&r, "id"), viewer, is_admin).await,
    }
}

/// List groups the viewer may see (hidden only for members/admins).
/// The hidden filter runs in SQL so `total` is exact.
pub async fn list(
    db: &dyn Db,
    viewer: i64,
    q: &str,
    page: i64,
    per_page: i64,
    is_admin: bool,
) -> Result<(Vec<Group>, i64), WordlessError> {
    let hidden_filter = if is_admin {
        "1 = 1".to_string()
    } else {
        // Hidden groups are listed for members AND invitees (an
        // invitee must see the group to accept the invite).
        "((SELECT 1 FROM wl_group_members m WHERE m.group_id = g.id AND m.user_id = ? AND m.status IN ('member', 'invited')) IS NOT NULL) \
         OR g.visibility != 'hidden'"
            .to_string()
    };
    let like = format!("%{}%", q.trim());
    let search = "(? = '' OR g.name LIKE ? OR g.description LIKE ?)";
    let list_sql = format!(
        "SELECT g.*, (SELECT COUNT(*) FROM wl_group_members m \
         WHERE m.group_id = g.id AND m.status = 'member') AS member_count \
         FROM wl_groups g \
         WHERE {search} AND {hidden_filter} \
         ORDER BY g.name ASC, g.id ASC LIMIT ? OFFSET ?"
    );
    let count_sql =
        format!("SELECT COUNT(*) AS c FROM wl_groups g WHERE {search} AND {hidden_filter}");
    let mut list_params: Vec<Param> = vec![
        Param::Text(q.trim().to_string()),
        Param::Text(like.clone()),
        Param::Text(like),
    ];
    if !is_admin {
        list_params.push(Param::Integer(viewer));
    }
    list_params.extend([
        Param::Integer(per_page),
        Param::Integer((page - 1) * per_page),
    ]);
    let rows = db.query_all(&list_sql, &list_params).await?;
    let mut count_params: Vec<Param> = vec![
        Param::Text(q.trim().to_string()),
        Param::Text(format!("%{}%", q.trim())),
        Param::Text(format!("%{}%", q.trim())),
    ];
    if !is_admin {
        count_params.push(Param::Integer(viewer));
    }
    let total_row = db.query_one(&count_sql, &count_params).await?;
    let total = total_row.map(|r| row_int(&r, "c")).unwrap_or(0);
    let mut out = Vec::new();
    for r in &rows {
        let gid = row_int(r, "id");
        let member = group_membership(db, gid, viewer).await?;
        out.push(row_to_group(r, member));
    }
    Ok((out, total))
}

/// Join a group. Public: immediate member. Private: join request
/// (notifies group admins). Hidden: refused — invite only.
/// Returns the notification created for the group admins, if any.
pub async fn join(
    db: &dyn Db,
    user_id: i64,
    group_id: i64,
) -> Result<(Group, Option<Notification>), WordlessError> {
    let group = get(db, group_id, user_id, false)
        .await?
        .ok_or_else(|| not_found("Group not found."))?;
    let member = group_membership(db, group_id, user_id).await?;
    if let Some((_, ref s)) = member {
        match s.as_str() {
            "member" => return Err(conflict("You are already a member.", "already_member")),
            "banned" => return Err(forbidden("You are banned from this group.")),
            "requested" => {
                return Err(conflict(
                    "Join request already pending.",
                    "already_requested",
                ))
            }
            "invited" => {
                // Accepting an invite: become a member.
                db.exec(
                    "UPDATE wl_group_members SET status = 'member', created_at = ? \
                     WHERE group_id = ? AND user_id = ?",
                    &[
                        Param::Text(now_iso()),
                        Param::Integer(group_id),
                        Param::Integer(user_id),
                    ],
                )
                .await?;
                let g = get(db, group_id, user_id, false)
                    .await?
                    .ok_or_else(|| not_found("Group not found."))?;
                return Ok((g, None));
            }
            _ => {}
        }
    }
    if !can_join_directly(&group.visibility) {
        if group.visibility == "hidden" {
            return Err(forbidden("Hidden groups are invite-only."));
        }
        // Private: file a join request.
        db.exec(
            "INSERT INTO wl_group_members (group_id, user_id, role, status, created_at) \
             VALUES (?, ?, 'member', 'requested', ?)",
            &[
                Param::Integer(group_id),
                Param::Integer(user_id),
                Param::Text(now_iso()),
            ],
        )
        .await?;
        // Notify the first group admin.
        let admin = db
            .query_one(
                "SELECT user_id FROM wl_group_members \
                 WHERE group_id = ? AND role = 'admin' AND status = 'member' \
                 ORDER BY created_at ASC LIMIT 1",
                &[Param::Integer(group_id)],
            )
            .await?;
        let notif = match admin {
            Some(r) => {
                notifications::create(
                    db,
                    row_int(&r, "user_id"),
                    "group_join_request",
                    user_id,
                    "group",
                    group_id,
                )
                .await?
            }
            None => None,
        };
        let g = get(db, group_id, user_id, false)
            .await?
            .ok_or_else(|| not_found("Group not found."))?;
        return Ok((g, notif));
    }
    db.exec(
        "INSERT INTO wl_group_members (group_id, user_id, role, status, created_at) \
         VALUES (?, ?, 'member', 'member', ?)",
        &[
            Param::Integer(group_id),
            Param::Integer(user_id),
            Param::Text(now_iso()),
        ],
    )
    .await?;
    let g = get(db, group_id, user_id, false)
        .await?
        .ok_or_else(|| not_found("Group not found."))?;
    Ok((g, None))
}

/// Leave a group. If the last admin leaves, the oldest active member
/// is promoted to admin first.
pub async fn leave(db: &dyn Db, user_id: i64, group_id: i64) -> Result<(), WordlessError> {
    let member = group_membership(db, group_id, user_id).await?;
    let Some((role, status)) = member else {
        return Err(not_found("You are not a member of this group."));
    };
    if status != "member" && status != "invited" {
        // A pending request is withdrawn by leaving.
        db.exec(
            "DELETE FROM wl_group_members WHERE group_id = ? AND user_id = ?",
            &[Param::Integer(group_id), Param::Integer(user_id)],
        )
        .await?;
        return Ok(());
    }
    if role == "admin" {
        let admins = db
            .query_one(
                "SELECT COUNT(*) AS c FROM wl_group_members \
                 WHERE group_id = ? AND role = 'admin' AND status = 'member' AND user_id != ?",
                &[Param::Integer(group_id), Param::Integer(user_id)],
            )
            .await?;
        if admins.map(|r| row_int(&r, "c")).unwrap_or(0) == 0 {
            // Promote the oldest non-admin active member, if any.
            let next = db
                .query_one(
                    "SELECT user_id FROM wl_group_members \
                     WHERE group_id = ? AND role != 'admin' AND status = 'member' \
                     ORDER BY created_at ASC LIMIT 1",
                    &[Param::Integer(group_id)],
                )
                .await?;
            if let Some(r) = next {
                db.exec(
                    "UPDATE wl_group_members SET role = 'admin' WHERE group_id = ? AND user_id = ?",
                    &[
                        Param::Integer(group_id),
                        Param::Integer(row_int(&r, "user_id")),
                    ],
                )
                .await?;
            }
        }
    }
    db.exec(
        "DELETE FROM wl_group_members WHERE group_id = ? AND user_id = ?",
        &[Param::Integer(group_id), Param::Integer(user_id)],
    )
    .await?;
    Ok(())
}

/// Approve a pending join request (group admin/moderator only).
pub async fn approve(
    db: &dyn Db,
    approver: i64,
    group_id: i64,
    user_id: i64,
) -> Result<Option<Notification>, WordlessError> {
    require_group_admin(db, approver, group_id).await?;
    let member = group_membership(db, group_id, user_id).await?;
    match member {
        Some((_, ref s)) if s == "requested" => {
            db.exec(
                "UPDATE wl_group_members SET status = 'member' WHERE group_id = ? AND user_id = ?",
                &[Param::Integer(group_id), Param::Integer(user_id)],
            )
            .await?;
            notifications::create(db, user_id, "group_approved", approver, "group", group_id).await
        }
        Some((_, ref s)) if s == "member" => {
            Err(conflict("User is already a member.", "already_member"))
        }
        _ => Err(not_found("No pending join request from this user.")),
    }
}

/// Invite a user to a group (group admin/moderator only). Hidden
/// groups can only grow this way.
pub async fn invite(
    db: &dyn Db,
    inviter: i64,
    group_id: i64,
    user_id: i64,
) -> Result<Option<Notification>, WordlessError> {
    require_group_admin(db, inviter, group_id).await?;
    if crate::helpers::user_basic(db, user_id).await?.is_none() {
        return Err(not_found("User not found."));
    }
    let member = group_membership(db, group_id, user_id).await?;
    if let Some((_, ref s)) = member {
        if s == "member" {
            return Err(conflict("User is already a member.", "already_member"));
        }
        if s == "banned" {
            return Err(forbidden("User is banned from this group."));
        }
    }
    db.exec(
        "INSERT OR REPLACE INTO wl_group_members (group_id, user_id, role, status, created_at) \
         VALUES (?, ?, 'member', 'invited', ?)",
        &[
            Param::Integer(group_id),
            Param::Integer(user_id),
            Param::Text(now_iso()),
        ],
    )
    .await?;
    notifications::create(db, user_id, "group_invite", inviter, "group", group_id).await
}

/// Ban a user from a group (group admin only). Removes membership.
pub async fn ban(
    db: &dyn Db,
    admin: i64,
    group_id: i64,
    user_id: i64,
) -> Result<(), WordlessError> {
    let member = group_membership(db, group_id, admin).await?;
    if !matches!(member, Some((ref r, ref s)) if r == "admin" && s == "member") {
        return Err(forbidden("Only a group admin can ban members."));
    }
    if admin == user_id {
        return Err(bad_request("You cannot ban yourself."));
    }
    db.exec(
        "INSERT OR REPLACE INTO wl_group_members (group_id, user_id, role, status, created_at) \
         VALUES (?, ?, 'member', 'banned', ?)",
        &[
            Param::Integer(group_id),
            Param::Integer(user_id),
            Param::Text(now_iso()),
        ],
    )
    .await?;
    Ok(())
}

/// List a group's members (members/admins only for private/hidden).
pub async fn members(
    db: &dyn Db,
    viewer: i64,
    group_id: i64,
    is_admin: bool,
) -> Result<Vec<GroupMember>, WordlessError> {
    let group = get(db, group_id, viewer, is_admin)
        .await?
        .ok_or_else(|| not_found("Group not found."))?;
    let member = group_membership(db, group_id, viewer).await?;
    let is_member = matches!(member, Some((_, ref s)) if s == "member");
    if !crate::visibility::can_view_group_content(&group.visibility, is_member, is_admin) {
        return Err(forbidden(
            "This group's members are visible to members only.",
        ));
    }
    let rows = db
        .query_all(
            "SELECT m.user_id, m.role, m.status, m.created_at, \
             COALESCE(u.user_login, '') AS login, COALESCE(u.display_name, '') AS display_name \
             FROM wl_group_members m LEFT JOIN wp_users u ON u.ID = m.user_id \
             WHERE m.group_id = ? AND m.status IN ('member', 'requested', 'invited') \
             ORDER BY CASE m.role WHEN 'admin' THEN 0 WHEN 'moderator' THEN 1 ELSE 2 END, m.created_at ASC",
            &[Param::Integer(group_id)],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| GroupMember {
            user_id: row_int(r, "user_id"),
            login: row_text(r, "login"),
            display_name: row_text(r, "display_name"),
            role: row_text(r, "role"),
            status: row_text(r, "status"),
            created_at: row_text(r, "created_at"),
        })
        .collect())
}

/// Fields for a group update — every field optional.
pub struct UpdateGroupInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub visibility: Option<String>,
    pub avatar_id: Option<i64>,
}

/// Update a group's name/description/visibility/avatar (group admin
/// or site admin). Visibility changes are allowed but a hidden group
/// stays hidden from non-members immediately.
pub async fn update(
    db: &dyn Db,
    updater: i64,
    group_id: i64,
    input: &UpdateGroupInput,
    is_site_admin: bool,
) -> Result<Group, WordlessError> {
    let group = get(db, group_id, updater, is_site_admin)
        .await?
        .ok_or_else(|| not_found("Group not found."))?;
    let member = group_membership(db, group_id, updater).await?;
    let is_group_admin = matches!(member, Some((ref r, ref s)) if r == "admin" && s == "member");
    if !is_group_admin && !is_site_admin {
        return Err(forbidden("Only a group admin can edit the group."));
    }
    let mut new_name = group.name.clone();
    let mut new_description = group.description.clone();
    let mut new_visibility = group.visibility.clone();
    let mut avatar = group.avatar_id;
    if let Some(n) = &input.name {
        let n = n.trim();
        if n.is_empty() {
            return Err(bad_request("Group name is required."));
        }
        new_name = n.to_string();
    }
    if let Some(d) = &input.description {
        new_description = d.trim().to_string();
    }
    if let Some(v) = &input.visibility {
        if !["public", "private", "hidden"].contains(&v.as_str()) {
            return Err(bad_request("Invalid group visibility."));
        }
        new_visibility = v.clone();
    }
    if let Some(a) = input.avatar_id {
        avatar = if a > 0 { Some(a) } else { None };
    }
    db.exec(
        "UPDATE wl_groups SET name = ?, description = ?, visibility = ?, avatar_id = ?, updated_at = ? \
         WHERE id = ?",
        &[
            Param::Text(new_name),
            Param::Text(new_description),
            Param::Text(new_visibility),
            match avatar {
                Some(v) => Param::Integer(v),
                None => Param::Null,
            },
            Param::Text(now_iso()),
            Param::Integer(group_id),
        ],
    )
    .await?;
    get(db, group_id, updater, is_site_admin)
        .await?
        .ok_or_else(|| not_found("Group not found."))
}

/// Caller must be a group admin or moderator (active member).
pub async fn require_group_admin(
    db: &dyn Db,
    user_id: i64,
    group_id: i64,
) -> Result<(), WordlessError> {
    let member = group_membership(db, group_id, user_id).await?;
    match member {
        Some((ref r, ref s)) if (r == "admin" || r == "moderator") && s == "member" => Ok(()),
        _ => Err(forbidden("Only a group admin or moderator can do this.")),
    }
}

fn row_to_group(row: &wordless_db::Row, member: Option<(String, String)>) -> Group {
    let (my_status, my_role) = match member {
        Some((r, s)) => (s, Some(r)),
        None => ("none".to_string(), None),
    };
    Group {
        id: row_int(row, "id"),
        slug: row_text(row, "slug"),
        name: row_text(row, "name"),
        description: row_text(row, "description"),
        visibility: row_text(row, "visibility"),
        avatar_id: row_opt_text(row, "avatar_id").and_then(|v| v.parse().ok()),
        created_by: row_int(row, "created_by"),
        member_count: row_int(row, "member_count"),
        my_status,
        my_role,
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}
