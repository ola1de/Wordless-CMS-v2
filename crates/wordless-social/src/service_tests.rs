//! End-to-end service tests on the real SQL (TestDb, migrations
//! 0001 + 0013). One test per acceptance flow; visibility rules
//! themselves are unit-tested in `visibility`.

use crate::activity;
use crate::friends;
use crate::groups;
use crate::messages;
use crate::notifications;
use crate::profiles;
use crate::test_support::{block_on, TestDb};

fn db_with_users() -> TestDb {
    let db = TestDb::new();
    db.seed_user(1, "alice", "Alice");
    db.seed_user(2, "bob", "Bob");
    db.seed_user(3, "carol", "Carol");
    db
}

#[test]
fn friend_request_accept_flow_notifies_both_ways() {
    block_on(async {
        let db = db_with_users();
        let n = friends::request(&db, 1, 2).await.unwrap();
        assert!(n.is_some(), "addressee is notified of the request");
        assert_eq!(friends::list_friends(&db, 2, true).await.unwrap().len(), 1);
        assert_eq!(friends::list_friends(&db, 1, true).await.unwrap().len(), 1);
        // Duplicate request is a conflict, not a second row.
        assert!(friends::request(&db, 1, 2).await.is_err());
        let n2 = friends::accept(&db, 2, 1).await.unwrap();
        assert!(n2.is_some(), "requester is notified of the acceptance");
        assert!(friends::is_friend(&db, 1, 2).await.unwrap());
        assert!(friends::is_friend(&db, 2, 1).await.unwrap());
        assert_eq!(friends::list_friends(&db, 1, false).await.unwrap().len(), 1);
        // Removing ends the friendship.
        friends::remove(&db, 1, 2).await.unwrap();
        assert!(!friends::is_friend(&db, 1, 2).await.unwrap());
    });
}

#[test]
fn block_hides_profile_feed_and_messages() {
    block_on(async {
        let db = db_with_users();
        friends::request(&db, 1, 2).await.unwrap();
        friends::accept(&db, 2, 1).await.unwrap();
        activity::post(&db, 1, "hello world", "public", None)
            .await
            .unwrap();
        assert_eq!(activity::feed(&db, 2, None, 1, 20).await.unwrap().len(), 1);
        friends::block(&db, 2, 1).await.unwrap();
        assert!(!friends::is_friend(&db, 1, 2).await.unwrap());
        // Profile view is indistinguishable from a missing user.
        assert!(profiles::view_profile(&db, 2, 1, false).await.is_err());
        // The blocked actor's posts vanish from the feed.
        assert_eq!(activity::feed(&db, 2, None, 1, 20).await.unwrap().len(), 0);
        // No new friend request across the block.
        assert!(friends::request(&db, 1, 2).await.is_err());
        // No direct message across the block.
        assert!(messages::get_or_create_thread(&db, 1, 2).await.is_err());
        // Unblock restores visibility (but not the friendship).
        friends::unblock(&db, 2, 1).await.unwrap();
        assert!(profiles::view_profile(&db, 2, 1, false).await.is_ok());
        assert!(!friends::is_friend(&db, 1, 2).await.unwrap());
    });
}

#[test]
fn activity_visibility_rules_in_the_feed() {
    block_on(async {
        let db = db_with_users();
        friends::request(&db, 1, 2).await.unwrap();
        friends::accept(&db, 2, 1).await.unwrap();
        activity::post(&db, 1, "public post", "public", None)
            .await
            .unwrap();
        activity::post(&db, 1, "friends post", "friends", None)
            .await
            .unwrap();
        activity::post(&db, 1, "private post", "private", None)
            .await
            .unwrap();
        // Friend sees public + friends, not private.
        let friend_feed = activity::feed(&db, 2, None, 1, 20).await.unwrap();
        assert_eq!(friend_feed.len(), 2);
        // Stranger sees public only.
        let stranger_feed = activity::feed(&db, 3, None, 1, 20).await.unwrap();
        assert_eq!(stranger_feed.len(), 1);
        assert_eq!(stranger_feed[0].content, "public post");
        // Actor sees all three.
        assert_eq!(activity::feed(&db, 1, None, 1, 20).await.unwrap().len(), 3);
        // Profile feed of the actor, seen by the stranger: public only.
        let profile_feed = activity::feed(&db, 3, Some(1), 1, 20).await.unwrap();
        assert_eq!(profile_feed.len(), 1);
    });
}

#[test]
fn like_notifies_and_unlike_clears() {
    block_on(async {
        let db = db_with_users();
        let (item, _) = activity::post(&db, 1, "like me", "public", None)
            .await
            .unwrap();
        let n = activity::like(&db, 2, item.id).await.unwrap();
        assert!(n.is_some(), "actor is notified of the like");
        let feed = activity::feed(&db, 2, None, 1, 20).await.unwrap();
        assert_eq!(feed[0].like_count, 1);
        assert!(feed[0].liked_by_me);
        // Second like is idempotent, not a second row.
        activity::like(&db, 2, item.id).await.unwrap();
        let feed = activity::feed(&db, 2, None, 1, 20).await.unwrap();
        assert_eq!(feed[0].like_count, 1);
        activity::unlike(&db, 2, item.id).await.unwrap();
        let feed = activity::feed(&db, 2, None, 1, 20).await.unwrap();
        assert_eq!(feed[0].like_count, 0);
        assert!(!feed[0].liked_by_me);
        // A stranger cannot like a friends-only item they cannot see.
        let (item2, _) = activity::post(&db, 1, "friends only", "friends", None)
            .await
            .unwrap();
        assert!(activity::like(&db, 3, item2.id).await.is_err());
    });
}

#[test]
fn mention_notifies_the_mentioned_user() {
    block_on(async {
        let db = db_with_users();
        let (_, notifs) = activity::post(&db, 1, "hey @bob, look at this", "public", None)
            .await
            .unwrap();
        assert_eq!(notifs.len(), 1);
        assert_eq!(notifs[0].user_id, 2);
        assert_eq!(notifs[0].notif_type, "activity_mention");
        let (list, unread, _) = notifications::list(&db, 2, false, 1, 20).await.unwrap();
        assert_eq!(unread, 1);
        assert_eq!(list.len(), 1);
        notifications::mark_all_read(&db, 2).await.unwrap();
        assert_eq!(notifications::unread_count(&db, 2).await.unwrap(), 0);
    });
}

#[test]
fn private_group_isolation_and_join_request_flow() {
    block_on(async {
        let db = db_with_users();
        let group = groups::create(&db, 1, "Secret Club", "members only", "private")
            .await
            .unwrap();
        // Non-member cannot join directly; files a request instead.
        let (g, notif) = groups::join(&db, 2, group.id).await.unwrap();
        assert_eq!(g.my_status, "requested");
        assert!(notif.is_some(), "group admin is notified of the request");
        // Non-member cannot see group activity or members.
        activity::post(&db, 1, "inside the club", "public", Some(group.id))
            .await
            .unwrap();
        assert!(activity::group_feed(&db, 2, group.id, 1, 20, false)
            .await
            .is_err());
        assert!(groups::members(&db, 2, group.id, false).await.is_err());
        // Admin approves; member sees the activity.
        let n = groups::approve(&db, 1, group.id, 2).await.unwrap();
        assert!(n.is_some());
        let feed = activity::group_feed(&db, 2, group.id, 1, 20, false)
            .await
            .unwrap();
        assert_eq!(feed.len(), 1);
        assert_eq!(feed[0].content, "inside the club");
        // Public group: direct join, no request.
        let pub_group = groups::create(&db, 1, "Open Club", "everyone welcome", "public")
            .await
            .unwrap();
        let (g2, _) = groups::join(&db, 3, pub_group.id).await.unwrap();
        assert_eq!(g2.my_status, "member");
    });
}

#[test]
fn hidden_group_invisible_to_non_members() {
    block_on(async {
        let db = db_with_users();
        let hidden = groups::create(&db, 1, "Hidden Club", "you cannot see this", "hidden")
            .await
            .unwrap();
        // Not listed for strangers, 404 for direct read.
        let (list, _) = groups::list(&db, 2, "", 1, 20, false).await.unwrap();
        assert!(list.iter().all(|g| g.id != hidden.id));
        assert!(groups::get(&db, hidden.id, 2, false)
            .await
            .unwrap()
            .is_none());
        // Join directly is refused; invite works.
        assert!(groups::join(&db, 2, hidden.id).await.is_err());
        groups::invite(&db, 1, hidden.id, 2).await.unwrap();
        let (g, _) = groups::join(&db, 2, hidden.id).await.unwrap();
        assert_eq!(g.my_status, "member");
        let (list, _) = groups::list(&db, 2, "", 1, 20, false).await.unwrap();
        assert!(list.iter().any(|g| g.id == hidden.id));
    });
}

#[test]
fn messaging_requires_friendship_and_tracks_read_state() {
    block_on(async {
        let db = db_with_users();
        // Strangers cannot message.
        assert!(messages::get_or_create_thread(&db, 1, 2).await.is_err());
        friends::request(&db, 1, 2).await.unwrap();
        friends::accept(&db, 2, 1).await.unwrap();
        let (t1, _) = messages::get_or_create_thread(&db, 1, 2).await.unwrap();
        // Same thread from the other side.
        let (t2, _) = messages::get_or_create_thread(&db, 2, 1).await.unwrap();
        assert_eq!(t1.id, t2.id);
        let (msg, notif) = messages::send(&db, 1, t1.id, "hello bob").await.unwrap();
        assert_eq!(msg.body, "hello bob");
        assert!(notif.is_some(), "recipient is notified of the message");
        let threads = messages::list_threads(&db, 2, 1, 20).await.unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].unread_count, 1);
        assert_eq!(threads[0].last_message.as_deref(), Some("hello bob"));
        messages::mark_read(&db, 2, t1.id).await.unwrap();
        let threads = messages::list_threads(&db, 2, 1, 20).await.unwrap();
        assert_eq!(threads[0].unread_count, 0);
        let list = messages::list_messages(&db, 2, t1.id, 1, 20).await.unwrap();
        assert_eq!(list.len(), 1);
        // A third user cannot read the thread.
        assert!(messages::list_messages(&db, 3, t1.id, 1, 20).await.is_err());
    });
}

#[test]
fn directory_hides_private_profiles_from_strangers() {
    block_on(async {
        let db = db_with_users();
        profiles::update_profile(
            &db,
            2,
            &crate::types::UpdateProfileInput {
                visibility: Some("private".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let (list, total) = profiles::list_members(&db, 3, "", 1, 20, false)
            .await
            .unwrap();
        assert_eq!(total, 2, "alice + carol; bob's private profile is hidden");
        assert!(list.iter().all(|m| m.user_id != 2));
        // Bob still sees himself; admin sees everyone.
        let (list, total) = profiles::list_members(&db, 2, "", 1, 20, false)
            .await
            .unwrap();
        assert_eq!(total, 3);
        assert!(list.iter().any(|m| m.user_id == 2));
        let (list, total) = profiles::list_members(&db, 1, "", 1, 20, true)
            .await
            .unwrap();
        assert_eq!(total, 3);
        assert!(list.iter().any(|m| m.user_id == 2));
        // Search matches display name.
        let (list, total) = profiles::list_members(&db, 3, "ali", 1, 20, false)
            .await
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(list[0].login, "alice");
    });
}

#[test]
fn moderator_hide_removes_item_from_every_feed() {
    block_on(async {
        let db = db_with_users();
        friends::request(&db, 1, 2).await.unwrap();
        friends::accept(&db, 2, 1).await.unwrap();
        let (item, _) = activity::post(&db, 1, "abusive content", "public", None)
            .await
            .unwrap();
        assert_eq!(activity::feed(&db, 2, None, 1, 20).await.unwrap().len(), 1);
        activity::hide(&db, 3, item.id).await.unwrap();
        assert_eq!(activity::feed(&db, 2, None, 1, 20).await.unwrap().len(), 0);
        assert_eq!(activity::feed(&db, 1, None, 1, 20).await.unwrap().len(), 0);
        // The admin list still shows it (for review), marked hidden.
        let admin = activity::admin_list(&db, 1, 20).await.unwrap();
        assert_eq!(admin.len(), 1);
        assert_eq!(admin[0].status, "hidden");
    });
}
