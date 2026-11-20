//! Pure visibility rules for the social module — no I/O, so each rule
//! gets its own unit test here instead of a hope. Callers resolve the
//! inputs (friendship, membership, block) from the DB via `helpers`
//! and pass plain booleans in.
//!
//! Invariants:
//! - A block in EITHER direction hides both users from each other
//!   everywhere: profiles, feed, directory, messages, friend requests.
//! - Hidden/deleted activity never appears in any feed, for anyone
//!   except the admin moderation list (which reads the table directly).
//! - Admins may view any profile or group for moderation, but admin
//!   does NOT override a block (a blocked admin still cannot message
//!   the user who blocked them).

/// Can `viewer` see `owner`'s profile? `visibility` is the profile's
/// setting; `are_friends` is mutual accepted friendship.
pub fn can_view_profile(
    viewer: i64,
    owner: i64,
    visibility: &str,
    are_friends: bool,
    is_admin: bool,
    blocked: bool,
) -> bool {
    if blocked {
        return false;
    }
    if viewer == owner || is_admin {
        return true;
    }
    match visibility {
        "public" => true,
        "friends" => are_friends,
        "private" => false,
        _ => false,
    }
}

/// Can `viewer` see this activity item? `visibility` is the item's
/// setting; `is_group_member` is membership of the item's group.
pub fn can_view_activity(
    viewer: i64,
    actor: i64,
    visibility: &str,
    is_friend: bool,
    is_group_member: bool,
    is_admin: bool,
    blocked: bool,
) -> bool {
    if blocked {
        return false;
    }
    if viewer == actor {
        return true;
    }
    match visibility {
        "public" => true,
        "friends" => is_friend || is_admin,
        "group" => is_group_member || is_admin,
        "private" => false,
        _ => false,
    }
}

/// Can `a` send a direct message to `b`? Requires friendship OR shared
/// group membership; never across a block.
pub fn can_message(a: i64, b: i64, are_friends: bool, share_group: bool, blocked: bool) -> bool {
    if a == b || blocked {
        return false;
    }
    are_friends || share_group
}

/// Can `viewer` see that this group exists (directory listing)?
pub fn can_view_group(viewer: i64, visibility: &str, is_member: bool, is_admin: bool) -> bool {
    if viewer == 0 && visibility == "hidden" {
        return false;
    }
    match visibility {
        "public" | "private" => true,
        "hidden" => is_member || is_admin,
        _ => false,
    }
}

/// Can `viewer` see this group's content (activity, member list)?
pub fn can_view_group_content(visibility: &str, is_member: bool, is_admin: bool) -> bool {
    match visibility {
        "public" => true,
        "private" | "hidden" => is_member || is_admin,
        _ => false,
    }
}

/// Can `viewer` join this group directly (no request needed)?
/// Private groups need a request; hidden groups need an invite.
pub fn can_join_directly(visibility: &str) -> bool {
    visibility == "public"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_public_visible_to_anyone() {
        assert!(can_view_profile(2, 1, "public", false, false, false));
        assert!(can_view_profile(0, 1, "public", false, false, false));
    }

    #[test]
    fn profile_friends_only_for_friends() {
        assert!(can_view_profile(2, 1, "friends", true, false, false));
        assert!(!can_view_profile(2, 1, "friends", false, false, false));
    }

    #[test]
    fn profile_private_only_self_and_admin() {
        assert!(can_view_profile(1, 1, "private", false, false, false));
        assert!(can_view_profile(99, 1, "private", false, true, false));
        assert!(!can_view_profile(2, 1, "private", true, false, false));
    }

    #[test]
    fn profile_block_beats_everything() {
        assert!(!can_view_profile(1, 1, "public", false, false, true));
        assert!(!can_view_profile(99, 1, "public", false, true, true));
        assert!(!can_view_profile(2, 1, "friends", true, false, true));
    }

    #[test]
    fn activity_public_visible_to_anyone() {
        assert!(can_view_activity(
            2, 1, "public", false, false, false, false
        ));
    }

    #[test]
    fn activity_friends_only_for_friends_or_admin() {
        assert!(can_view_activity(
            2, 1, "friends", true, false, false, false
        ));
        assert!(!can_view_activity(
            2, 1, "friends", false, false, false, false
        ));
        assert!(can_view_activity(
            99, 1, "friends", false, false, true, false
        ));
    }

    #[test]
    fn activity_group_only_for_members_or_admin() {
        assert!(can_view_activity(2, 1, "group", false, true, false, false));
        assert!(!can_view_activity(2, 1, "group", true, false, false, false));
        assert!(can_view_activity(99, 1, "group", false, false, true, false));
    }

    #[test]
    fn activity_private_only_actor() {
        assert!(can_view_activity(
            1, 1, "private", false, false, false, false
        ));
        assert!(!can_view_activity(
            2, 1, "private", true, true, false, false
        ));
        assert!(!can_view_activity(
            99, 1, "private", false, false, true, false
        ));
    }

    #[test]
    fn activity_block_beats_everything() {
        assert!(!can_view_activity(
            1, 1, "public", false, false, false, true
        ));
        assert!(!can_view_activity(
            2, 1, "public", false, false, false, true
        ));
    }

    #[test]
    fn messaging_needs_friendship_or_shared_group() {
        assert!(can_message(1, 2, true, false, false));
        assert!(can_message(1, 2, false, true, false));
        assert!(!can_message(1, 2, false, false, false));
        assert!(!can_message(1, 1, true, true, false));
    }

    #[test]
    fn messaging_never_across_a_block() {
        assert!(!can_message(1, 2, true, true, true));
    }

    #[test]
    fn group_listing_hides_hidden_from_non_members() {
        assert!(can_view_group(2, "public", false, false));
        assert!(can_view_group(2, "private", false, false));
        assert!(!can_view_group(2, "hidden", false, false));
        assert!(can_view_group(2, "hidden", true, false));
        assert!(can_view_group(99, "hidden", false, true));
        assert!(!can_view_group(0, "hidden", false, false));
    }

    #[test]
    fn group_content_public_open_private_and_hidden_members_only() {
        assert!(can_view_group_content("public", false, false));
        assert!(!can_view_group_content("private", false, false));
        assert!(can_view_group_content("private", true, false));
        assert!(!can_view_group_content("hidden", false, false));
        assert!(can_view_group_content("hidden", false, true));
    }

    #[test]
    fn join_directly_only_for_public_groups() {
        assert!(can_join_directly("public"));
        assert!(!can_join_directly("private"));
        assert!(!can_join_directly("hidden"));
    }
}
