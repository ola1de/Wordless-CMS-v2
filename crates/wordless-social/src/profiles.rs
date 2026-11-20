//! Member profiles: extended `wp_users` data in `wl_social_profiles`
//! (display name, bio, avatar/cover media ids, free-form fields JSON,
//! visibility). A profile row is created lazily on first read/update
//! so every registered user has a profile without a backfill.

use serde_json::Value;
use wordless_db::row::{row_int, row_opt_text, row_text};
use wordless_db::{Db, Param};
use wordless_kernel::error::{bad_request, forbidden, not_found, WordlessError};
use wordless_kernel::time::now_iso;

use crate::helpers::{friend_status, is_blocked, user_basic};
use crate::types::{MemberEntry, SocialProfile, UpdateProfileInput};
use crate::visibility::can_view_profile;

const VISIBILITIES: [&str; 3] = ["public", "friends", "private"];

fn row_to_profile(row: &wordless_db::Row, login: &str, registered: &str) -> SocialProfile {
    let fields: Value = row_text(row, "fields_json")
        .parse()
        .unwrap_or(Value::Object(Default::default()));
    SocialProfile {
        user_id: row_int(row, "user_id"),
        login: login.to_string(),
        display_name: row_text(row, "display_name"),
        bio: row_text(row, "bio"),
        avatar_id: row_opt_text(row, "avatar_id").and_then(|v| v.parse().ok()),
        cover_id: row_opt_text(row, "cover_id").and_then(|v| v.parse().ok()),
        fields,
        visibility: row_text(row, "visibility"),
        registered: registered.to_string(),
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

/// Ensure a profile row exists for `user_id` (defaults from wp_users)
/// and return it. Errors when the user does not exist.
pub async fn ensure_profile(db: &dyn Db, user_id: i64) -> Result<SocialProfile, WordlessError> {
    let user = user_basic(db, user_id)
        .await?
        .ok_or_else(|| not_found("User not found."))?;
    let now = now_iso();
    db.exec(
        "INSERT OR IGNORE INTO wl_social_profiles \
         (user_id, display_name, bio, avatar_id, cover_id, fields_json, visibility, created_at, updated_at) \
         VALUES (?, ?, '', NULL, NULL, '{}', 'public', ?, ?)",
        &[
            Param::Integer(user_id),
            Param::Text(user.display_name.clone()),
            Param::Text(now.clone()),
            Param::Text(now),
        ],
    )
    .await?;
    get_profile(db, user_id)
        .await?
        .ok_or_else(|| not_found("User not found."))
}

/// Read a profile row (no visibility check — callers enforce it).
pub async fn get_profile(
    db: &dyn Db,
    user_id: i64,
) -> Result<Option<SocialProfile>, WordlessError> {
    let row = db
        .query_one(
            "SELECT p.*, COALESCE(u.user_login, '') AS login, COALESCE(u.user_registered, '') AS registered \
             FROM wl_social_profiles p LEFT JOIN wp_users u ON u.ID = p.user_id \
             WHERE p.user_id = ? LIMIT 1",
            &[Param::Integer(user_id)],
        )
        .await?;
    Ok(row.map(|r| {
        let login = row_text(&r, "login");
        let registered = row_text(&r, "registered");
        row_to_profile(&r, &login, &registered)
    }))
}

/// Read another member's profile with the visibility rules applied.
/// `is_admin` bypasses visibility (moderation) but never a block.
pub async fn view_profile(
    db: &dyn Db,
    viewer: i64,
    owner: i64,
    is_admin: bool,
) -> Result<SocialProfile, WordlessError> {
    let profile = ensure_profile(db, owner).await?;
    let blocked = is_blocked(db, viewer, owner).await?;
    let friends = crate::helpers::are_friends(db, viewer, owner).await?;
    if !can_view_profile(
        viewer,
        owner,
        &profile.visibility,
        friends,
        is_admin,
        blocked,
    ) {
        return Err(if blocked {
            not_found("User not found.")
        } else {
            forbidden("This profile is not visible to you.")
        });
    }
    Ok(profile)
}

/// Update the caller's own profile. `display_name` falls back to the
/// wp_users display name when blank; `visibility` must be a known
/// value; `fields` must be a JSON object when given.
pub async fn update_profile(
    db: &dyn Db,
    user_id: i64,
    input: &UpdateProfileInput,
) -> Result<SocialProfile, WordlessError> {
    if let Some(v) = &input.visibility {
        if !VISIBILITIES.contains(&v.as_str()) {
            return Err(bad_request("Invalid visibility."));
        }
    }
    if let Some(f) = &input.fields {
        if !f.is_object() {
            return Err(bad_request("fields must be an object."));
        }
    }
    let mut profile = ensure_profile(db, user_id).await?;
    if let Some(v) = &input.display_name {
        let v = v.trim();
        if !v.is_empty() {
            profile.display_name = v.to_string();
        }
    }
    if let Some(v) = &input.bio {
        profile.bio = v.trim().to_string();
    }
    if let Some(v) = input.avatar_id {
        profile.avatar_id = if v > 0 { Some(v) } else { None };
    }
    if let Some(v) = input.cover_id {
        profile.cover_id = if v > 0 { Some(v) } else { None };
    }
    if let Some(f) = &input.fields {
        profile.fields = f.clone();
    }
    if let Some(v) = &input.visibility {
        profile.visibility = v.clone();
    }
    let now = now_iso();
    db.exec(
        "UPDATE wl_social_profiles SET display_name = ?, bio = ?, avatar_id = ?, cover_id = ?, \
         fields_json = ?, visibility = ?, updated_at = ? WHERE user_id = ?",
        &[
            Param::Text(profile.display_name.clone()),
            Param::Text(profile.bio.clone()),
            match profile.avatar_id {
                Some(v) => Param::Integer(v),
                None => Param::Null,
            },
            match profile.cover_id {
                Some(v) => Param::Integer(v),
                None => Param::Null,
            },
            Param::Text(profile.fields.to_string()),
            Param::Text(profile.visibility.clone()),
            Param::Text(now),
            Param::Integer(user_id),
        ],
    )
    .await?;
    get_profile(db, user_id)
        .await?
        .ok_or_else(|| not_found("User not found."))
}

/// Member directory: every registered user with a lazily-created
/// profile, filtered by the viewer's visibility rules. `q` matches
/// display name, login, or bio (LIKE, case-insensitive on ASCII).
/// Blocked users are excluded both ways; admins see everyone.
/// The visibility filter runs in SQL so `total` is exact.
/// Returns (entries, total).
pub async fn list_members(
    db: &dyn Db,
    viewer: i64,
    q: &str,
    page: i64,
    per_page: i64,
    is_admin: bool,
) -> Result<(Vec<MemberEntry>, i64), WordlessError> {
    // Privacy filter (skipped for admins): no blocked user either way;
    // self always; public profiles; friends-only profiles only for
    // accepted friends; private profiles never (except self/admin).
    let privacy = if is_admin {
        "1 = 1".to_string()
    } else {
        "NOT EXISTS (SELECT 1 FROM wl_friendships b WHERE b.status = 'blocked' \
         AND ((b.requester_id = ? AND b.addressee_id = u.ID) \
         OR (b.requester_id = u.ID AND b.addressee_id = ?))) \
         AND (u.ID = ? OR COALESCE(p.visibility, 'public') = 'public' \
         OR (COALESCE(p.visibility, 'public') = 'friends' \
         AND EXISTS (SELECT 1 FROM wl_friendships f WHERE f.status = 'accepted' \
         AND ((f.requester_id = ? AND f.addressee_id = u.ID) \
         OR (f.requester_id = u.ID AND f.addressee_id = ?)))))"
            .to_string()
    };
    let like = format!("%{}%", q.trim());
    let search = "(? = '' OR u.display_name LIKE ? OR u.user_login LIKE ? \
        OR COALESCE(p.bio, '') LIKE ?)";
    let list_sql = format!(
        "SELECT u.ID, u.user_login, u.user_registered, \
         COALESCE(p.display_name, u.display_name) AS display_name, \
         COALESCE(p.bio, '') AS bio, p.avatar_id, \
         COALESCE(p.visibility, 'public') AS visibility \
         FROM wp_users u LEFT JOIN wl_social_profiles p ON p.user_id = u.ID \
         WHERE {search} AND {privacy} \
         ORDER BY display_name ASC, u.ID ASC LIMIT ? OFFSET ?"
    );
    let count_sql = format!(
        "SELECT COUNT(*) AS c FROM wp_users u \
         LEFT JOIN wl_social_profiles p ON p.user_id = u.ID \
         WHERE {search} AND {privacy}"
    );
    // Parameter order: search(4) + privacy(5 when not admin) + limit/offset.
    let mut list_params: Vec<Param> = vec![
        Param::Text(q.trim().to_string()),
        Param::Text(like.clone()),
        Param::Text(like.clone()),
        Param::Text(like),
    ];
    if !is_admin {
        list_params.extend([
            Param::Integer(viewer),
            Param::Integer(viewer),
            Param::Integer(viewer),
            Param::Integer(viewer),
            Param::Integer(viewer),
        ]);
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
        Param::Text(format!("%{}%", q.trim())),
    ];
    if !is_admin {
        count_params.extend([
            Param::Integer(viewer),
            Param::Integer(viewer),
            Param::Integer(viewer),
            Param::Integer(viewer),
            Param::Integer(viewer),
        ]);
    }
    let total_row = db.query_one(&count_sql, &count_params).await?;
    let total = total_row.map(|r| row_int(&r, "c")).unwrap_or(0);
    let mut out = Vec::new();
    for r in &rows {
        let uid = row_int(r, "ID");
        out.push(MemberEntry {
            user_id: uid,
            login: row_text(r, "user_login"),
            display_name: row_text(r, "display_name"),
            bio: row_text(r, "bio"),
            avatar_id: row_opt_text(r, "avatar_id").and_then(|v| v.parse().ok()),
            visibility: row_text(r, "visibility"),
            friend_status: if uid == viewer {
                "none".to_string()
            } else {
                friend_status(db, viewer, uid).await?
            },
            registered: row_text(r, "user_registered"),
        });
    }
    Ok((out, total))
}
