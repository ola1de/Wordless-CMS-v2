//! Executor tests: the P3 done-when is that a GraphQL query returns
//! the same post a REST call returns, under the same capability guard.

use futures_executor::block_on;
use serde_json::{json, Value};
use wordless_kernel::capabilities::{AuthUser, Role};

use crate::executor::execute;
use crate::test_support::SqliteDb;

fn admin() -> AuthUser {
    AuthUser {
        id: 1,
        login: "admin".to_string(),
        email: "admin@example.test".to_string(),
        display_name: "Admin User".to_string(),
        role: Role::Administrator,
    }
}

fn seed() -> SqliteDb {
    let db = SqliteDb::new();
    block_on(async {
        db.insert_post(
            "Hello World",
            "First post content.",
            "First excerpt.",
            "publish",
            "hello-world",
            "post",
            "2026-11-19T10:00:00",
            1,
        )
        .await;
        db.insert_post(
            "Draft Post",
            "Draft content.",
            "",
            "draft",
            "draft-post",
            "post",
            "2026-11-19T11:00:00",
            1,
        )
        .await;
        db.set_option_raw("blogname", "\"Wordless Test\"").await;
        db.set_option_raw("siteurl", "\"https://example.test\"")
            .await;
        db.exec_batch(
            "INSERT INTO wp_users (ID, user_login, user_nicename, user_email, display_name) VALUES (1, 'admin', 'admin', 'admin@example.test', 'Admin User');
             INSERT INTO wp_usermeta (user_id, meta_key, meta_value) VALUES (1, 'wp_capabilities', '{\"administrator\": true}');
             INSERT INTO wp_terms (term_id, name, slug) VALUES (1, 'News', 'news');
             INSERT INTO wp_term_taxonomy (term_taxonomy_id, term_id, taxonomy, description, parent, count) VALUES (1, 1, 'category', '', 0, 1);
             INSERT INTO wp_term_relationships (object_id, term_taxonomy_id, term_order) VALUES (1, 1, 0);",
        );
    });
    db
}

#[test]
fn posts_list_returns_published_and_draft_like_rest() {
    let db = seed();
    let out = block_on(execute(
        &db, 1,
        None,
        "{ posts { total posts { id title status } } }",
        None,
        None,
    ));
    assert!(out.errors.is_empty(), "errors: {:?}", out.errors);
    assert_eq!(out.data["posts"]["total"], json!(2));
    let titles: Vec<&str> = out.data["posts"]["posts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["title"].as_str().unwrap())
        .collect();
    assert!(titles.contains(&"Hello World"));
    assert!(titles.contains(&"Draft Post"));
}

#[test]
fn single_post_by_slug_matches_rest_shape() {
    let db = seed();
    let out = block_on(execute(
        &db, 1,
        None,
        "{ post(slug: \"hello-world\") { id title slug status excerpt } }",
        None,
        None,
    ));
    assert!(out.errors.is_empty(), "errors: {:?}", out.errors);
    assert_eq!(out.data["post"]["title"], json!("Hello World"));
    assert_eq!(out.data["post"]["status"], json!("publish"));
}

#[test]
fn draft_post_hidden_from_anonymous_like_rest() {
    let db = seed();
    let out = block_on(execute(
        &db, 1,
        None,
        "{ post(slug: \"draft-post\") { id title } }",
        None,
        None,
    ));
    assert_eq!(out.data["post"], Value::Null);
    assert!(!out.errors.is_empty(), "expected a not-found error");

    let user = admin();
    let out = block_on(execute(
        &db, 1,
        Some(&user),
        "{ post(slug: \"draft-post\") { id title } }",
        None,
        None,
    ));
    assert!(out.errors.is_empty(), "errors: {:?}", out.errors);
    assert_eq!(out.data["post"]["title"], json!("Draft Post"));
}

#[test]
fn aliases_variables_and_fragments_work() {
    let db = seed();
    let out = block_on(execute(
        &db, 1,
        None,
        "query GetPost($slug: String) { hero: post(slug: $slug) { ...Bits } } fragment Bits on Post { id title }",
        Some(json!({ "slug": "hello-world" })),
        None,
    ));
    assert!(out.errors.is_empty(), "errors: {:?}", out.errors);
    assert_eq!(out.data["hero"]["title"], json!("Hello World"));
}

#[test]
fn nested_terms_and_author_resolve() {
    let db = seed();
    let out = block_on(execute(
        &db, 1,
        None,
        "{ post(slug: \"hello-world\") { title terms { name slug } author { id displayName } } }",
        None,
        None,
    ));
    assert!(out.errors.is_empty(), "errors: {:?}", out.errors);
    assert_eq!(out.data["post"]["terms"][0]["name"], json!("News"));
    assert_eq!(
        out.data["post"]["author"]["displayName"],
        json!("Admin User")
    );
}

#[test]
fn guarded_fields_reject_anonymous() {
    let db = seed();
    let out = block_on(execute(&db, 1, None, "{ users { total } }", None, None));
    assert_eq!(out.data["users"], Value::Null);
    assert!(!out.errors.is_empty());

    let out = block_on(execute(&db, 1, None, "{ media { total } }", None, None));
    assert_eq!(out.data["media"], Value::Null);
    assert!(!out.errors.is_empty());

    let user = admin();
    let out = block_on(execute(&db, 1, Some(&user), "{ users { total } }", None, None));
    assert!(out.errors.is_empty(), "errors: {:?}", out.errors);
    assert_eq!(out.data["users"]["total"], json!(1));
}

#[test]
fn site_info_and_terms_are_public() {
    let db = seed();
    let out = block_on(execute(
        &db, 1,
        None,
        "{ siteInfo { title url } terms { total terms { name } } }",
        None,
        None,
    ));
    assert!(out.errors.is_empty(), "errors: {:?}", out.errors);
    assert_eq!(out.data["siteInfo"]["title"], json!("Wordless Test"));
    assert_eq!(out.data["terms"]["total"], json!(1));
}

#[test]
fn bad_queries_return_errors_not_panics() {
    let db = seed();
    let out = block_on(execute(&db, 1, None, "{ nope { id } }", None, None));
    assert!(!out.errors.is_empty());
    let out = block_on(execute(&db, 1, None, "mutation { x }", None, None));
    assert_eq!(out.data, Value::Null);
    assert!(!out.errors.is_empty());
    let out = block_on(execute(&db, 1, None, "{ posts {", None, None));
    assert_eq!(out.data, Value::Null);
    assert!(!out.errors.is_empty());
}
