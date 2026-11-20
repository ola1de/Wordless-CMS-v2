//! Wire shapes for the social API (camelCase, v1 envelope).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A member's extended profile. `login`/`registered` are filled for
/// directory entries; profile reads carry the owner's `wp_users` data.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SocialProfile {
    pub user_id: i64,
    pub login: String,
    pub display_name: String,
    pub bio: String,
    pub avatar_id: Option<i64>,
    pub cover_id: Option<i64>,
    #[serde(default = "default_fields")]
    pub fields: Value,
    pub visibility: String,
    pub registered: String,
    pub created_at: String,
    pub updated_at: String,
}

fn default_fields() -> Value {
    Value::Object(Default::default())
}

/// Input for `PUT /api/social/profile` — every field optional.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateProfileInput {
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub avatar_id: Option<i64>,
    pub cover_id: Option<i64>,
    pub fields: Option<Value>,
    pub visibility: Option<String>,
}

/// One member-directory row.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberEntry {
    pub user_id: i64,
    pub login: String,
    pub display_name: String,
    pub bio: String,
    pub avatar_id: Option<i64>,
    pub visibility: String,
    /// From the viewer's perspective: none | pending_sent |
    /// pending_received | accepted | blocked.
    pub friend_status: String,
    pub registered: String,
}

/// One friend (or pending request) in the friends list.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FriendEntry {
    pub user_id: i64,
    pub login: String,
    pub display_name: String,
    /// pending | accepted | blocked, from the caller's perspective.
    pub status: String,
    /// True when the other user sent the pending request.
    pub incoming: bool,
    pub created_at: String,
}

/// One activity-feed item.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityItem {
    pub id: i64,
    pub actor_id: i64,
    pub actor_name: String,
    pub actor_login: String,
    #[serde(rename = "type")]
    pub item_type: String,
    pub object_type: String,
    pub object_id: i64,
    pub content: String,
    pub visibility: String,
    pub group_id: Option<i64>,
    pub group_name: Option<String>,
    pub status: String,
    pub like_count: i64,
    pub liked_by_me: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// One group.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub visibility: String,
    pub avatar_id: Option<i64>,
    pub created_by: i64,
    pub member_count: i64,
    /// The caller's membership: none | member | requested | invited | banned.
    pub my_status: String,
    /// The caller's role when a member: member | moderator | admin.
    pub my_role: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// One group member.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupMember {
    pub user_id: i64,
    pub login: String,
    pub display_name: String,
    pub role: String,
    pub status: String,
    pub created_at: String,
}

/// One direct-message thread (two participants).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Thread {
    pub id: i64,
    pub other_user_id: i64,
    pub other_login: String,
    pub other_display_name: String,
    pub last_message: Option<String>,
    pub last_message_at: Option<String>,
    pub unread_count: i64,
    pub updated_at: String,
}

/// One message inside a thread.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: i64,
    pub thread_id: i64,
    pub sender_id: i64,
    pub sender_name: String,
    pub body: String,
    pub created_at: String,
}

/// One in-app notification.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub id: i64,
    pub user_id: i64,
    #[serde(rename = "type")]
    pub notif_type: String,
    pub actor_id: i64,
    pub actor_name: String,
    pub object_type: String,
    pub object_id: i64,
    pub read_at: Option<String>,
    pub created_at: String,
}
