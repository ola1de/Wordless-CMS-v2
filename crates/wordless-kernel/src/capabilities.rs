//! Roles and capabilities, mirroring WordPress default roles.
//! (Port of v1 `src/modules/users/capabilities.ts` — same roles, same
//! capability lists, same names, so converted extensions calling
//! `current_user_can()` keep working.)

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Administrator,
    Editor,
    Author,
    Contributor,
    Subscriber,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Administrator => "administrator",
            Role::Editor => "editor",
            Role::Author => "author",
            Role::Contributor => "contributor",
            Role::Subscriber => "subscriber",
        }
    }

    pub fn from_str(s: &str) -> Option<Role> {
        match s {
            "administrator" => Some(Role::Administrator),
            "editor" => Some(Role::Editor),
            "author" => Some(Role::Author),
            "contributor" => Some(Role::Contributor),
            "subscriber" => Some(Role::Subscriber),
            _ => None,
        }
    }
}

pub const ALL_ROLES: [Role; 5] = [
    Role::Administrator,
    Role::Editor,
    Role::Author,
    Role::Contributor,
    Role::Subscriber,
];

pub fn is_role(value: &str) -> bool {
    Role::from_str(value).is_some()
}

pub fn capabilities_for(role: Role) -> &'static [&'static str] {
    match role {
        Role::Administrator => &[
            "read",
            "edit_posts",
            "publish_posts",
            "edit_pages",
            "publish_pages",
            "edit_others_posts",
            "edit_others_pages",
            "delete_posts",
            "delete_pages",
            "delete_others_posts",
            "upload_files",
            "moderate_comments",
            "edit_users",
            "manage_options",
            "manage_categories",
            "install_plugins",
            "install_themes",
            "activate_plugins",
            "switch_themes",
            "edit_theme_options",
            "manage_links",
            "import",
            "unfiltered_html",
        ],
        Role::Editor => &[
            "read",
            "edit_posts",
            "publish_posts",
            "edit_pages",
            "publish_pages",
            "edit_others_posts",
            "edit_others_pages",
            "delete_posts",
            "delete_pages",
            "delete_others_posts",
            "upload_files",
            "moderate_comments",
            "manage_categories",
            "manage_links",
        ],
        Role::Author => &[
            "read",
            "edit_posts",
            "publish_posts",
            "delete_posts",
            "upload_files",
        ],
        Role::Contributor => &["read", "edit_posts", "delete_posts"],
        Role::Subscriber => &["read"],
    }
}

/// A request-scoped, authenticated user (null = anonymous visitor).
/// This is the kernel's view of a user: what the router guards need.
/// The full `User` record lives in `wordless-auth`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthUser {
    pub id: i64,
    pub login: String,
    pub email: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    pub role: Role,
}

impl AuthUser {
    /// The exact JSON shape v1 returns for the logged-in user
    /// (`{id, login, email, displayName, role}`).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "login": self.login,
            "email": self.email,
            "displayName": self.display_name,
            "role": self.role.as_str(),
        })
    }
}

pub fn user_can(user: &AuthUser, capability: &str) -> bool {
    capabilities_for(user.role).contains(&capability)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_has_everything_subscriber_has_read_only() {
        assert!(user_can(
            &AuthUser {
                id: 1,
                login: "admin".into(),
                email: "a@example.com".into(),
                display_name: "Admin".into(),
                role: Role::Administrator,
            },
            "manage_options"
        ));
        assert!(!user_can(
            &AuthUser {
                id: 2,
                login: "sub".into(),
                email: "s@example.com".into(),
                display_name: "Sub".into(),
                role: Role::Subscriber,
            },
            "edit_posts"
        ));
    }

    #[test]
    fn role_round_trips() {
        for role in ALL_ROLES {
            assert_eq!(Role::from_str(role.as_str()), Some(role));
        }
        assert_eq!(Role::from_str("superadmin"), None);
    }
}
