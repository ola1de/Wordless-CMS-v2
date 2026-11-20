//! Users service.
//! Roles are stored the WordPress way: a `wp_capabilities` row in
//! `wp_usermeta` holding JSON, e.g. `{"administrator": true}`.
//! (Port of v1 `src/modules/users/service.ts` — same validation,
//! same conflict codes, same messages, same meta handling.)

use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::capabilities::{AuthUser, Role};
use wordless_kernel::crypto;
use wordless_kernel::error::{bad_request, conflict, internal_error, not_found, WordlessError};
use wordless_kernel::time::now_iso;
use wordless_kernel::validate::{require_email, require_string};

use crate::password::{hash_password, verify_password};
use crate::session::revoke_all_sessions_for_user;
use crate::totp::PASSWORD_RESET_META_KEY;

const ROLE_META_KEY: &str = "wp_capabilities";
/// Temporary passwords: 16 chars, no ambiguous glyphs (v1
/// `TEMP_PASSWORD_CHARS`).
const TEMP_PASSWORD_CHARS: &[u8] = b"abcdefghjkmnpqrstuvwxyzABCDEFGHJKMNPQRSTUVWXYZ23456789";

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: i64,
    pub login: String,
    pub nicename: String,
    pub email: String,
    pub url: String,
    pub registered: String,
    pub display_name: String,
    pub role: Role,
}

impl User {
    pub fn to_auth_user(&self) -> AuthUser {
        AuthUser {
            id: self.id,
            login: self.login.clone(),
            email: self.email.clone(),
            display_name: self.display_name.clone(),
            role: self.role,
        }
    }
}

fn row_to_user(row: &Row, role: Role) -> User {
    User {
        id: row_int(row, "ID"),
        login: row_text(row, "user_login"),
        nicename: row_text(row, "user_nicename"),
        email: row_text(row, "user_email"),
        url: row_text(row, "user_url"),
        registered: row_text(row, "user_registered"),
        display_name: row_text(row, "display_name"),
        role,
    }
}

/// Parse the `wp_capabilities` JSON meta value into a role.
/// First key that is a known role with value `true` wins; anything
/// corrupt or missing falls back to `subscriber` (same as v1).
pub fn role_from_capabilities_meta(meta_value: &str) -> Role {
    let parsed: serde_json::Value = match serde_json::from_str(meta_value) {
        Ok(v) => v,
        Err(_) => return Role::Subscriber,
    };
    let Some(obj) = parsed.as_object() else {
        return Role::Subscriber;
    };
    for (key, value) in obj {
        if value.as_bool() == Some(true) {
            if let Some(role) = Role::from_str(key) {
                return role;
            }
        }
    }
    Role::Subscriber
}

async fn role_for_user(db: &dyn Db, user_id: i64) -> Result<Role, WordlessError> {
    let row = db
        .prepare("SELECT meta_value FROM wp_usermeta WHERE user_id = ? AND meta_key = ? LIMIT 1")
        .bind(vec![
            Param::Integer(user_id),
            Param::Text("wp_capabilities".to_string()),
        ])
        .first::<Row>()
        .await?;
    let Some(row) = row else {
        return Ok(Role::Subscriber);
    };
    Ok(role_from_capabilities_meta(&row_text(&row, "meta_value")))
}

pub async fn get_user_by_id(db: &dyn Db, id: i64) -> Result<Option<User>, WordlessError> {
    let row = db
        .prepare("SELECT * FROM wp_users WHERE ID = ? LIMIT 1")
        .bind(vec![Param::Integer(id)])
        .first::<Row>()
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let role = role_for_user(db, id).await?;
    Ok(Some(row_to_user(&row, role)))
}

pub async fn get_user_by_login(db: &dyn Db, login: &str) -> Result<Option<User>, WordlessError> {
    let row = db
        .prepare("SELECT * FROM wp_users WHERE user_login = ? LIMIT 1")
        .bind(vec![Param::Text(login.to_string())])
        .first::<Row>()
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let id = row_int(&row, "ID");
    let role = role_for_user(db, id).await?;
    Ok(Some(row_to_user(&row, role)))
}

pub async fn get_user_by_email(db: &dyn Db, email: &str) -> Result<Option<User>, WordlessError> {
    let row = db
        .prepare("SELECT * FROM wp_users WHERE user_email = ? LIMIT 1")
        .bind(vec![Param::Text(email.to_string())])
        .first::<Row>()
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let id = row_int(&row, "ID");
    let role = role_for_user(db, id).await?;
    Ok(Some(row_to_user(&row, role)))
}

/// User meta (`wp_usermeta`). Values are stored as JSON.
/// Returns `default` when the row is missing or the value is not
/// valid JSON for `T`. (v1 `getUserMeta`.)
pub async fn get_user_meta<T: serde::de::DeserializeOwned>(
    db: &dyn Db,
    user_id: i64,
    key: &str,
    default: T,
) -> Result<T, WordlessError> {
    let row = db
        .prepare("SELECT meta_value FROM wp_usermeta WHERE user_id = ? AND meta_key = ? LIMIT 1")
        .bind(vec![Param::Integer(user_id), Param::Text(key.to_string())])
        .first::<Row>()
        .await?;
    let Some(row) = row else {
        return Ok(default);
    };
    match serde_json::from_str::<T>(&row_text(&row, "meta_value")) {
        Ok(v) => Ok(v),
        Err(_) => Ok(default),
    }
}

/// Write user meta (insert or update), JSON-encoding the value.
/// (v1 `updateUserMeta`.)
pub async fn update_user_meta<T: serde::Serialize>(
    db: &dyn Db,
    user_id: i64,
    key: &str,
    value: &T,
) -> Result<(), WordlessError> {
    let json = serde_json::to_string(value)
        .map_err(|e| internal_error(&format!("user meta JSON failed: {e}")))?;
    let meta = db
        .prepare("UPDATE wp_usermeta SET meta_value = ? WHERE user_id = ? AND meta_key = ?")
        .bind(vec![
            Param::Text(json.clone()),
            Param::Integer(user_id),
            Param::Text(key.to_string()),
        ])
        .run()
        .await?;
    if meta.changes == 0 {
        db.prepare("INSERT INTO wp_usermeta (user_id, meta_key, meta_value) VALUES (?, ?, ?)")
            .bind(vec![
                Param::Integer(user_id),
                Param::Text(key.to_string()),
                Param::Text(json),
            ])
            .run()
            .await?;
    }
    Ok(())
}

/// Delete one user-meta row. Returns true when a row was deleted.
/// (v1 `deleteUserMeta`.)
pub async fn delete_user_meta(db: &dyn Db, user_id: i64, key: &str) -> Result<bool, WordlessError> {
    let meta = db
        .prepare("DELETE FROM wp_usermeta WHERE user_id = ? AND meta_key = ?")
        .bind(vec![Param::Integer(user_id), Param::Text(key.to_string())])
        .run()
        .await?;
    Ok(meta.changes > 0)
}

/// `{"<role>": true}` — the `wp_capabilities` value for one role.
fn role_caps_json(role: Role) -> serde_json::Value {
    let mut caps = serde_json::Map::new();
    caps.insert(role.as_str().to_string(), serde_json::Value::Bool(true));
    serde_json::Value::Object(caps)
}

/// v1 `LOGIN_RE`: letters, digits, underscore, dot and hyphen only.
fn is_valid_login(login: &str) -> bool {
    !login.is_empty()
        && login
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
}

/// v1 nicename: lowercase, each run of non-`[a-z0-9]` becomes one `-`.
fn nicename_from(raw: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for c in raw.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct CreateUserInput {
    pub login: String,
    pub email: String,
    pub password: String,
    pub display_name: Option<String>,
    pub nicename: Option<String>,
    pub url: Option<String>,
    pub role: Option<Role>,
}

/// Create a user. (v1 `createUser` — same validation, same conflict
/// code `user_exists` (409), same defaults: display name falls back
/// to the login, role falls back to subscriber.)
pub async fn create_user(db: &dyn Db, input: CreateUserInput) -> Result<User, WordlessError> {
    let login = require_string(&serde_json::json!(input.login), "login", 1, 60)?;
    if !is_valid_login(&login) {
        return Err(bad_request(
            "Field \"login\" may only contain letters, digits, underscore, dot and hyphen.",
        ));
    }
    let email = require_email(&serde_json::json!(input.email), "email")?;
    let password = require_string(&serde_json::json!(input.password), "password", 8, 200)?;
    let display_name = input
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&login)
        .to_string();
    let nicename = nicename_from(
        input
            .nicename
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&login),
    );
    let url = input
        .url
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let role = input.role.unwrap_or(Role::Subscriber);

    let existing_login = db
        .prepare("SELECT ID FROM wp_users WHERE user_login = ? LIMIT 1")
        .bind(vec![Param::Text(login.clone())])
        .first::<Row>()
        .await?;
    if existing_login.is_some() {
        return Err(conflict(
            &format!("A user with login \"{login}\" already exists."),
            "user_exists",
        ));
    }
    let existing_email = db
        .prepare("SELECT ID FROM wp_users WHERE user_email = ? LIMIT 1")
        .bind(vec![Param::Text(email.clone())])
        .first::<Row>()
        .await?;
    if existing_email.is_some() {
        return Err(conflict(
            &format!("A user with email \"{email}\" already exists."),
            "user_exists",
        ));
    }

    let hash = hash_password(&password).await?;
    let now = now_iso();
    let meta = db
        .prepare(
            "INSERT INTO wp_users (user_login, user_pass, user_nicename, user_email, user_url, user_registered, display_name) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(vec![
            Param::Text(login),
            Param::Text(hash),
            Param::Text(nicename),
            Param::Text(email),
            Param::Text(url),
            Param::Text(now),
            Param::Text(display_name),
        ])
        .run()
        .await?;
    let id = meta.last_row_id.unwrap_or(0);
    update_user_meta(db, id, ROLE_META_KEY, &role_caps_json(role)).await?;
    match get_user_by_id(db, id).await? {
        Some(created) => Ok(created),
        None => Err(internal_error("Failed to read back the created user.")),
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserListResult {
    pub users: Vec<User>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

/// Paginated user list (admin only at the route layer). `page` below 1
/// reads as 1; `per_page` is clamped to 1..=100 (v1 `listUsers`).
/// The route passes page 1 / per-page 20 as defaults.
pub async fn list_users(
    db: &dyn Db,
    page: i64,
    per_page: i64,
) -> Result<UserListResult, WordlessError> {
    let page = if page >= 1 { page } else { 1 };
    let per_page = per_page.clamp(1, 100);
    let total_row = db
        .prepare("SELECT COUNT(*) AS n FROM wp_users")
        .first::<Row>()
        .await?;
    let total = total_row.as_ref().map(|r| row_int(r, "n")).unwrap_or(0);
    let rows = db
        .prepare("SELECT * FROM wp_users ORDER BY user_registered DESC, ID DESC LIMIT ? OFFSET ?")
        .bind(vec![
            Param::Integer(per_page),
            Param::Integer((page - 1) * per_page),
        ])
        .all::<Row>()
        .await?;
    let mut users = Vec::new();
    for row in &rows {
        users.push(row_to_user(
            row,
            role_for_user(db, row_int(row, "ID")).await?,
        ));
    }
    Ok(UserListResult {
        users,
        total,
        page,
        per_page,
        total_pages: if total == 0 {
            0
        } else {
            (total + per_page - 1) / per_page
        },
    })
}

/// Change a user's role (rewrites the `wp_capabilities` meta row).
/// (v1 `setUserRole`.)
pub async fn set_user_role(db: &dyn Db, id: i64, role: Role) -> Result<User, WordlessError> {
    if get_user_by_id(db, id).await?.is_none() {
        return Err(not_found(&format!("User {id} not found.")));
    }
    update_user_meta(db, id, ROLE_META_KEY, &role_caps_json(role)).await?;
    match get_user_by_id(db, id).await? {
        Some(updated) => Ok(updated),
        None => Err(internal_error("Failed to read back the updated user.")),
    }
}

/// Delete a user. Refused when the user owns posts — content must be
/// reassigned or deleted first. Reassignment is a later milestone;
/// the refusal message says so instead of silently deleting content.
/// (v1 `deleteUser`.)
pub async fn delete_user(db: &dyn Db, id: i64) -> Result<bool, WordlessError> {
    let Some(existing) = get_user_by_id(db, id).await? else {
        return Err(not_found(&format!("User {id} not found.")));
    };
    let owned_row = db
        .prepare(
            "SELECT COUNT(*) AS n FROM wp_posts WHERE post_author = ? AND post_status != 'trash'",
        )
        .bind(vec![Param::Integer(id)])
        .first::<Row>()
        .await?;
    let owned = owned_row.as_ref().map(|r| row_int(r, "n")).unwrap_or(0);
    if owned > 0 {
        return Err(bad_request(&format!(
            "User \"{}\" owns {owned} post(s). Reassign or delete their posts first — post reassignment is a later milestone and is not available yet.",
            existing.login
        )));
    }
    // Revoke sessions directly (same SQL as `revoke_all_sessions_for_user`).
    revoke_all_sessions_for_user(db, id).await?;
    db.prepare("DELETE FROM wp_usermeta WHERE user_id = ?")
        .bind(vec![Param::Integer(id)])
        .run()
        .await?;
    let meta = db
        .prepare("DELETE FROM wp_users WHERE ID = ?")
        .bind(vec![Param::Integer(id)])
        .run()
        .await?;
    Ok(meta.changes > 0)
}

/// Generate a random temporary password (16 chars, no ambiguous glyphs).
/// (v1 `generateTemporaryPassword`.)
pub fn generate_temporary_password() -> Result<String, WordlessError> {
    let bytes = crypto::random_bytes(16)
        .map_err(|e| internal_error(&format!("random temporary password failed: {e}")))?;
    let mut out = String::with_capacity(16);
    for b in bytes {
        out.push(TEMP_PASSWORD_CHARS[(b as usize) % TEMP_PASSWORD_CHARS.len()] as char);
    }
    Ok(out)
}

/// Admin password reset. Sets a fresh temporary password, revokes all
/// of the user's sessions, and returns the password ONCE. It is never
/// logged and never stored in plaintext — only the PBKDF2 hash is
/// stored. (v1 `resetUserPassword`.)
///
/// NOTE: this does NOT clear the `wl_password_reset_required` flag —
/// in v1 the ROUTE does that (`deleteUserMeta` after the reset, see
/// `src/index.ts` `POST /api/users/:id/reset-password`), along with
/// the `user.password_reset` activity log and the email. Use
/// `clear_password_reset_required` for the flag step.
pub async fn reset_user_password(db: &dyn Db, id: i64) -> Result<(User, String), WordlessError> {
    if get_user_by_id(db, id).await?.is_none() {
        return Err(not_found(&format!("User {id} not found.")));
    }
    let temporary_password = generate_temporary_password()?;
    let hash = hash_password(&temporary_password).await?;
    db.prepare("UPDATE wp_users SET user_pass = ? WHERE ID = ?")
        .bind(vec![Param::Text(hash), Param::Integer(id)])
        .run()
        .await?;
    revoke_all_sessions_for_user(db, id).await?;
    match get_user_by_id(db, id).await? {
        Some(user) => Ok((user, temporary_password)),
        None => Err(internal_error("Failed to read back the updated user.")),
    }
}

/// Clear the imported-user forced-reset flag (`wl_password_reset_required`).
/// Route-level step of `POST /api/users/:id/reset-password` in v1.
pub async fn clear_password_reset_required(
    db: &dyn Db,
    user_id: i64,
) -> Result<bool, WordlessError> {
    delete_user_meta(db, user_id, PASSWORD_RESET_META_KEY).await
}

/// Check a login/password pair. Returns the user on success, `None`
/// otherwise. (v1 `checkUserPassword` — login matches `user_login`
/// only; email is NOT accepted as the login identifier.)
pub async fn check_user_password(
    db: &dyn Db,
    login: &str,
    password: &str,
) -> Result<Option<User>, WordlessError> {
    let row = db
        .prepare("SELECT * FROM wp_users WHERE user_login = ? LIMIT 1")
        .bind(vec![Param::Text(login.to_string())])
        .first::<Row>()
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if !verify_password(password, &row_text(&row, "user_pass")).await {
        return Ok(None);
    }
    let id = row_int(&row, "ID");
    let role = role_for_user(db, id).await?;
    Ok(Some(row_to_user(&row, role)))
}

// ---------------------------------------------------------------- profiles
// (P4, §0.1 A — NEW in v2, no v1 equivalent. v1's users service has NO
// profile fields at all: no bio, no avatar, no theme. The keys below
// follow the WordPress conventions where one exists, and the `wl_*`
// prefix where v1/Rust invented the field:
//
// - bio: `wp_usermeta` key `description` — the WordPress core bio key
//   (WP stores it as plain text; Wordless stores every usermeta value
//   JSON-encoded, per v1 `updateUserMeta`, so it is a JSON string here).
// - avatar: `wp_usermeta` key `wl_avatar_id` — NEW, no v1 key exists
//   (v1 has no avatar concept; WordPress core uses Gravatar, not a
//   stored attachment). Value: the `wp_posts` ID of a media attachment
//   (`post_type = 'attachment'`), JSON-encoded integer; absent = no
//   custom avatar.
// - theme: `wp_usermeta` key `wl_admin_theme` — NEW, the per-user
//   dashboard theme (§0.1 A / doc §3 P4). Value: one of the 8 theme
//   ids in `ADMIN_THEMES`, JSON-encoded string; absent reads as the
//   default `"teal"`.
//
// A profile edit never touches `user_login`, `user_nicename` or the
// role — those stay on their own routes (`POST /api/users`,
// `PUT /api/users/:id/role`).)

/// The WordPress core bio key in `wp_usermeta`.
pub const BIO_META_KEY: &str = "description";
/// Avatar attachment id in `wp_usermeta` (NEW in P4 — no v1 key).
pub const AVATAR_META_KEY: &str = "wl_avatar_id";
/// Per-user dashboard theme in `wp_usermeta` (NEW in P4).
pub const ADMIN_THEME_META_KEY: &str = "wl_admin_theme";
/// Forced password change after an admin reset (NEW in P4 — see
/// `reset_user_password` and `change_password` below). Distinct from
/// the imported-user flag `wl_password_reset_required` (which BLOCKS
/// sign-in entirely): this flag lets the user sign in with the
/// temporary password and requires them to set a new one.
pub const PASSWORD_CHANGE_REQUIRED_META_KEY: &str = "wl_password_change_required";

/// The 8 dashboard themes (§3 P4 — the admin team's contract).
/// There is NO server-side default: when no theme is stored the
/// profile returns `theme: null`, so the admin can tell "never
/// picked" apart from an explicit `"teal"` pick and apply the
/// role-based default itself (admin-foundation contract fix, P4).
pub const ADMIN_THEMES: &[&str] = &[
    "teal", "midnight", "ocean", "forest", "sunset", "berry", "slate", "contrast",
];

/// Validate a dashboard theme id. 400 on anything outside `ADMIN_THEMES`.
pub fn validate_admin_theme(theme: &str) -> Result<String, WordlessError> {
    let t = theme.trim().to_lowercase();
    if ADMIN_THEMES.contains(&t.as_str()) {
        Ok(t)
    } else {
        Err(bad_request(
            "Field \"theme\" must be one of teal, midnight, ocean, forest, sunset, berry, slate, contrast.",
        ))
    }
}

/// A user with their editable profile fields attached.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserProfile {
    #[serde(flatten)]
    pub user: User,
    /// Bio (`description` usermeta), "" when unset.
    pub bio: String,
    /// Avatar attachment id (`wl_avatar_id`), `None` when unset.
    pub avatar_id: Option<i64>,
    /// Dashboard theme (`wl_admin_theme`): the stored id when the
    /// user explicitly picked one, `None` (JSON `null`) when nothing
    /// is stored — never a server-side default, so the admin can
    /// distinguish "unset" from an explicit `"teal"` and apply the
    /// role-based default itself. An invalid stored value also reads
    /// as `None` rather than inventing a theme.
    pub theme: Option<String>,
    /// True while an admin reset forces a password change on next login.
    pub must_change_password: bool,
}

/// Whether the user must set a new password (admin reset flag).
pub async fn is_password_change_required(db: &dyn Db, user_id: i64) -> Result<bool, WordlessError> {
    let flag = get_user_meta::<String>(
        db,
        user_id,
        PASSWORD_CHANGE_REQUIRED_META_KEY,
        String::new(),
    )
    .await?;
    Ok(flag == "1")
}

/// Set the forced-change flag (the admin-reset route's step).
pub async fn set_password_change_required(db: &dyn Db, user_id: i64) -> Result<(), WordlessError> {
    update_user_meta(db, user_id, PASSWORD_CHANGE_REQUIRED_META_KEY, &"1").await
}

/// Clear the forced-change flag (the change-password route's step).
pub async fn clear_password_change_required(
    db: &dyn Db,
    user_id: i64,
) -> Result<bool, WordlessError> {
    delete_user_meta(db, user_id, PASSWORD_CHANGE_REQUIRED_META_KEY).await
}

/// Full profile for one user (404 when unknown).
pub async fn get_user_profile(db: &dyn Db, id: i64) -> Result<UserProfile, WordlessError> {
    let Some(user) = get_user_by_id(db, id).await? else {
        return Err(not_found(&format!("User {id} not found.")));
    };
    let bio = get_user_meta::<String>(db, id, BIO_META_KEY, String::new()).await?;
    let avatar_raw = get_user_meta::<i64>(db, id, AVATAR_META_KEY, 0).await?;
    let avatar_id = if avatar_raw > 0 {
        Some(avatar_raw)
    } else {
        None
    };
    let stored_theme = get_user_meta::<String>(db, id, ADMIN_THEME_META_KEY, String::new()).await?;
    let theme = if stored_theme.is_empty() {
        None
    } else {
        validate_admin_theme(&stored_theme).ok()
    };
    let must_change_password = is_password_change_required(db, id).await?;
    Ok(UserProfile {
        user,
        bio,
        avatar_id,
        theme,
        must_change_password,
    })
}

#[derive(Debug, Clone, Default)]
pub struct UpdateProfileInput {
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub bio: Option<String>,
    /// `None` = field absent (leave alone); `Some(None)` = clear the
    /// avatar; `Some(Some(id))` = set it to attachment `id`.
    pub avatar: Option<Option<i64>>,
    pub theme: Option<String>,
}

/// Update a user's editable profile. Email uniqueness uses the same
/// conflict shape as user creation (`user_exists`, 409). Display name
/// is 1..=200 chars, bio at most 5000 chars, avatar (when set) must be
/// an existing media attachment, theme must be one of `ADMIN_THEMES`.
pub async fn update_user_profile(
    db: &dyn Db,
    id: i64,
    input: UpdateProfileInput,
) -> Result<UserProfile, WordlessError> {
    let Some(existing) = get_user_by_id(db, id).await? else {
        return Err(not_found(&format!("User {id} not found.")));
    };

    let display_name = match &input.display_name {
        Some(name) => Some(require_string(
            &serde_json::json!(name),
            "displayName",
            1,
            200,
        )?),
        None => None,
    };
    let email = match &input.email {
        Some(raw) => Some(require_email(&serde_json::json!(raw), "email")?),
        None => None,
    };
    if let Some(email) = &email {
        if *email != existing.email {
            let clash = db
                .prepare("SELECT ID FROM wp_users WHERE user_email = ? AND ID != ? LIMIT 1")
                .bind(vec![Param::Text(email.clone()), Param::Integer(id)])
                .first::<Row>()
                .await?;
            if clash.is_some() {
                return Err(conflict(
                    &format!("A user with email \"{email}\" already exists."),
                    "user_exists",
                ));
            }
        }
    }
    let bio = match &input.bio {
        Some(b) => {
            if b.len() > 5000 {
                return Err(bad_request(
                    "Field \"bio\" must be at most 5000 characters.",
                ));
            }
            Some(b.clone())
        }
        None => None,
    };
    let theme = match &input.theme {
        Some(t) => Some(validate_admin_theme(t)?),
        None => None,
    };
    if let Some(Some(avatar_id)) = input.avatar {
        if avatar_id < 0 {
            return Err(bad_request(
                "Field \"avatarId\" must be an existing media attachment id, or null to clear it.",
            ));
        }
        if avatar_id > 0 {
            let found = db
                .prepare(
                    "SELECT ID FROM wp_posts WHERE ID = ? AND post_type = 'attachment' LIMIT 1",
                )
                .bind(vec![Param::Integer(avatar_id)])
                .first::<Row>()
                .await?;
            if found.is_none() {
                return Err(bad_request(&format!(
                    "Field \"avatarId\" must be an existing media attachment id ({avatar_id} is not one)."
                )));
            }
        }
    }

    if let Some(name) = &display_name {
        db.prepare("UPDATE wp_users SET display_name = ? WHERE ID = ?")
            .bind(vec![Param::Text(name.clone()), Param::Integer(id)])
            .run()
            .await?;
    }
    if let Some(email) = &email {
        db.prepare("UPDATE wp_users SET user_email = ? WHERE ID = ?")
            .bind(vec![Param::Text(email.clone()), Param::Integer(id)])
            .run()
            .await?;
    }
    if let Some(bio) = &bio {
        update_user_meta(db, id, BIO_META_KEY, bio).await?;
    }
    match input.avatar {
        Some(Some(avatar_id)) => {
            update_user_meta(db, id, AVATAR_META_KEY, &avatar_id).await?;
        }
        Some(None) => {
            delete_user_meta(db, id, AVATAR_META_KEY).await?;
        }
        None => {}
    }
    if let Some(theme) = &theme {
        update_user_meta(db, id, ADMIN_THEME_META_KEY, theme).await?;
    }
    get_user_profile(db, id).await
}

// ------------------------------------------------------- forced password change
// (P4, §0.1 A. What v1 did, and what changes here:
//
// v1 (`src/index.ts` `POST /api/users/:id/reset-password` + M9 comment):
// an admin reset CLEARS `wl_password_reset_required`, so the user signs
// in with the temporary password and is merely *asked* by email to
// "change it as soon as you can" — nothing enforces it, and there is no
// change-password endpoint at all. The imported-user flag
// (`wl_password_reset_required = "1"`) is the only forced-reset
// mechanism in v1, and it BLOCKS sign-in (403) because an imported
// WordPress phpass hash has no known password to sign in with.
//
// §0.1 A requires the opposite for admin resets: the user signs in
// with the temporary password AND is forced to set a new one. Reusing
// the imported flag for that would block the very sign-in it must
// allow, so P4 adds a second flag, `wl_password_change_required`:
//   - admin reset SETS it (and still clears the imported flag, as v1 did);
//   - `login_guarded` is UNCHANGED: the imported flag still 403s,
//     the change flag does not block sign-in;
//   - the login and `/me` responses carry `mustChangePassword: true`
//     while the flag is set (route-level, additive — no v1 field changes);
//   - `POST /api/auth/change-password` (current + new, min 8 chars)
//     sets the hash, clears the flag, and revokes every session
//     (including the current one — the client signs in again with the
//     new password, which the smoke test does).
// A user who never changes the password keeps signing in with the
// temporary one but `mustChangePassword` stays true — the admin UI
// gates on it; the API does not lock the account out, because v1 never
// did either and a lockout would strand a user whose admin is away.)

/// Change the signed-in user's own password. Verifies the current
/// password, sets the new hash, clears the forced-change flag, and
/// revokes all of the user's sessions (they sign in again with the
/// new password). Returns `true` — the route body is
/// `{ "changed": true }`.
pub async fn change_password(
    db: &dyn Db,
    user_id: i64,
    current_password: &str,
    new_password: &str,
) -> Result<bool, WordlessError> {
    let Some(user) = get_user_by_id(db, user_id).await? else {
        return Err(not_found(&format!("User {user_id} not found.")));
    };
    if check_user_password(db, &user.login, current_password)
        .await?
        .is_none()
    {
        return Err(bad_request("That password is not correct."));
    }
    let new_password = require_string(&serde_json::json!(new_password), "newPassword", 8, 200)?;
    let hash = hash_password(&new_password).await?;
    db.prepare("UPDATE wp_users SET user_pass = ? WHERE ID = ?")
        .bind(vec![Param::Text(hash), Param::Integer(user_id)])
        .run()
        .await?;
    clear_password_change_required(db, user_id).await?;
    revoke_all_sessions_for_user(db, user_id).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_validation_matches_v1_regex() {
        assert!(is_valid_login("ann"));
        assert!(is_valid_login("john.doe-1_x"));
        assert!(!is_valid_login(""));
        assert!(!is_valid_login("has space"));
        assert!(!is_valid_login("bad@login"));
        assert!(!is_valid_login("bad/slash"));
        assert!(!is_valid_login("ümlaut"));
    }

    #[test]
    fn nicename_matches_v1_replace() {
        assert_eq!(nicename_from("Ann"), "ann");
        assert_eq!(nicename_from("John Doe"), "john-doe");
        assert_eq!(nicename_from("a  b--c"), "a-b-c");
        assert_eq!(nicename_from("  spaced  "), "-spaced-");
        assert_eq!(nicename_from("under_score.dot"), "under-score-dot");
    }

    #[test]
    fn temporary_password_shape() {
        for _ in 0..20 {
            let pw = generate_temporary_password().unwrap();
            assert_eq!(pw.len(), 16);
            assert!(pw.bytes().all(|b| TEMP_PASSWORD_CHARS.contains(&b)));
        }
        assert_ne!(
            generate_temporary_password().unwrap(),
            generate_temporary_password().unwrap()
        );
    }

    #[test]
    fn capabilities_meta_parsing_matches_v1() {
        assert_eq!(
            role_from_capabilities_meta("{\"administrator\":true}"),
            Role::Administrator
        );
        assert_eq!(
            role_from_capabilities_meta("{\"editor\":true}"),
            Role::Editor
        );
        assert_eq!(
            role_from_capabilities_meta("{\"subscriber\":true}"),
            Role::Subscriber
        );
        // First true role wins.
        assert_eq!(
            role_from_capabilities_meta("{\"author\":true,\"editor\":true}"),
            Role::Author
        );
        // False values do not count.
        assert_eq!(
            role_from_capabilities_meta("{\"administrator\":false}"),
            Role::Subscriber
        );
        // Corrupt / unknown values fall back to subscriber.
        assert_eq!(role_from_capabilities_meta("not json"), Role::Subscriber);
        assert_eq!(
            role_from_capabilities_meta("{\"superadmin\":true}"),
            Role::Subscriber
        );
        assert_eq!(role_from_capabilities_meta("{}"), Role::Subscriber);
    }

    #[test]
    fn admin_theme_validation_accepts_only_the_eight_ids() {
        for theme in ADMIN_THEMES {
            assert_eq!(validate_admin_theme(theme).unwrap(), *theme);
        }
        assert_eq!(validate_admin_theme("  Forest ").unwrap(), "forest");
        assert!(validate_admin_theme("teal-dark").is_err());
        assert!(validate_admin_theme("").is_err());
        assert!(validate_admin_theme("light").is_err());
    }

    #[test]
    fn theme_is_null_when_unset_and_teal_only_when_picked() {
        // Contract fix (admin-foundation worker): the profile must
        // return `theme: null` while nothing is stored, so the admin
        // can tell "unset" from an explicit `"teal"` pick and apply
        // the role-based default itself.
        use crate::test_db::TestDb;
        let db = TestDb::new();
        futures_executor::block_on(async {
            let user = create_user(
                &db,
                CreateUserInput {
                    login: "eve".to_string(),
                    email: "eve@example.com".to_string(),
                    password: "password-123".to_string(),
                    display_name: None,
                    nicename: None,
                    url: None,
                    role: None,
                },
            )
            .await
            .unwrap();
            let profile = get_user_profile(&db, user.id).await.unwrap();
            assert_eq!(profile.theme, None);
            // Wire shape: the field is present and JSON null.
            let wire = serde_json::to_value(&profile).unwrap();
            assert!(wire.get("theme").is_some());
            assert_eq!(wire["theme"], serde_json::Value::Null);
            // An explicit teal pick is stored and reads back as "teal".
            let picked = update_user_profile(
                &db,
                user.id,
                UpdateProfileInput {
                    theme: Some("teal".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            assert_eq!(picked.theme, Some("teal".to_string()));
            assert_eq!(serde_json::to_value(&picked).unwrap()["theme"], "teal");
        });
    }

    #[test]
    fn profile_update_round_trips_and_validates() {
        use crate::test_db::TestDb;
        let db = TestDb::new();
        futures_executor::block_on(async {
            let user = create_user(
                &db,
                CreateUserInput {
                    login: "ann".to_string(),
                    email: "ann@example.com".to_string(),
                    password: "password-123".to_string(),
                    display_name: Some("Ann".to_string()),
                    nicename: None,
                    url: None,
                    role: None,
                },
            )
            .await
            .unwrap();
            // Defaults: empty bio, no avatar, theme NULL when unset
            // (contract fix: unset is distinguishable from an explicit
            // "teal" pick, so the admin can apply role defaults).
            let profile = get_user_profile(&db, user.id).await.unwrap();
            assert_eq!(profile.bio, "");
            assert_eq!(profile.avatar_id, None);
            assert_eq!(profile.theme, None);
            assert!(!profile.must_change_password);

            // Avatar 1 is not an attachment (no wp_posts row) → 400.
            let bad_avatar = update_user_profile(
                &db,
                user.id,
                UpdateProfileInput {
                    avatar: Some(Some(1)),
                    ..Default::default()
                },
            )
            .await;
            assert!(bad_avatar.is_err());
            assert_eq!(bad_avatar.unwrap_err().status, 400);

            // Seed a real attachment row, then set bio/theme/displayName.
            db.prepare(
                "INSERT INTO wp_posts (ID, post_author, post_date, post_date_gmt, post_content, post_title, post_excerpt, post_status, post_name, post_modified, post_modified_gmt, post_parent, post_type, post_mime_type, comment_status) VALUES (7, 1, '2026-11-19T00:00:00.000Z', '2026-11-19T00:00:00.000Z', '', 'pic.jpg', '', 'publish', 'pic', '2026-11-19T00:00:00.000Z', '2026-11-19T00:00:00.000Z', 0, 'attachment', 'image/jpeg', 'closed')",
            )
            .bind(vec![])
            .run()
            .await
            .unwrap();
            let updated = update_user_profile(
                &db,
                user.id,
                UpdateProfileInput {
                    display_name: Some("Ann Other".to_string()),
                    bio: Some("Writer and editor.".to_string()),
                    avatar: Some(Some(7)),
                    theme: Some("forest".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            assert_eq!(updated.user.display_name, "Ann Other");
            assert_eq!(updated.bio, "Writer and editor.");
            assert_eq!(updated.avatar_id, Some(7));
            assert_eq!(updated.theme, Some("forest".to_string()));

            // Invalid theme → 400, nothing applied.
            let bad_theme = update_user_profile(
                &db,
                user.id,
                UpdateProfileInput {
                    theme: Some("neon".to_string()),
                    ..Default::default()
                },
            )
            .await;
            assert!(bad_theme.is_err());
            assert_eq!(bad_theme.unwrap_err().status, 400);
            assert_eq!(
                get_user_profile(&db, user.id).await.unwrap().theme,
                Some("forest".to_string())
            );

            // Clearing the avatar, and email uniqueness (409 user_exists).
            let cleared = update_user_profile(
                &db,
                user.id,
                UpdateProfileInput {
                    avatar: Some(None),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            assert_eq!(cleared.avatar_id, None);
            let other = create_user(
                &db,
                CreateUserInput {
                    login: "bob".to_string(),
                    email: "bob@example.com".to_string(),
                    password: "password-123".to_string(),
                    display_name: None,
                    nicename: None,
                    url: None,
                    role: None,
                },
            )
            .await
            .unwrap();
            let clash = update_user_profile(
                &db,
                other.id,
                UpdateProfileInput {
                    email: Some("ann@example.com".to_string()),
                    ..Default::default()
                },
            )
            .await;
            assert!(clash.is_err());
            let err = clash.unwrap_err();
            assert_eq!(err.status, 409);
            assert_eq!(err.code, "user_exists");
            // Same email for the same user is fine.
            let same = update_user_profile(
                &db,
                user.id,
                UpdateProfileInput {
                    email: Some("ann@example.com".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            assert_eq!(same.user.email, "ann@example.com");
            // Unknown user → 404.
            assert!(get_user_profile(&db, 9999).await.is_err());
        });
    }

    #[test]
    fn forced_reset_flow_temp_login_then_change_then_normal_login() {
        use crate::guarded::login_guarded;
        use crate::test_db::TestDb;
        use wordless_kernel::time::now_ms;
        let db = TestDb::new();
        futures_executor::block_on(async {
            let user = create_user(
                &db,
                CreateUserInput {
                    login: "carol".to_string(),
                    email: "carol@example.com".to_string(),
                    password: "original-pass-1".to_string(),
                    display_name: None,
                    nicename: None,
                    url: None,
                    role: None,
                },
            )
            .await
            .unwrap();
            // Admin reset: new temp password + forced-change flag set
            // (the route's step — the service only hashes + revokes).
            let (_u, temp) = reset_user_password(&db, user.id).await.unwrap();
            set_password_change_required(&db, user.id).await.unwrap();
            assert!(is_password_change_required(&db, user.id).await.unwrap());

            // The temporary password signs in (unlike the imported-user
            // flag, which 403s) — the flag is a signal, not a block.
            let outcome = login_guarded(&db, "carol", &temp, None, None, None, now_ms())
                .await
                .unwrap();
            assert!(matches!(outcome, crate::LoginOutcome::Ok { .. }));

            // Wrong current password → 400; flag stays set.
            assert!(
                change_password(&db, user.id, "not-the-temp", "brand-new-pass-2")
                    .await
                    .is_err()
            );
            assert!(is_password_change_required(&db, user.id).await.unwrap());

            // Change with the temp password as current: clears the flag,
            // revokes sessions, new password works, temp does not.
            assert!(change_password(&db, user.id, &temp, "brand-new-pass-2")
                .await
                .unwrap());
            assert!(!is_password_change_required(&db, user.id).await.unwrap());
            assert!(check_user_password(&db, "carol", "brand-new-pass-2")
                .await
                .unwrap()
                .is_some());
            assert!(check_user_password(&db, "carol", &temp)
                .await
                .unwrap()
                .is_none());
            // And a normal guarded login now succeeds with the new password.
            let outcome2 =
                login_guarded(&db, "carol", "brand-new-pass-2", None, None, None, now_ms())
                    .await
                    .unwrap();
            assert!(matches!(outcome2, crate::LoginOutcome::Ok { .. }));
        });
    }

    #[test]
    fn imported_flag_still_blocks_sign_in() {
        // The v1 imported-user flag is untouched by P4: a user with
        // `wl_password_reset_required = "1"` still gets ForcedReset,
        // even with a correct password.
        use crate::guarded::login_guarded;
        use crate::test_db::TestDb;
        use wordless_kernel::time::now_ms;
        let db = TestDb::new();
        futures_executor::block_on(async {
            let user = create_user(
                &db,
                CreateUserInput {
                    login: "dave".to_string(),
                    email: "dave@example.com".to_string(),
                    password: "dave-pass-123".to_string(),
                    display_name: None,
                    nicename: None,
                    url: None,
                    role: None,
                },
            )
            .await
            .unwrap();
            update_user_meta(&db, user.id, PASSWORD_RESET_META_KEY, &"1")
                .await
                .unwrap();
            let outcome = login_guarded(&db, "dave", "dave-pass-123", None, None, None, now_ms())
                .await
                .unwrap();
            assert!(matches!(outcome, crate::LoginOutcome::ForcedReset));
        });
    }
}
