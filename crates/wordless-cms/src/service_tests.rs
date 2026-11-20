//! Service-level integration tests against real SQLite (migrations
//! 0001 + 0002 applied by `crate::test_db::TestDb`). These exercise the
//! same SQL D1 executes, with the same bound parameters.

use futures_executor::block_on;
use serde_json::json;

use crate::activity::{list_activity, log, log_activity, ListActivityOptions, LogActivityInput};
use crate::comments::{
    count_links, create_comment, delete_comment, get_comment, list_approved_threaded,
    list_comments, set_comment_status, thread_comments, Comment, CommentStatus, CreateCommentInput,
    ListCommentsOptions,
};
use crate::menus::{
    add_menu_item, create_menu, delete_menu, get_menu_locations, list_menu_items, list_menus,
    remove_menu_item, resolve_menu_by_location, set_menu_location, update_menu_item,
    CreateMenuItemInput, UpdateMenuItemInput,
};
use crate::options::{add_option, delete_option, get_option, get_option_value, set_option};
use crate::permalinks::{
    get_permalink_structure, list_posts_by_date_prefix, post_url, resolve_path, resolve_post_by_id,
    set_permalink_structure, term_url, PermalinkStructure, ResolvedPath,
};
use crate::post_types::{
    delete_post_type, find_post_type, get_post_type, list_post_types, post_type_archive_url,
    post_type_single_url, register_post_type, update_post_type, RegisterPostTypeInput,
    UpdatePostTypeInput,
};
use crate::postmeta::{
    delete_post_meta, get_post_meta, get_post_meta_map, set_post_meta, update_post_meta,
};
use crate::posts::{
    create_post, delete_post, get_featured_image_id, get_post_by_id, get_post_by_slug,
    get_post_by_slug_or_id, list_posts, restore_post, row_to_post, slugify, trash_post,
    update_post, CreatePostInput, ListPostsOptions, PostStatus, UpdatePostInput,
};
use crate::revisions::{
    create_revision, diff_lines, diff_revisions, get_revision, list_revisions, restore_revision,
    save_autosave,
};
use crate::search::{escape_like, search_posts, SearchOptions};
use crate::settings::{
    get_default_comment_status, get_posts_per_page, get_settings, update_settings,
};
use crate::taxonomies::{
    delete_taxonomy, get_taxonomy, list_taxonomies, register_taxonomy, update_taxonomy,
    RegisterTaxonomyInput, UpdateTaxonomyInput,
};
use crate::terms::{
    add_post_term, build_term_tree, create_term, delete_term, get_post_terms, get_term,
    list_posts_by_term, list_term_tree, list_terms, remove_post_term, row_to_term, set_post_terms,
    update_term, CreateTermInput, ListTermsOptions, Term, UpdateTermInput,
};
use crate::test_db::TestDb;

fn run<T>(f: impl std::future::Future<Output = T>) -> T {
    block_on(f)
}

fn mk_post(db: &TestDb, title: &str) -> crate::posts::Post {
    run(create_post(
        db,
        1,
        CreatePostInput {
            title: title.to_string(),
            author_id: 1,
            ..Default::default()
        },
    ))
    .unwrap()
}

fn mk_published(db: &TestDb, title: &str) -> crate::posts::Post {
    run(create_post(
        db,
        1,
        CreatePostInput {
            title: title.to_string(),
            author_id: 1,
            status: Some("publish".to_string()),
            ..Default::default()
        },
    ))
    .unwrap()
}

// ---------------------------------------------------------------- posts

#[test]
fn posts_crud_and_slug_uniqueness() {
    let db = TestDb::new();
    let a = mk_post(&db, "Hello World");
    assert_eq!(a.slug, "hello-world");
    assert_eq!(a.status, PostStatus::Draft);
    assert_eq!(a.comment_status, "open"); // default_comment_status default
    let b = mk_post(&db, "Hello World");
    assert_eq!(b.slug, "hello-world-2");
    let c = mk_post(&db, "Hello World");
    assert_eq!(c.slug, "hello-world-3");

    let by_id = run(get_post_by_id(&db, 1, a.id)).unwrap().unwrap();
    assert_eq!(by_id.title, "Hello World");
    let by_slug = run(get_post_by_slug(&db, 1, "hello-world")).unwrap().unwrap();
    assert_eq!(by_slug.id, a.id);
    // Numeric slug-or-id lookup (editor loads /api/posts/<id>).
    let by_num = run(get_post_by_slug_or_id(&db, 1, &a.id.to_string()))
        .unwrap()
        .unwrap();
    assert_eq!(by_num.id, a.id);
    assert!(run(get_post_by_slug(&db, 1, "nope")).unwrap().is_none());

    // List: trashed excluded by default; status + type filters work.
    run(trash_post(&db, 1, b.id)).unwrap();
    let list = run(list_posts(&db, 1, ListPostsOptions::default())).unwrap();
    assert_eq!(list.total, 2);
    assert_eq!(list.page, 1);
    assert_eq!(list.per_page, 20);
    assert_eq!(list.total_pages, 1);
    let trashed = run(list_posts(
        &db, 1,
        ListPostsOptions {
            status: Some("trash".to_string()),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(trashed.total, 1);
    let with_trash = run(list_posts(
        &db, 1,
        ListPostsOptions {
            include_trash: true,
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(with_trash.total, 3);
    let bad = run(list_posts(
        &db, 1,
        ListPostsOptions {
            status: Some("bogus".to_string()),
            ..Default::default()
        },
    ));
    assert!(bad.is_err());
    assert_eq!(bad.unwrap_err().code, "bad_request");
}

#[test]
fn posts_validation_errors() {
    let db = TestDb::new();
    let err = run(create_post(
        &db, 1,
        CreatePostInput {
            title: "   ".to_string(),
            author_id: 1,
            ..Default::default()
        },
    ))
    .unwrap_err();
    assert_eq!(err.code, "bad_request");
    let err = run(create_post(
        &db, 1,
        CreatePostInput {
            title: "x".to_string(),
            status: Some("bogus".to_string()),
            author_id: 1,
            ..Default::default()
        },
    ))
    .unwrap_err();
    assert_eq!(err.code, "bad_request");
    let err = run(create_post(
        &db, 1,
        CreatePostInput {
            title: "x".to_string(),
            post_type: Some("Bad Type!".to_string()),
            author_id: 1,
            ..Default::default()
        },
    ))
    .unwrap_err();
    assert_eq!(err.code, "bad_request");
    let err = run(update_post(&db, 1, 999, UpdatePostInput::default())).unwrap_err();
    assert_eq!(err.code, "not_found");
    assert_eq!(err.status, 404);
}

#[test]
fn posts_update_creates_revision_and_slug_change_redirects() {
    let db = TestDb::new();
    let post = mk_published(&db, "Original Title");
    assert_eq!(run(list_revisions(&db, 1, post.id)).unwrap().len(), 0);

    let updated = run(update_post(
        &db, 1,
        post.id,
        UpdatePostInput {
            title: Some("New Title".to_string()),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(updated.title, "New Title");
    let revs = run(list_revisions(&db, 1, post.id)).unwrap();
    assert_eq!(revs.len(), 1);
    assert_eq!(revs[0].title, "Original Title"); // snapshot of pre-update content
    assert!(!revs[0].is_autosave);

    // No-op update: no new revision.
    run(update_post(&db, 1, post.id, UpdatePostInput::default())).unwrap();
    assert_eq!(run(list_revisions(&db, 1, post.id)).unwrap().len(), 1);

    // Slug change on a published post records a 301.
    let renamed = run(update_post(
        &db, 1,
        post.id,
        UpdatePostInput {
            slug: Some("renamed-title".to_string()),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(renamed.slug, "renamed-title");
    let resolved = run(resolve_path(&db, 1, "/original-title")).unwrap();
    match resolved {
        ResolvedPath::Redirect { target } => assert_eq!(target, "/renamed-title"),
        other => panic!("expected redirect, got {other:?}"),
    }
    // New slug resolves to the post.
    let resolved = run(resolve_path(&db, 1, "/renamed-title")).unwrap();
    assert!(matches!(resolved, ResolvedPath::Post { .. }));
}

#[test]
fn posts_trash_restore_delete() {
    let db = TestDb::new();
    let post = mk_post(&db, "To Trash");
    let trashed = run(trash_post(&db, 1, post.id)).unwrap();
    assert_eq!(trashed.status, PostStatus::Trash);
    // Trashed posts are never returned by slug.
    assert!(run(get_post_by_slug(&db, 1, "to-trash")).unwrap().is_none());
    assert!(run(get_post_by_slug_or_id(&db, 1, "to-trash"))
        .unwrap()
        .is_none());
    // Restore of a non-trashed post is a no-op returning the post.
    let restored = run(restore_post(&db, 1, post.id)).unwrap();
    assert_eq!(restored.status, PostStatus::Draft);
    // Trash again, then delete permanently.
    run(trash_post(&db, 1, post.id)).unwrap();
    assert!(run(delete_post(&db, 1, post.id)).unwrap());
    assert!(run(get_post_by_id(&db, 1, post.id)).unwrap().is_none());
    assert!(run(delete_post(&db, 1, post.id)).is_err());
}

#[test]
fn revisions_autosave_restore_diff() {
    let db = TestDb::new();
    let post = mk_post(&db, "Draft One");
    // Autosave: one per post + author, updated in place.
    let a1 = run(save_autosave(
        &db, 1,
        post.id,
        1,
        Some("Draft One edited"),
        Some("line1\nline2"),
        None,
    ))
    .unwrap();
    assert!(a1.is_autosave);
    assert_eq!(a1.slug, format!("{}-autosave-v1", post.id));
    let a2 = run(save_autosave(
        &db, 1,
        post.id,
        1,
        Some("Draft One edited again"),
        None,
        None,
    ))
    .unwrap();
    assert_eq!(a1.id, a2.id);
    assert_eq!(a2.title, "Draft One edited again");
    // A different author gets their own autosave.
    let a3 = run(save_autosave(&db, 1, post.id, 2, None, None, None)).unwrap();
    assert_ne!(a3.id, a1.id);

    // Update creates a real revision; restore brings the old content back
    // and snapshots the pre-restore content first.
    run(update_post(
        &db, 1,
        post.id,
        UpdatePostInput {
            content: Some("v2 content".to_string()),
            ..Default::default()
        },
    ))
    .unwrap();
    let revs = run(list_revisions(&db, 1, post.id)).unwrap();
    let real: Vec<_> = revs.iter().filter(|r| !r.is_autosave).collect();
    assert_eq!(real.len(), 1);
    let restored = run(restore_revision(&db, 1, real[0].id)).unwrap();
    // The restored content is the revision's content (original body "").
    assert_eq!(restored.content, "");
    // Restore snapshots pre-restore content: now 2 real revisions.
    let revs = run(list_revisions(&db, 1, post.id)).unwrap();
    assert_eq!(revs.iter().filter(|r| !r.is_autosave).count(), 2);

    // Diff between two revisions.
    let r1 = run(create_revision(&db, 1, &restored)).unwrap();
    let mut changed = restored.clone();
    changed.content = "alpha\nbeta".to_string();
    let r2 = run(create_revision(&db, 1, &changed)).unwrap();
    let diff = run(diff_revisions(&db, 1, r1.id, r2.id)).unwrap();
    assert!(diff.exact);
    assert_eq!(diff.added, 2);
    assert_eq!(diff.from_id, r1.id);
    assert_eq!(diff.to_id, r2.id);
    assert!(run(get_revision(&db, 1, r1.id)).is_ok());
    assert!(run(get_revision(&db, 1, 99999)).is_err());
    // Diff across posts is a 400.
    let other = mk_post(&db, "Other Post");
    let ro = run(create_revision(&db, 1, &other)).unwrap();
    assert!(run(diff_revisions(&db, 1, r1.id, ro.id)).is_err());

    // Pure diff helper.
    let d = diff_lines("a\nb\nc", "a\nx\nc\nd");
    assert_eq!((d.added, d.removed), (2, 1));
}

// ---------------------------------------------------------------- terms

#[test]
fn terms_crud_hierarchy_and_counts() {
    let db = TestDb::new();
    let cat = run(create_term(
        &db, 1,
        CreateTermInput {
            name: "News".to_string(),
            taxonomy: "category".to_string(),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(cat.slug, "news");
    assert_eq!(cat.count, 0);
    // Same slug in a different taxonomy is allowed.
    let tag = run(create_term(
        &db, 1,
        CreateTermInput {
            name: "News".to_string(),
            taxonomy: "tag".to_string(),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(tag.slug, "news");
    // Same taxonomy slug collides → suffix.
    let cat2 = run(create_term(
        &db, 1,
        CreateTermInput {
            name: "News".to_string(),
            taxonomy: "category".to_string(),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(cat2.slug, "news-2");
    // Tags are flat.
    let err = run(create_term(
        &db, 1,
        CreateTermInput {
            name: "Sub".to_string(),
            taxonomy: "tag".to_string(),
            parent_id: Some(tag.term_taxonomy_id),
            ..Default::default()
        },
    ))
    .unwrap_err();
    assert_eq!(err.code, "bad_request");
    // Child category.
    let child = run(create_term(
        &db, 1,
        CreateTermInput {
            name: "Local".to_string(),
            taxonomy: "category".to_string(),
            parent_id: Some(cat.term_taxonomy_id),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(child.parent_id, cat.term_taxonomy_id);
    // Tree nests children.
    let tree = run(list_term_tree(&db, 1, "category")).unwrap();
    assert_eq!(tree.len(), 2); // News + News-2 (top level)
    let news = tree.iter().find(|t| t.slug == "news").unwrap();
    assert_eq!(news.children.as_ref().unwrap().len(), 1);
    assert_eq!(news.children.as_ref().unwrap()[0].slug, "local");

    // Update: rename + cycle protection.
    let renamed = run(update_term(
        &db, 1,
        child.term_taxonomy_id,
        UpdateTermInput {
            name: Some("Regional".to_string()),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(renamed.name, "Regional");
    let err = run(update_term(
        &db, 1,
        cat.term_taxonomy_id,
        UpdateTermInput {
            parent_id: Some(child.term_taxonomy_id),
            ..Default::default()
        },
    ))
    .unwrap_err();
    assert_eq!(err.code, "bad_request");
    let err = run(update_term(
        &db, 1,
        cat.term_taxonomy_id,
        UpdateTermInput {
            parent_id: Some(cat.term_taxonomy_id),
            ..Default::default()
        },
    ))
    .unwrap_err();
    assert_eq!(err.code, "bad_request");

    // Assign terms to a post; counts maintained.
    let post = mk_published(&db, "Categorised");
    let assigned = run(set_post_terms(
        &db, 1,
        post.id,
        "category",
        &[cat.term_taxonomy_id, child.term_taxonomy_id],
    ))
    .unwrap();
    assert_eq!(assigned.len(), 2);
    assert_eq!(run(get_term(&db, 1, cat.term_taxonomy_id)).unwrap().count, 1);
    assert_eq!(run(get_term(&db, 1, child.term_taxonomy_id)).unwrap().count, 1);
    // Replace with exactly one term; the other's count drops.
    run(set_post_terms(
        &db, 1,
        post.id,
        "category",
        &[cat.term_taxonomy_id],
    ))
    .unwrap();
    assert_eq!(run(get_term(&db, 1, child.term_taxonomy_id)).unwrap().count, 0);
    // Add / remove single terms.
    run(add_post_term(&db, 1, post.id, tag.term_taxonomy_id)).unwrap();
    assert_eq!(
        run(get_post_terms(&db, 1, post.id, Some("tag")))
            .unwrap()
            .len(),
        1
    );
    run(remove_post_term(&db, 1, post.id, tag.term_taxonomy_id)).unwrap();
    assert_eq!(
        run(get_post_terms(&db, 1, post.id, Some("tag")))
            .unwrap()
            .len(),
        0
    );
    // Wrong-taxonomy assignment is a 400.
    assert!(run(set_post_terms(&db, 1, post.id, "tag", &[cat.term_taxonomy_id])).is_err());
    // Posts by term.
    let by_term = run(list_posts_by_term(
        &db, 1,
        cat.term_taxonomy_id,
        None,
        None,
        None,
    ))
    .unwrap();
    assert_eq!(by_term.total, 1);

    // Delete reparents children to the deleted term's parent.
    run(delete_term(&db, 1, cat.term_taxonomy_id)).unwrap();
    let child_now = run(get_term(&db, 1, child.term_taxonomy_id)).unwrap();
    assert_eq!(child_now.parent_id, 0);
    assert!(run(get_term(&db, 1, cat.term_taxonomy_id)).is_err());

    // List + pagination.
    let list = run(list_terms(&db, 1, "category", ListTermsOptions::default())).unwrap();
    assert_eq!(list.total, 2); // news-2 + regional
}

#[test]
fn term_tree_pure_builder() {
    let mk = |id: i64, parent: i64, name: &str| Term {
        id,
        term_taxonomy_id: id,
        name: name.to_string(),
        slug: slugify(name),
        taxonomy: "category".to_string(),
        description: String::new(),
        parent_id: parent,
        count: 0,
        children: None,
    };
    let flat = vec![mk(1, 0, "a"), mk(2, 1, "b"), mk(3, 1, "c"), mk(4, 0, "d")];
    let tree = build_term_tree(&flat, 0);
    assert_eq!(tree.len(), 2);
    assert_eq!(tree[0].children.as_ref().unwrap().len(), 2);
    assert!(tree[1].children.as_ref().unwrap().is_empty());
}

// ---------------------------------------------------------------- comments

#[test]
fn comments_moderation_rules() {
    let db = TestDb::new();
    let post = mk_published(&db, "Commentable");
    // Anonymous comment is pending.
    let c1 = run(create_comment(
        &db, 1,
        CreateCommentInput {
            post_id: post.id,
            author: "Visitor".to_string(),
            content: "Nice post.".to_string(),
            ..Default::default()
        },
        false,
    ))
    .unwrap();
    assert_eq!(c1.status, CommentStatus::Pending);
    // Moderator comment is approved immediately.
    let c2 = run(create_comment(
        &db, 1,
        CreateCommentInput {
            post_id: post.id,
            author: "Mod".to_string(),
            content: "Welcome.".to_string(),
            ..Default::default()
        },
        true,
    ))
    .unwrap();
    assert_eq!(c2.status, CommentStatus::Approved);
    // comment_count counts approved only.
    assert_eq!(
        run(get_post_by_id(&db, 1, post.id))
            .unwrap()
            .unwrap()
            .comment_count,
        1
    );
    // Honeypot → spam even for a moderator.
    let c3 = run(create_comment(
        &db, 1,
        CreateCommentInput {
            post_id: post.id,
            author: "Bot".to_string(),
            content: "Buy now.".to_string(),
            honeypot: Some("http://spam.example".to_string()),
            ..Default::default()
        },
        true,
    ))
    .unwrap();
    assert_eq!(c3.status, CommentStatus::Spam);
    // Too many links (default max 2) → pending even for a moderator.
    let c4 = run(create_comment(
        &db, 1,
        CreateCommentInput {
            post_id: post.id,
            author: "Linky".to_string(),
            content: "see https://a.co https://b.co https://c.co".to_string(),
            ..Default::default()
        },
        true,
    ))
    .unwrap();
    assert_eq!(c4.status, CommentStatus::Pending);
    // Approve → count syncs.
    run(set_comment_status(&db, 1, c1.id, "approved")).unwrap();
    assert_eq!(
        run(get_post_by_id(&db, 1, post.id))
            .unwrap()
            .unwrap()
            .comment_count,
        2
    );
    // List filters.
    let pending = run(list_comments(
        &db, 1,
        ListCommentsOptions {
            status: Some("pending".to_string()),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(pending.total, 1);
    let all = run(list_comments(&db, 1, ListCommentsOptions::default())).unwrap();
    assert_eq!(all.total, 4);
    assert!(run(list_comments(
        &db, 1,
        ListCommentsOptions {
            status: Some("bogus".to_string()),
            ..Default::default()
        }
    ))
    .is_err());
    // Threading: reply nests under its parent; approved-only.
    let reply = run(create_comment(
        &db, 1,
        CreateCommentInput {
            post_id: post.id,
            author: "Replier".to_string(),
            content: "Agreed.".to_string(),
            parent_id: Some(c2.id),
            ..Default::default()
        },
        true,
    ))
    .unwrap();
    let threads = run(list_approved_threaded(&db, 1, post.id)).unwrap();
    assert_eq!(threads.len(), 2); // c2 + c1 (approved); c3 spam, c4 pending excluded
    let parent = threads.iter().find(|c| c.id == c2.id).unwrap();
    assert_eq!(parent.children.as_ref().unwrap().len(), 1);
    assert_eq!(parent.children.as_ref().unwrap()[0].id, reply.id);
    // Delete reparents replies to top level, then the reply is a root.
    run(delete_comment(&db, 1, c2.id)).unwrap();
    assert_eq!(run(get_comment(&db, 1, reply.id)).unwrap().parent_id, 0);
    // Approved comments left: c1 + the reply.
    assert_eq!(
        run(get_post_by_id(&db, 1, post.id))
            .unwrap()
            .unwrap()
            .comment_count,
        2
    );
    // Closed comments.
    let closed = run(create_post(
        &db, 1,
        CreatePostInput {
            title: "Closed".to_string(),
            author_id: 1,
            status: Some("publish".to_string()),
            ..Default::default()
        },
    ))
    .unwrap();
    run(crate::posts::update_post(
        &db, 1,
        closed.id,
        UpdatePostInput {
            status: Some("publish".to_string()),
            ..Default::default()
        },
    ))
    .unwrap();
    // Flip comment_status directly (no service for it) then verify the guard.
    run(crate::posts::update_post_row(
        &db, 1,
        closed.id,
        &["comment_status = ?".to_string()],
        &[wordless_db::Param::Text("closed".to_string())],
    ))
    .unwrap();
    let err = run(create_comment(
        &db, 1,
        CreateCommentInput {
            post_id: closed.id,
            author: "X".to_string(),
            content: "Hi".to_string(),
            ..Default::default()
        },
        false,
    ))
    .unwrap_err();
    assert_eq!(err.code, "bad_request");
    // Reply to a comment on another post is a 400.
    let other = mk_published(&db, "Other Post");
    let err = run(create_comment(
        &db, 1,
        CreateCommentInput {
            post_id: other.id,
            author: "X".to_string(),
            content: "Hi".to_string(),
            parent_id: Some(reply.id),
            ..Default::default()
        },
        false,
    ))
    .unwrap_err();
    assert_eq!(err.code, "bad_request");
}

#[test]
fn comment_threading_pure() {
    let mk = |id: i64, parent: i64| Comment {
        id,
        post_id: 1,
        author: format!("u{id}"),
        author_email: String::new(),
        author_url: String::new(),
        author_ip: String::new(),
        date: String::new(),
        date_gmt: String::new(),
        content: String::new(),
        status: CommentStatus::Approved,
        parent_id: parent,
        user_id: 0,
        children: None,
    };
    // Three levels deep + an orphan (missing parent → root).
    let flat = vec![mk(1, 0), mk(2, 1), mk(3, 2), mk(4, 99)];
    let roots = thread_comments(flat);
    assert_eq!(roots.len(), 2);
    let l1 = roots.iter().find(|c| c.id == 1).unwrap();
    let l2 = &l1.children.as_ref().unwrap()[0];
    assert_eq!(l2.id, 2);
    assert_eq!(l2.children.as_ref().unwrap()[0].id, 3);
    assert_eq!(count_links("a https://x.co b"), 1);
}

// ---------------------------------------------------------------- menus

#[test]
fn menus_crud_locations_and_nesting() {
    let db = TestDb::new();
    let menu = run(create_menu(&db, 1, "Primary")).unwrap();
    assert_eq!(menu.name, "Primary");
    let post = mk_published(&db, "Menu Target");
    let cat = run(create_term(
        &db, 1,
        CreateTermInput {
            name: "Menu Cat".to_string(),
            taxonomy: "category".to_string(),
            ..Default::default()
        },
    ))
    .unwrap();
    // Custom item.
    let custom = run(add_menu_item(
        &db, 1,
        menu.id,
        CreateMenuItemInput {
            title: "Home".to_string(),
            item_type: "custom".to_string(),
            url: Some("/".to_string()),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(custom.url, "/");
    assert_eq!(custom.item_type, crate::menus::MenuItemType::Custom);
    // Post item resolves its URL through permalinks.
    let post_item = run(add_menu_item(
        &db, 1,
        menu.id,
        CreateMenuItemInput {
            title: "Target".to_string(),
            item_type: "post".to_string(),
            object_id: Some(post.id),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(post_item.url, "/menu-target");
    assert_eq!(post_item.object_id, post.id);
    // Term item.
    let term_item = run(add_menu_item(
        &db, 1,
        menu.id,
        CreateMenuItemInput {
            title: "Cat".to_string(),
            item_type: "term".to_string(),
            object_id: Some(cat.term_taxonomy_id),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(term_item.url, "/category/menu-cat");
    // Child item nests under its parent.
    let child = run(add_menu_item(
        &db, 1,
        menu.id,
        CreateMenuItemInput {
            title: "Sub".to_string(),
            item_type: "custom".to_string(),
            url: Some("/sub".to_string()),
            parent_id: Some(custom.id),
            ..Default::default()
        },
    ))
    .unwrap();
    let items = run(list_menu_items(&db, 1, menu.id)).unwrap();
    assert_eq!(items.len(), 3);
    let home = items.iter().find(|i| i.id == custom.id).unwrap();
    assert_eq!(home.children.as_ref().unwrap().len(), 1);
    assert_eq!(home.children.as_ref().unwrap()[0].id, child.id);
    // Update + validation.
    let updated = run(update_menu_item(
        &db, 1,
        menu.id,
        child.id,
        UpdateMenuItemInput {
            title: Some("Sub 2".to_string()),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(updated.title, "Sub 2");
    assert!(run(add_menu_item(
        &db, 1,
        menu.id,
        CreateMenuItemInput {
            title: "Bad".to_string(),
            item_type: "custom".to_string(),
            ..Default::default()
        },
    ))
    .is_err()); // custom item without url
    assert!(run(add_menu_item(
        &db, 1,
        menu.id,
        CreateMenuItemInput {
            title: "Bad".to_string(),
            item_type: "post".to_string(),
            object_id: Some(99999),
            ..Default::default()
        },
    ))
    .is_err()); // missing post
                // Locations.
    let locs = run(set_menu_location(&db, 1, "primary", Some(menu.id))).unwrap();
    assert_eq!(locs.get("primary"), Some(&menu.id));
    let resolved = run(resolve_menu_by_location(&db, 1, "primary")).unwrap();
    assert_eq!(resolved.menu.as_ref().unwrap().id, menu.id);
    assert_eq!(resolved.items.len(), 3);
    let empty = run(resolve_menu_by_location(&db, 1, "footer")).unwrap();
    assert!(empty.menu.is_none());
    assert!(empty.items.is_empty());
    assert_eq!(run(get_menu_locations(&db, 1)).unwrap().len(), 1);
    // Remove item; children become top-level.
    run(remove_menu_item(&db, 1, menu.id, custom.id)).unwrap();
    let items = run(list_menu_items(&db, 1, menu.id)).unwrap();
    assert_eq!(items.len(), 3); // post, term, sub (now top-level)
                                // Delete menu removes items + location.
    assert_eq!(run(list_menus(&db, 1)).unwrap().len(), 1);
    run(delete_menu(&db, 1, menu.id)).unwrap();
    assert!(run(list_menus(&db, 1)).unwrap().is_empty());
    assert!(run(get_menu_locations(&db, 1)).unwrap().is_empty());
}

// ---------------------------------------------------------------- search

#[test]
fn search_like_based_title_first() {
    let db = TestDb::new();
    mk_published(&db, "Rust Guide");
    let body_post = run(create_post(
        &db, 1,
        CreatePostInput {
            title: "Something Else".to_string(),
            content: Some("This body mentions Rust inside.".to_string()),
            status: Some("publish".to_string()),
            author_id: 1,
            ..Default::default()
        },
    ))
    .unwrap();
    mk_post(&db, "Rust Draft"); // draft: not searchable
    let res = run(search_posts(&db, 1, "Rust", SearchOptions::default())).unwrap();
    assert_eq!(res.total, 2);
    assert_eq!(res.query, "Rust");
    // Title match ranks first.
    assert_eq!(res.posts[0].title, "Rust Guide");
    assert_eq!(res.posts[1].id, body_post.id);
    // Type filter.
    let res = run(search_posts(
        &db, 1,
        "Rust",
        SearchOptions {
            post_type: Some("page".to_string()),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(res.total, 0);
    // LIKE wildcards are escaped: "%" matches literally, not everything.
    let res = run(search_posts(&db, 1, "%", SearchOptions::default())).unwrap();
    assert_eq!(res.total, 0);
    assert!(run(search_posts(&db, 1, "", SearchOptions::default())).is_err());
    assert!(run(search_posts(
        &db, 1,
        &"x".repeat(201),
        SearchOptions::default()
    ))
    .is_err());
    assert_eq!(escape_like("100%_\\"), "100\\%\\_\\\\");
}

// ---------------------------------------------------------------- permalinks

#[test]
fn permalinks_structures_and_resolution() {
    let db = TestDb::new();
    assert_eq!(
        run(get_permalink_structure(&db, 1)).unwrap(),
        PermalinkStructure::Postname
    );
    let post = mk_published(&db, "Dated Post");
    // postname (default).
    assert_eq!(run(post_url(&db, 1, &post)).unwrap(), "/dated-post");
    // day-name / month-name / plain.
    run(set_permalink_structure(&db, 1, "day-name")).unwrap();
    let (year, month, day) = (&post.date[0..4], &post.date[5..7], &post.date[8..10]);
    assert_eq!(
        run(post_url(&db, 1, &post)).unwrap(),
        format!("/{year}/{month}/{day}/dated-post")
    );
    run(set_permalink_structure(&db, 1, "month-name")).unwrap();
    assert_eq!(
        run(post_url(&db, 1, &post)).unwrap(),
        format!("/{year}/{month}/dated-post")
    );
    run(set_permalink_structure(&db, 1, "plain")).unwrap();
    assert_eq!(
        run(post_url(&db, 1, &post)).unwrap(),
        format!("/?p={}", post.id)
    );
    assert!(run(set_permalink_structure(&db, 1, "bogus")).is_err());
    // v1 turns the plain-Error into 500 internal_error.
    assert_eq!(
        run(set_permalink_structure(&db, 1, "bogus"))
            .unwrap_err()
            .status,
        500
    );
    run(set_permalink_structure(&db, 1, "postname")).unwrap();

    // term_url.
    assert_eq!(term_url("category", "news"), "/category/news");
    assert_eq!(term_url("tag", "rust"), "/tag/rust");
    assert_eq!(term_url("genre", "scifi"), "/genre/scifi");

    // Resolution: home, term archive, date archive, dated post, plain slug.
    assert!(matches!(
        run(resolve_path(&db, 1, "/")).unwrap(),
        ResolvedPath::Home
    ));
    let cat = run(create_term(
        &db, 1,
        CreateTermInput {
            name: "News".to_string(),
            taxonomy: "category".to_string(),
            ..Default::default()
        },
    ))
    .unwrap();
    assert!(
        matches!(run(resolve_path(&db, 1, "/category/news")).unwrap(), ResolvedPath::Term { term } if term.term_taxonomy_id == cat.term_taxonomy_id)
    );
    assert!(matches!(
        run(resolve_path(&db, 1, "/category/nope")).unwrap(),
        ResolvedPath::NotFound
    ));
    assert!(matches!(
        run(resolve_path(&db, 1, "/2026/11")).unwrap(),
        ResolvedPath::DateArchive {
            year: 2026,
            month: Some(11)
        }
    ));
    assert!(matches!(
        run(resolve_path(&db, 1, "/2026")).unwrap(),
        ResolvedPath::DateArchive {
            year: 2026,
            month: None
        }
    ));
    assert!(matches!(
        run(resolve_path(&db, 1, "/2026/13")).unwrap(),
        ResolvedPath::NotFound
    ));
    let dated = run(resolve_path(
        &db, 1,
        &format!("/{year}/{month}/{day}/dated-post"),
    ))
    .unwrap();
    assert!(matches!(dated, ResolvedPath::Post { .. }));
    assert!(matches!(
        run(resolve_path(&db, 1, "/2020/01/01/dated-post")).unwrap(),
        ResolvedPath::NotFound
    )); // wrong date
    assert!(matches!(
        run(resolve_path(&db, 1, "/dated-post")).unwrap(),
        ResolvedPath::Post { .. }
    ));
    assert!(matches!(
        run(resolve_path(&db, 1, "/missing-slug")).unwrap(),
        ResolvedPath::NotFound
    ));
    // resolve by id + date-prefix listing.
    assert!(run(resolve_post_by_id(&db, 1, post.id)).unwrap().is_some());
    assert!(run(resolve_post_by_id(&db, 1, 99999)).unwrap().is_none());
    let by_date = run(list_posts_by_date_prefix(
        &db, 1,
        &format!("{year}-{month}"),
        20,
        0,
    ))
    .unwrap();
    assert_eq!(by_date.len(), 1);
}

// ---------------------------------------------------------------- options/settings/activity

#[test]
fn options_crud() {
    let db = TestDb::new();
    assert_eq!(run(get_option_value(&db, "missing")).unwrap(), json!(null));
    run(set_option(&db, "site_name", &"Wordless".to_string())).unwrap();
    assert_eq!(
        run(get_option::<String>(&db, "site_name", "dflt".to_string())).unwrap(),
        "Wordless"
    );
    // JSON values round-trip.
    run(set_option(&db, "num", &42i64)).unwrap();
    assert_eq!(run(get_option::<i64>(&db, "num", 0)).unwrap(), 42);
    assert!(!run(add_option(&db, "site_name", &"Other".to_string())).unwrap());
    assert!(run(add_option(&db, "fresh", &true)).unwrap());
    assert!(run(delete_option(&db, "fresh")).unwrap());
    assert!(!run(delete_option(&db, "fresh")).unwrap());
    // Key validation (max 191).
    assert!(run(set_option(&db, &"k".repeat(192), &1i64)).is_err());
}

#[test]
fn settings_defaults_and_updates() {
    let db = TestDb::new();
    let s = run(get_settings(&db, 1)).unwrap();
    assert_eq!(s.site_title, "Wordless");
    assert_eq!(s.posts_per_page, 10);
    assert_eq!(s.permalink_structure, PermalinkStructure::Postname);
    assert_eq!(s.timezone, "UTC");
    assert_eq!(s.default_comment_status, "open");
    assert_eq!(s.comment_max_links, 2);
    let updated = run(update_settings(
        &db, 1,
        &json!({ "siteTitle": "My Site", "postsPerPage": 5, "timezone": "Africa/Lagos", "commentMaxLinks": 0 }),
    ))
    .unwrap();
    assert_eq!(updated.site_title, "My Site");
    assert_eq!(updated.posts_per_page, 5);
    assert_eq!(updated.timezone, "Africa/Lagos");
    assert_eq!(updated.comment_max_links, 0);
    assert_eq!(run(get_posts_per_page(&db, 1)).unwrap(), 5);
    // Invalid patch is rejected wholesale.
    assert!(run(update_settings(&db, 1, &json!({ "postsPerPage": 500 }))).is_err());
    assert_eq!(run(get_posts_per_page(&db, 1)).unwrap(), 5);
    // Default comment status flows into new posts.
    run(update_settings(
        &db, 1,
        &json!({ "defaultCommentStatus": "closed" }),
    ))
    .unwrap();
    assert_eq!(run(get_default_comment_status(&db, 1)).unwrap(), "closed");
    let closed_post = mk_post(&db, "Closed Comments");
    assert_eq!(closed_post.comment_status, "closed");
}

#[test]
fn activity_log_and_list() {
    let db = TestDb::new();
    let entry = run(log_activity(
        &db,
        LogActivityInput {
            actor_id: 7,
            action: "post.create".to_string(),
            object_type: Some("post".to_string()),
            object_id: Some("42".to_string()),
            details: None,
            ip_hash: None,
        },
    ))
    .unwrap();
    assert_eq!(entry.actor_id, 7);
    assert_eq!(entry.action, "post.create");
    assert_eq!(entry.object_type.as_deref(), Some("post"));
    assert_eq!(entry.object_id.as_deref(), Some("42"));
    // Best-effort log() helper.
    let entry2 = run(log(&db, 0, "term.create", "term", 3));
    assert!(entry2.is_some());
    let list = run(list_activity(&db, ListActivityOptions::default())).unwrap();
    assert_eq!(list.total, 2);
    assert_eq!(list.entries[0].action, "term.create"); // newest first
    let page2 = run(list_activity(
        &db,
        ListActivityOptions {
            page: Some(2),
            per_page: Some(1),
        },
    ))
    .unwrap();
    assert_eq!(page2.entries.len(), 1);
    assert_eq!(page2.total_pages, 2);
}

// ---------------------------------------------------------------- postmeta

#[test]
fn postmeta_helpers() {
    let db = TestDb::new();
    let post = mk_post(&db, "Meta Post");
    assert!(run(get_post_meta(&db, 1, post.id, "_thumbnail_id"))
        .unwrap()
        .is_none());
    run(set_post_meta(&db, 1, post.id, "_thumbnail_id", "12")).unwrap();
    assert_eq!(
        run(get_post_meta(&db, 1, post.id, "_thumbnail_id"))
            .unwrap()
            .as_deref(),
        Some("12")
    );
    // Upsert: second set updates, does not duplicate.
    run(set_post_meta(&db, 1, post.id, "_thumbnail_id", "13")).unwrap();
    let map = run(get_post_meta_map(&db, 1, post.id)).unwrap();
    assert_eq!(map.len(), 1);
    assert_eq!(map.get("_thumbnail_id").map(|s| s.as_str()), Some("13"));
    assert!(run(delete_post_meta(&db, 1, post.id, "_thumbnail_id")).unwrap());
    assert!(!run(delete_post_meta(&db, 1, post.id, "_thumbnail_id")).unwrap());
}

// ---------------------------------------------------------------- featured image (P4)

fn mk_attachment(db: &TestDb, title: &str) -> crate::posts::Post {
    run(create_post(
        db,
        1,
        CreatePostInput {
            title: title.to_string(),
            post_type: Some("attachment".to_string()),
            mime_type: Some("image/png".to_string()),
            author_id: 1,
            ..Default::default()
        },
    ))
    .unwrap()
}

#[test]
fn featured_image_round_trip_and_validation() {
    let db = TestDb::new();
    let media = mk_attachment(&db, "pic.png");
    // Create with a featured image: `_thumbnail_id` is written.
    let post = run(create_post(
        &db, 1,
        CreatePostInput {
            title: "With image".to_string(),
            author_id: 1,
            featured_image_id: Some(media.id),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(
        run(get_featured_image_id(&db, 1, post.id)).unwrap(),
        Some(media.id)
    );
    // Clear with null (Some(None)): the meta row is deleted.
    run(update_post(
        &db, 1,
        post.id,
        UpdatePostInput {
            featured_image_id: Some(None),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(run(get_featured_image_id(&db, 1, post.id)).unwrap(), None);
    // Set again via update — with no other field changed, so no
    // revision is snapshotted and no row update runs.
    run(update_post(
        &db, 1,
        post.id,
        UpdatePostInput {
            featured_image_id: Some(Some(media.id)),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(
        run(get_featured_image_id(&db, 1, post.id)).unwrap(),
        Some(media.id)
    );
    // Unknown attachment id: 400, and no post is created.
    let before = run(list_posts(&db, 1, ListPostsOptions::default()))
        .unwrap()
        .total;
    let err = run(create_post(
        &db, 1,
        CreatePostInput {
            title: "Bad image".to_string(),
            author_id: 1,
            featured_image_id: Some(99999),
            ..Default::default()
        },
    ))
    .unwrap_err();
    assert_eq!(err.status, 400);
    assert_eq!(
        run(list_posts(&db, 1, ListPostsOptions::default()))
            .unwrap()
            .total,
        before,
        "a bad featuredImageId must not leave an orphaned post"
    );
    // A non-attachment post id is also a 400.
    let other = mk_post(&db, "Plain post");
    let err = run(update_post(
        &db, 1,
        post.id,
        UpdatePostInput {
            featured_image_id: Some(Some(other.id)),
            ..Default::default()
        },
    ))
    .unwrap_err();
    assert_eq!(err.status, 400);
    // Zero / negative ids are 400.
    let err = run(update_post(
        &db, 1,
        post.id,
        UpdatePostInput {
            featured_image_id: Some(Some(0)),
            ..Default::default()
        },
    ))
    .unwrap_err();
    assert_eq!(err.status, 400);
}

#[test]
fn post_meta_patch_sets_deletes_and_blocks_schedule_key() {
    let db = TestDb::new();
    let post = mk_post(&db, "Meta Patch");
    let media = mk_attachment(&db, "thumb.png");
    // Set a custom key and the thumbnail in one patch.
    let map = run(update_post_meta(
        &db, 1,
        post.id,
        &[
            ("custom_key".to_string(), Some("v".to_string())),
            ("_thumbnail_id".to_string(), Some(media.id.to_string())),
        ],
    ))
    .unwrap();
    assert_eq!(map.get("custom_key").map(|s| s.as_str()), Some("v"));
    assert_eq!(
        map.get("_thumbnail_id").map(|s| s.as_str()),
        Some(media.id.to_string().as_str())
    );
    assert_eq!(
        run(get_featured_image_id(&db, 1, post.id)).unwrap(),
        Some(media.id)
    );
    // Null deletes the key.
    let map = run(update_post_meta(
        &db, 1,
        post.id,
        &[("custom_key".to_string(), None)],
    ))
    .unwrap();
    assert!(!map.contains_key("custom_key"));
    assert!(map.contains_key("_thumbnail_id"));
    // `_wl_publish_at` is blocked: schedules go through `publishAt`.
    let err = run(update_post_meta(
        &db, 1,
        post.id,
        &[(
            "_wl_publish_at".to_string(),
            Some("2026-12-01T09:00:00.000Z".to_string()),
        )],
    ))
    .unwrap_err();
    assert_eq!(err.status, 400);
    // A thumbnail that is not an attachment is a 400.
    let err = run(update_post_meta(
        &db, 1,
        post.id,
        &[("_thumbnail_id".to_string(), Some("99999".to_string()))],
    ))
    .unwrap_err();
    assert_eq!(err.status, 400);
    let err = run(update_post_meta(
        &db, 1,
        post.id,
        &[(
            "_thumbnail_id".to_string(),
            Some("not-a-number".to_string()),
        )],
    ))
    .unwrap_err();
    assert_eq!(err.status, 400);
    // Unknown post is a 404; empty key is a 400.
    let err = run(update_post_meta(
        &db, 1,
        99999,
        &[("k".to_string(), Some("v".to_string()))],
    ))
    .unwrap_err();
    assert_eq!(err.status, 404);
    let err = run(update_post_meta(
        &db, 1,
        post.id,
        &[("".to_string(), Some("v".to_string()))],
    ))
    .unwrap_err();
    assert_eq!(err.status, 400);
}

// ---------------------------------------------------------------- post types

#[test]
fn post_types_register_list_update_delete() {
    let db = TestDb::new();
    // Built-ins are listed first.
    let initial = run(list_post_types(&db, 1)).unwrap();
    assert_eq!(initial.len(), 2);
    assert!(initial.iter().any(|t| t.slug == "post" && t.builtin));
    assert!(initial.iter().any(|t| t.slug == "page" && t.builtin));
    assert!(run(get_post_type(&db, 1, "post")).unwrap().builtin);

    let book = run(register_post_type(
        &db, 1,
        RegisterPostTypeInput {
            slug: "book".to_string(),
            label: "Books".to_string(),
            singular_label: Some("Book".to_string()),
            description: Some("Long-form works.".to_string()),
            supports: Some(vec!["title".to_string(), "editor".to_string()]),
            rewrite_slug: Some("books".to_string()),
            taxonomies: Some(vec!["genre".to_string()]),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(book.rewrite_slug, "books");
    assert_eq!(book.supports, vec!["title", "editor"]);
    // Duplicate → 409.
    let err = run(register_post_type(
        &db, 1,
        RegisterPostTypeInput {
            slug: "book".to_string(),
            label: "Books".to_string(),
            ..Default::default()
        },
    ))
    .unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(err.code, "post_type_exists");
    // Reserved slug → 400.
    assert!(run(register_post_type(
        &db, 1,
        RegisterPostTypeInput {
            slug: "revision".to_string(),
            label: "X".to_string(),
            ..Default::default()
        },
    ))
    .is_err());
    // Update (slug cannot change; built-ins cannot update).
    let updated = run(update_post_type(
        &db, 1,
        "book",
        UpdatePostTypeInput {
            label: Some("Novels".to_string()),
            has_archive: Some(false),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(updated.label, "Novels");
    assert!(!updated.has_archive);
    assert!(run(update_post_type(
        &db, 1,
        "post",
        UpdatePostTypeInput {
            label: Some("X".to_string()),
            ..Default::default()
        }
    ))
    .is_err());
    // Rewrite helpers.
    let post = run(create_post(
        &db, 1,
        CreatePostInput {
            title: "Dune".to_string(),
            status: Some("publish".to_string()),
            post_type: Some("book".to_string()),
            author_id: 1,
            ..Default::default()
        },
    ))
    .unwrap();
    let def = run(find_post_type(&db, 1, "book")).unwrap().unwrap();
    assert_eq!(post_type_single_url(&def, &post), "/books/dune");
    assert!(post_type_archive_url(&def).is_none()); // has_archive now false
                                                    // Posts of a custom type list/get through the posts service.
    let listed = run(list_posts(
        &db, 1,
        ListPostsOptions {
            post_type: Some("book".to_string()),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(listed.total, 1);
    // Slug uniqueness is scoped to the type: same title in another type is fine,
    // but a second book with the same title gets a suffix.
    let book2 = run(create_post(
        &db, 1,
        CreatePostInput {
            title: "Dune".to_string(),
            post_type: Some("book".to_string()),
            author_id: 1,
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(book2.slug, "dune-2");
    // Resolve the CPT single + archive paths.
    run(update_post_type(
        &db, 1,
        "book",
        UpdatePostTypeInput {
            has_archive: Some(true),
            ..Default::default()
        },
    ))
    .unwrap();
    assert!(matches!(
        run(resolve_path(&db, 1, "/books/dune")).unwrap(),
        ResolvedPath::Post { .. }
    ));
    assert!(
        matches!(run(resolve_path(&db, 1, "/books")).unwrap(), ResolvedPath::PostTypeArchive { post_type } if post_type == "book")
    );
    assert!(matches!(
        run(resolve_path(&db, 1, "/books/nope")).unwrap(),
        ResolvedPath::NotFound
    ));
    // post_url for a CPT post uses the rewrite rule.
    assert_eq!(run(post_url(&db, 1, &post)).unwrap(), "/books/dune");
    // Delete blocked while posts exist; allowed once they are gone.
    let err = run(delete_post_type(&db, 1, "book")).unwrap_err();
    assert_eq!(err.code, "post_type_in_use");
    run(delete_post(&db, 1, post.id)).unwrap();
    run(delete_post(&db, 1, book2.id)).unwrap();
    assert!(run(delete_post_type(&db, 1, "book")).unwrap());
    assert!(run(get_post_type(&db, 1, "book")).is_err());
    assert!(run(delete_post_type(&db, 1, "post")).is_err());
}

#[test]
fn post_type_json_shape() {
    // The stored JSON shape (option wl_post_types) round-trips exactly.
    let raw = json!({
        "book": {
            "slug": "book", "label": "Books", "singularLabel": "Book",
            "description": "", "public": true, "hasArchive": true,
            "hierarchical": false, "supports": ["title"],
            "rewriteSlug": "books", "taxonomies": [], "builtin": false
        }
    });
    let map: std::collections::BTreeMap<String, crate::post_types::PostType> =
        serde_json::from_value(raw).unwrap();
    let def = map.get("book").unwrap();
    assert_eq!(def.singular_label, "Book");
    let back = serde_json::to_value(def).unwrap();
    assert_eq!(back["singularLabel"], "Book");
    assert_eq!(back["rewriteSlug"], "books");
}

// ---------------------------------------------------------------- taxonomies

#[test]
fn taxonomies_register_list_update_delete_and_terms() {
    let db = TestDb::new();
    let initial = run(list_taxonomies(&db, 1)).unwrap();
    assert_eq!(initial.len(), 3); // category, tag, nav_menu
    assert!(initial
        .iter()
        .any(|t| t.slug == "category" && t.hierarchical && t.builtin));

    let genre = run(register_taxonomy(
        &db, 1,
        RegisterTaxonomyInput {
            slug: "genre".to_string(),
            label: "Genres".to_string(),
            hierarchical: Some(true),
            object_types: Some(vec!["book".to_string()]),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(genre.rewrite_slug, "genre");
    assert!(genre.hierarchical);
    // Duplicate → 409; reserved → 400.
    let err = run(register_taxonomy(
        &db, 1,
        RegisterTaxonomyInput {
            slug: "genre".to_string(),
            label: "G".to_string(),
            ..Default::default()
        },
    ))
    .unwrap_err();
    assert_eq!(err.code, "taxonomy_exists");
    assert!(run(register_taxonomy(
        &db, 1,
        RegisterTaxonomyInput {
            slug: "category".to_string(),
            label: "C".to_string(),
            ..Default::default()
        },
    ))
    .is_err());
    // Term assignment works for the custom taxonomy through the terms service.
    let scifi = run(create_term(
        &db, 1,
        CreateTermInput {
            name: "Sci-Fi".to_string(),
            taxonomy: "genre".to_string(),
            ..Default::default()
        },
    ))
    .unwrap();
    let book = run(create_post(
        &db, 1,
        CreatePostInput {
            title: "Dune".to_string(),
            post_type: Some("book".to_string()),
            author_id: 1,
            ..Default::default()
        },
    ))
    .unwrap();
    run(set_post_terms(
        &db, 1,
        book.id,
        "genre",
        &[scifi.term_taxonomy_id],
    ))
    .unwrap();
    assert_eq!(
        run(get_post_terms(&db, 1, book.id, Some("genre")))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(run(get_term(&db, 1, scifi.term_taxonomy_id)).unwrap().count, 1);
    // Custom-taxonomy archive resolves.
    assert!(matches!(
        run(resolve_path(&db, 1, "/genre/sci-fi")).unwrap(),
        ResolvedPath::Term { .. }
    ));
    assert!(matches!(
        run(resolve_path(&db, 1, "/genre/nope")).unwrap(),
        ResolvedPath::NotFound
    ));
    // Update + delete guards.
    let updated = run(update_taxonomy(
        &db, 1,
        "genre",
        UpdateTaxonomyInput {
            label: Some("Book Genres".to_string()),
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(updated.label, "Book Genres");
    let err = run(delete_taxonomy(&db, 1, "genre")).unwrap_err();
    assert_eq!(err.code, "taxonomy_in_use");
    run(delete_term(&db, 1, scifi.term_taxonomy_id)).unwrap();
    assert!(run(delete_taxonomy(&db, 1, "genre")).unwrap());
    assert!(run(get_taxonomy(&db, 1, "genre")).is_err());
    assert!(run(delete_taxonomy(&db, 1, "category")).is_err());
}

// ---------------------------------------------------------------- row mapping

#[test]
fn row_mapping_handles_d1_shaped_numbers() {
    // D1 numbers arrive as JSON numbers (f64); the mappers must read them.
    let row = json!({
        "ID": 7.0, "post_author": 3.0, "post_date": "2026-11-19T00:00:00.000Z",
        "post_date_gmt": "2026-11-19T00:00:00.000Z", "post_content": "B",
        "post_title": "T", "post_excerpt": "", "post_status": "weird-status",
        "comment_status": "open", "ping_status": "open", "post_password": "",
        "post_name": "t", "post_modified": "", "post_modified_gmt": "",
        "post_content_filtered": "", "post_parent": 0.0, "guid": "",
        "menu_order": 0.0, "post_type": "post", "post_mime_type": "",
        "comment_count": 5.0
    });
    let post = row_to_post(&row);
    assert_eq!(post.id, 7);
    assert_eq!(post.comment_count, 5);
    assert_eq!(post.status, PostStatus::Draft); // unknown status → draft, as v1

    let trow = json!({
        "term_id": 2.0, "term_taxonomy_id": 9.0, "name": "N", "slug": "n",
        "taxonomy": "category", "description": "", "parent": 0.0, "count": 3.0
    });
    let term = row_to_term(&trow);
    assert_eq!(term.term_taxonomy_id, 9);
    assert_eq!(term.count, 3);
}
