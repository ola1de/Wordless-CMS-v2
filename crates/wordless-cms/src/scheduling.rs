//! Scheduled publishing (P4, §0.1 A).
//!
//! # Decision: `draft` + a `_wl_publish_at` meta — NOT a `future` status
//!
//! v1 has no `future` post status (the Rust `PostStatus` enum asserts
//! `future` is NOT valid, and every public query filters
//! `post_status = 'publish'`). Adding a fifth status would touch the
//! enum, the status validation in `create_post`/`update_post`/
//! `list_posts`, the GraphQL post filter, the render queries, and the
//! v1 API contract (a client sending `status: "future"` to a v1
//! worker gets a 400 today). Instead, scheduling rides on the two
//! mechanisms v1 already has:
//!
//! * the post stays `draft` until its time — and drafts are already
//!   invisible on the public site (every render/feed/sitemap query
//!   requires `post_status = 'publish'`, and the API single-post
//!   handler 401s anonymous readers of non-published posts), so a
//!   scheduled post can never leak early through the public site;
//! * the target time lives in `wp_postmeta` under `_wl_publish_at`
//!   (plain ISO-8601 text, like the `_wl_seo_*` fields — not JSON),
//!   so no migration and no new table.
//!
//! Two flip paths, both calling the same service functions here:
//!
//! 1. **Cron Trigger sweep** (`#[event(scheduled)]` in the worker
//!    entry, `[triggers] crons = ["*/5 * * * *"]` in `wrangler.toml`):
//!    every 5 minutes `publish_due_posts` flips every due draft to
//!    `publish` and deletes its meta. This is the real-deploy path.
//! 2. **Lazy publish on read**: `GET /api/posts/:slug` (and the
//!    create/update handlers, via `set_publish_at`) call
//!    `maybe_publish_due` / `set_publish_at`, so a due post flips
//!    the moment anyone looks at it. This is the verifiable path:
//!    local `wrangler dev` fires no real cron, so the lazy check is
//!    what the smoke test and local dev exercise. The sweep is the
//!    same code, unit-tested; its cron *firing* can only be observed
//!    on a deployed Worker (P5).
//!
//! Semantics of `publishAt` (the editor's schedule control writes it
//! through `POST/PUT /api/posts`, as `publishAt` in the JSON body):
//!
//! * future time → post is stored as `draft` with the meta set
//!   (a `status: "publish"` in the same request is overridden — the
//!   schedule wins, and no `post.published` webhook fires yet);
//! * past/present time → the post publishes **now** (meta cleared);
//! * explicit `null` → any pending schedule is cleared;
//! * absent → no change.
//! * a manual `status: "publish"` update with no `publishAt` also
//!   clears a stale schedule (publishing early is a deliberate act).
//!
//! The scheduled time is exposed on the single-post response as
//! `publishAt` (the route attaches it; it is not part of the stored
//! `Post` shape, so list responses are unchanged — no API break for
//! existing clients).

use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, not_found, WordlessError};
use wordless_kernel::time::{iso_from_ms, ms_from_iso, now_iso};

use crate::postmeta::{delete_post_meta, get_post_meta, set_post_meta};
use crate::posts::{find_post_by_id, Post, PostStatus};

/// The schedule time lives here, in `wp_postmeta`, as plain ISO text.
pub const PUBLISH_AT_META_KEY: &str = "_wl_publish_at";

/// Parse a `publishAt` value. 400 on anything that is not ISO-8601.
pub fn parse_publish_at(raw: &str) -> Result<u64, WordlessError> {
    ms_from_iso(raw.trim()).ok_or_else(|| {
        bad_request(
            "Field \"publishAt\" must be an ISO-8601 UTC time, e.g. \"2026-12-01T09:00:00.000Z\".",
        )
    })
}

/// The pending schedule for a post, as stored (ISO text), or `None`.
pub async fn get_publish_at(db: &dyn Db, site_id: i64, post_id: i64) -> Result<Option<String>, WordlessError> {
    get_post_meta(db, site_id, post_id, PUBLISH_AT_META_KEY).await
}

/// Delete a post's pending schedule (a manual publish with no
/// `publishAt` clears a stale one). Returns `true` when a row was removed.
pub async fn delete_schedule(db: &dyn Db, site_id: i64, post_id: i64) -> Result<bool, WordlessError> {
    delete_post_meta(db, site_id, post_id, PUBLISH_AT_META_KEY).await
}

/// Set (or clear) a post's schedule.
///
/// * `None` → delete any pending schedule.
/// * `Some(iso)` in the future → store the meta and force the post to
///   `draft`; returns the stored ISO time.
/// * `Some(iso)` due or past → publish the post **now** (status
///   `publish`, meta cleared); returns `None` (nothing pending).
pub async fn set_publish_at(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    publish_at: Option<&str>,
    now_ms: u64,
) -> Result<Option<String>, WordlessError> {
    let Some(post) = find_post_by_id(db, site_id, post_id).await? else {
        return Err(not_found(&format!("Post {post_id} not found.")));
    };
    let Some(raw) = publish_at else {
        delete_post_meta(db, site_id, post_id, PUBLISH_AT_META_KEY).await?;
        return Ok(None);
    };
    let at_ms = parse_publish_at(raw)?;
    if at_ms <= now_ms {
        // Due already: publish now, no meta left behind.
        delete_post_meta(db, site_id, post_id, PUBLISH_AT_META_KEY).await?;
        if post.status != PostStatus::Publish {
            let now = now_iso();
            db.prepare("UPDATE wp_posts SET post_status = 'publish', post_modified = ?, post_modified_gmt = ? WHERE ID = ? AND site_id = ?")
                .bind(vec![
                    Param::Text(now.clone()),
                    Param::Text(now),
                    Param::Integer(post_id),
                    Param::Integer(site_id),
                ])
                .run()
                .await?;
        }
        return Ok(None);
    }
    let iso = iso_from_ms(at_ms);
    set_post_meta(db, site_id, post_id, PUBLISH_AT_META_KEY, &iso).await?;
    if post.status != PostStatus::Draft {
        db.prepare("UPDATE wp_posts SET post_status = 'draft' WHERE ID = ? AND site_id = ?")
            .bind(vec![Param::Integer(post_id), Param::Integer(site_id)])
            .run()
            .await?;
    }
    Ok(Some(iso))
}

/// Flip one due draft to `publish`. Returns `true` when it flipped.
/// A post with no schedule, a future schedule, or a non-draft status
/// is left alone.
pub async fn publish_due_post(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    now_ms: u64,
) -> Result<bool, WordlessError> {
    let Some(post) = find_post_by_id(db, site_id, post_id).await? else {
        return Ok(false);
    };
    if post.status != PostStatus::Draft {
        return Ok(false);
    }
    let Some(raw) = get_post_meta(db, site_id, post_id, PUBLISH_AT_META_KEY).await? else {
        return Ok(false);
    };
    let Some(at_ms) = ms_from_iso(raw.trim()) else {
        return Ok(false);
    };
    if at_ms > now_ms {
        return Ok(false);
    }
    db.prepare("UPDATE wp_posts SET post_status = 'publish' WHERE ID = ? AND site_id = ?")
        .bind(vec![Param::Integer(post_id), Param::Integer(site_id)])
        .run()
        .await?;
    delete_post_meta(db, site_id, post_id, PUBLISH_AT_META_KEY).await?;
    Ok(true)
}

/// The cron sweep: flip every due draft to `publish`, clear their
/// meta, and return how many flipped. Unparseable schedule values
/// are skipped (never crash the sweep on one bad row).
/// The cron sweep is network-wide by design (one sweep flips due
/// drafts on every site); each flip is still scoped to its own site.
pub async fn publish_due_posts(db: &dyn Db, now_ms: u64) -> Result<i64, WordlessError> {
    let rows: Vec<Row> = db
        .prepare(
            "SELECT p.ID AS id, p.site_id AS site_id, pm.meta_value AS at_raw FROM wp_posts p \
             JOIN wp_postmeta pm ON pm.post_id = p.ID AND pm.meta_key = ? AND pm.site_id = p.site_id \
             WHERE p.post_status = 'draft'",
        )
        .bind(vec![Param::Text(PUBLISH_AT_META_KEY.to_string())])
        .all()
        .await?;
    let mut flipped = 0i64;
    for row in &rows {
        let id = row_int(row, "id");
        let site_id = row_int(row, "site_id");
        let raw = row_text(row, "at_raw");
        let Some(at_ms) = ms_from_iso(raw.trim()) else {
            continue;
        };
        if at_ms > now_ms {
            continue;
        }
        db.prepare("UPDATE wp_posts SET post_status = 'publish' WHERE ID = ? AND site_id = ?")
            .bind(vec![Param::Integer(id), Param::Integer(site_id)])
            .run()
            .await?;
        delete_post_meta(db, site_id, id, PUBLISH_AT_META_KEY).await?;
        flipped += 1;
    }
    Ok(flipped)
}

/// Lazy publish on read: if this post is a due draft, flip it and
/// return the fresh row; otherwise return the post unchanged.
pub async fn maybe_publish_due(
    db: &dyn Db,
    post: Post,
    now_ms: u64,
) -> Result<Post, WordlessError> {
    if post.status != PostStatus::Draft {
        return Ok(post);
    }
    // The post carries its own site (internal-only field).
    if publish_due_post(db, post.site_id, post.id, now_ms).await? {
        let refreshed = find_post_by_id(db, post.site_id, post.id).await?;
        return Ok(refreshed.unwrap_or(post));
    }
    Ok(post)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::posts::{create_post, CreatePostInput};
    use crate::test_db::TestDb;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        futures_executor::block_on(f)
    }

    fn make_post(db: &TestDb, status: &str) -> Post {
        block_on(create_post(
            db,
            1,
            CreatePostInput {
                title: "Scheduled".to_string(),
                content: Some("Body".to_string()),
                excerpt: None,
                slug: None,
                status: Some(status.to_string()),
                post_type: None,
                author_id: 1,
                parent_id: None,
                mime_type: None,
                featured_image_id: None,
            },
        ))
        .unwrap()
    }

    #[test]
    fn future_schedule_keeps_draft_and_stores_meta() {
        let db = TestDb::new();
        let post = make_post(&db, "draft");
        let stored = block_on(set_publish_at(
            &db, 1,
            post.id,
            Some("2027-01-01T09:00:00.000Z"),
            1_795_000_000_000,
        ))
        .unwrap();
        assert_eq!(stored.as_deref(), Some("2027-01-01T09:00:00.000Z"));
        assert_eq!(
            block_on(get_publish_at(&db, 1, post.id)).unwrap().as_deref(),
            Some("2027-01-01T09:00:00.000Z")
        );
        // Still a draft — and the sweep / lazy check must not flip it.
        assert_eq!(
            block_on(find_post_by_id(&db, 1, post.id))
                .unwrap()
                .unwrap()
                .status,
            PostStatus::Draft
        );
        assert_eq!(
            block_on(publish_due_posts(&db, 1_795_000_000_000)).unwrap(),
            0
        );
        assert!(!block_on(publish_due_post(&db, 1, post.id, 1_795_000_000_000)).unwrap());
    }

    #[test]
    fn past_schedule_publishes_now_and_clears_meta() {
        let db = TestDb::new();
        let post = make_post(&db, "draft");
        let stored = block_on(set_publish_at(
            &db, 1,
            post.id,
            Some("2026-01-01T09:00:00.000Z"),
            1_795_000_000_000,
        ))
        .unwrap();
        assert_eq!(stored, None);
        assert_eq!(block_on(get_publish_at(&db, 1, post.id)).unwrap(), None);
        assert_eq!(
            block_on(find_post_by_id(&db, 1, post.id))
                .unwrap()
                .unwrap()
                .status,
            PostStatus::Publish
        );
    }

    #[test]
    fn sweep_flips_due_drafts_only() {
        let db = TestDb::new();
        // A due draft (meta written directly so the sweep, not
        // set_publish_at, does the flip), a future-scheduled draft,
        // and a plain draft with no schedule at all.
        let due = make_post(&db, "draft");
        block_on(set_post_meta(
            &db, 1,
            due.id,
            PUBLISH_AT_META_KEY,
            "2026-01-01T09:00:00.000Z",
        ))
        .unwrap();
        let future = make_post(&db, "draft");
        block_on(set_publish_at(
            &db, 1,
            future.id,
            Some("2027-01-01T09:00:00.000Z"),
            1_795_000_000_000,
        ))
        .unwrap();
        let _plain = make_post(&db, "draft");
        assert_eq!(
            block_on(publish_due_posts(&db, 1_795_000_000_000)).unwrap(),
            1
        );
        assert_eq!(
            block_on(find_post_by_id(&db, 1, due.id))
                .unwrap()
                .unwrap()
                .status,
            PostStatus::Publish
        );
        assert_eq!(block_on(get_publish_at(&db, 1, due.id)).unwrap(), None);
        // The future one is untouched until the clock passes it.
        assert_eq!(
            block_on(find_post_by_id(&db, 1, future.id))
                .unwrap()
                .unwrap()
                .status,
            PostStatus::Draft
        );
        assert_eq!(
            block_on(publish_due_posts(&db, 1_800_000_000_000)).unwrap(),
            1
        );
        assert_eq!(
            block_on(find_post_by_id(&db, 1, future.id))
                .unwrap()
                .unwrap()
                .status,
            PostStatus::Publish
        );
    }

    #[test]
    fn lazy_check_flips_on_read_and_ignores_future() {
        let db = TestDb::new();
        let post = make_post(&db, "draft");
        block_on(set_publish_at(
            &db, 1,
            post.id,
            Some("2027-01-01T09:00:00.000Z"),
            1_795_000_000_000,
        ))
        .unwrap();
        let still_draft =
            block_on(maybe_publish_due(&db, post.clone(), 1_795_000_000_000)).unwrap();
        assert_eq!(still_draft.status, PostStatus::Draft);
        let flipped = block_on(maybe_publish_due(&db, post, 1_800_000_000_000)).unwrap();
        assert_eq!(flipped.status, PostStatus::Publish);
    }

    #[test]
    fn invalid_time_is_400_and_clearing_works() {
        let db = TestDb::new();
        let post = make_post(&db, "draft");
        assert!(block_on(set_publish_at(
            &db, 1,
            post.id,
            Some("not-a-date"),
            1_795_000_000_000
        ))
        .is_err());
        block_on(set_publish_at(
            &db, 1,
            post.id,
            Some("2027-01-01T09:00:00.000Z"),
            1_795_000_000_000,
        ))
        .unwrap();
        assert!(block_on(get_publish_at(&db, 1, post.id)).unwrap().is_some());
        assert_eq!(
            block_on(set_publish_at(&db, 1, post.id, None, 1_795_000_000_000)).unwrap(),
            None
        );
        assert_eq!(block_on(get_publish_at(&db, 1, post.id)).unwrap(), None);
        assert_eq!(
            block_on(find_post_by_id(&db, 1, post.id))
                .unwrap()
                .unwrap()
                .status,
            PostStatus::Draft
        );
    }

    #[test]
    fn future_status_is_still_not_a_valid_status() {
        // The scheduling decision deliberately keeps the v1 status set:
        // `future` must not become valid because scheduling rides on
        // draft + meta.
        assert!(!crate::posts::is_post_status("future"));
    }
}
