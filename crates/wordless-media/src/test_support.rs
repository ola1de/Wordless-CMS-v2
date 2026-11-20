//! Test-only fakes: an in-memory `Db` and an in-memory `ObjectStore`.
//!
//! Mirrors v1's `src/test/test-db.ts` + `src/test/fake-storage.ts`
//! (real bytes, real SQL-shape rows, zero infrastructure). NEVER a
//! live bucket or D1 in tests.
//!
//! The `FakeDb` understands only the SQL this crate issues (the exact
//! statements in `service.rs` / `files.rs` / `optimize.rs`'s
//! dependencies). It matches on statement shape, not by parsing
//! SQL — that is deliberate: the fake is a test double for this
//! crate's queries, not a SQL engine.

use std::cell::RefCell;
use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};
use wordless_db::{Db, DbError, Param, Row, RunMeta, Statement};
use wordless_kernel::{
    error::WordlessError,
    storage::{ListPage, ObjectMeta, ObjectStore},
    time::now_iso,
};

/// Run an async block on the native test thread.
pub fn block_on<F: std::future::Future>(f: F) -> F::Output {
    futures_executor::block_on(f)
}

fn param_text(p: &Param) -> String {
    match p {
        Param::Text(s) => s.clone(),
        Param::Integer(n) => n.to_string(),
        Param::Real(f) => f.to_string(),
        Param::Null => String::new(),
        Param::Blob(b) => String::from_utf8_lossy(b).into_owned(),
    }
}

fn param_int(p: &Param) -> i64 {
    match p {
        Param::Integer(n) => *n,
        Param::Real(f) => *f as i64,
        Param::Text(s) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

#[derive(Default)]
struct FakeState {
    posts: Vec<Row>,
    meta: Vec<Row>,
    options: Vec<Row>,
    derivatives: Vec<Row>,
    next_post_id: i64,
    next_meta_id: i64,
    next_deriv_id: i64,
}

pub struct FakeDb {
    state: RefCell<FakeState>,
}

impl FakeDb {
    pub fn new() -> Self {
        Self {
            state: RefCell::new(FakeState {
                next_post_id: 1,
                next_meta_id: 1,
                next_deriv_id: 1,
                ..Default::default()
            }),
        }
    }

    /// Seed an option row (e.g. `default_comment_status`).
    #[allow(dead_code)]
    pub fn set_option(&self, name: &str, value: &str) {
        let mut s = self.state.borrow_mut();
        s.options
            .retain(|r| r.get("option_name").and_then(|v| v.as_str()) != Some(name));
        s.options
            .push(json!({ "option_name": name, "option_value": value }));
    }

    fn post_matches_list(row: &Row, params: &[Param]) -> bool {
        // P12 params: [site_id, "attachment", "trash", optional mime].
        if row.get("post_type").and_then(|v| v.as_str()) != Some("attachment") {
            return false;
        }
        if row.get("post_status").and_then(|v| v.as_str()) == Some("trash") {
            return false;
        }
        if params.len() > 3 {
            let mime = param_text(&params[3]);
            if row.get("post_mime_type").and_then(|v| v.as_str()) != Some(mime.as_str()) {
                return false;
            }
        }
        true
    }

    fn count_posts(&self, params: &[Param]) -> i64 {
        let s = self.state.borrow();
        s.posts
            .iter()
            .filter(|r| Self::post_matches_list(r, params))
            .count() as i64
    }

    fn list_posts(&self, params: &[Param]) -> Vec<Row> {
        // Last two params are LIMIT then OFFSET.
        let n = params.len();
        let limit = if n >= 2 {
            param_int(&params[n - 2])
        } else {
            20
        };
        let offset = if n >= 2 { param_int(&params[n - 1]) } else { 0 };
        let filter_params = if n >= 2 { &params[..n - 2] } else { params };
        let s = self.state.borrow();
        let mut rows: Vec<Row> = s
            .posts
            .iter()
            .filter(|r| Self::post_matches_list(r, filter_params))
            .cloned()
            .collect();
        // ORDER BY post_date DESC, ID DESC.
        rows.sort_by(|a, b| {
            let da = a.get("post_date").and_then(|v| v.as_str()).unwrap_or("");
            let db = b.get("post_date").and_then(|v| v.as_str()).unwrap_or("");
            db.cmp(da).then(
                b.get("ID")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0)
                    .cmp(&a.get("ID").and_then(|v| v.as_i64()).unwrap_or(0)),
            )
        });
        let start = (offset as usize).min(rows.len());
        let end = start.saturating_add(limit.max(0) as usize).min(rows.len());
        rows[start..end].to_vec()
    }
}

impl Default for FakeDb {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait(?Send)]
impl Db for FakeDb {
    fn prepare(&self, sql: &str) -> Statement<'_> {
        // P12: the service now reaches `prepare` through the
        // multisite option helpers (site-scoped reads), so the fake
        // must implement it. `Statement::new` is public; the
        // statement routes back through this fake's trait methods.
        Statement::new(self as &dyn Db, sql.to_string())
    }

    async fn query_all(&self, sql: &str, params: &[Param]) -> Result<Vec<Row>, DbError> {
        if sql.contains("FROM wp_posts") && sql.contains("ORDER BY") {
            return Ok(self.list_posts(params));
        }
        if sql.contains("FROM wp_postmeta WHERE post_id") {
            // P12: get_meta is (post_id, key, site_id); get_map is
            // (post_id, site_id) and returns every key for the post.
            let post_id = param_int(&params[0]);
            let keys: Vec<String> = if sql.contains("meta_key = ?") {
                vec![param_text(&params[1])]
            } else {
                Vec::new()
            };
            let s = self.state.borrow();
            return Ok(s
                .meta
                .iter()
                .filter(|r| {
                    r.get("post_id").and_then(|v| v.as_i64()) == Some(post_id)
                        && (keys.is_empty()
                            || r.get("meta_key")
                                .and_then(|v| v.as_str())
                                .map(|k| keys.iter().any(|key| key == k))
                                .unwrap_or(false))
                })
                .cloned()
                .collect());
        }
        if sql.contains("FROM wp_postmeta WHERE meta_key = '_wp_attached_file'") {
            let s = self.state.borrow();
            return Ok(s
                .meta
                .iter()
                .filter(|r| r.get("meta_key").and_then(|v| v.as_str()) == Some("_wp_attached_file"))
                .cloned()
                .collect());
        }
        if sql.contains("FROM wl_media_derivatives") && sql.contains("ORDER BY width") {
            let media_id = param_int(&params[0]);
            let s = self.state.borrow();
            let mut rows: Vec<Row> = s
                .derivatives
                .iter()
                .filter(|r| r.get("media_id").and_then(|v| v.as_i64()) == Some(media_id))
                .cloned()
                .collect();
            rows.sort_by_key(|r| r.get("width").and_then(|v| v.as_i64()).unwrap_or(0));
            return Ok(rows);
        }
        Ok(Vec::new())
    }

    async fn query_one(&self, sql: &str, params: &[Param]) -> Result<Option<Row>, DbError> {
        if sql.contains("SELECT * FROM wp_posts WHERE ID") {
            let id = param_int(&params[0]);
            let s = self.state.borrow();
            return Ok(s
                .posts
                .iter()
                .find(|r| r.get("ID").and_then(|v| v.as_i64()) == Some(id))
                .cloned());
        }
        if sql.contains("SELECT * FROM wp_posts WHERE post_name") {
            let slug = param_text(&params[0]);
            let s = self.state.borrow();
            return Ok(s
                .posts
                .iter()
                .find(|r| {
                    r.get("post_name").and_then(|v| v.as_str()) == Some(slug.as_str())
                        && matches!(
                            r.get("post_type").and_then(|v| v.as_str()),
                            Some("post") | Some("page")
                        )
                })
                .cloned());
        }
        if sql.contains("COUNT(*) AS n FROM wp_posts") {
            return Ok(Some(json!({ "n": self.count_posts(params) })));
        }
        if sql.contains("SELECT post_id FROM wp_postmeta") {
            let (key, value) = (param_text(&params[0]), param_text(&params[1]));
            let s = self.state.borrow();
            return Ok(s
                .meta
                .iter()
                .find(|r| {
                    r.get("meta_key").and_then(|v| v.as_str()) == Some(key.as_str())
                        && r.get("meta_value").and_then(|v| v.as_str()) == Some(value.as_str())
                })
                .cloned());
        }
        if sql.contains("FROM wp_options") {
            let name = param_text(&params[0]);
            let s = self.state.borrow();
            return Ok(s
                .options
                .iter()
                .find(|r| r.get("option_name").and_then(|v| v.as_str()) == Some(name.as_str()))
                .cloned());
        }
        if sql.contains("COUNT(*) AS n FROM wl_media_derivatives") {
            let s = self.state.borrow();
            return Ok(Some(json!({ "n": s.derivatives.len() as i64 })));
        }
        if sql.contains("FROM wl_media_derivatives WHERE media_id") {
            let (media_id, width, format) = (
                param_int(&params[0]),
                param_int(&params[1]),
                param_text(&params[2]),
            );
            let s = self.state.borrow();
            return Ok(s
                .derivatives
                .iter()
                .find(|r| {
                    r.get("media_id").and_then(|v| v.as_i64()) == Some(media_id)
                        && r.get("width").and_then(|v| v.as_i64()) == Some(width)
                        && r.get("format").and_then(|v| v.as_str()) == Some(format.as_str())
                })
                .cloned());
        }
        Ok(None)
    }

    async fn exec(&self, sql: &str, params: &[Param]) -> Result<RunMeta, DbError> {
        let mut s = self.state.borrow_mut();
        if sql.starts_with("INSERT INTO wp_posts") {
            let id = s.next_post_id;
            s.next_post_id += 1;
            // P12 repo column order: site_id, author, date, date_gmt,
            // content, title, excerpt, status, slug, modified,
            // modified_gmt, parent, type, mime, comment_status.
            let t = |i: usize| param_text(&params[i]);
            s.posts.push(json!({
                "ID": id,
                "site_id": param_int(&params[0]),
                "post_author": param_int(&params[1]),
                "post_date": t(2),
                "post_date_gmt": t(3),
                "post_content": t(4),
                "post_title": t(5),
                "post_excerpt": t(6),
                "post_status": t(7),
                "post_name": t(8),
                "post_modified": t(9),
                "post_modified_gmt": t(10),
                "post_parent": param_int(&params[11]),
                "post_type": t(12),
                "post_mime_type": t(13),
                "comment_status": t(14),
                "ping_status": "open",
                "post_password": "",
                "post_content_filtered": "",
                "guid": "",
                "menu_order": 0,
                "comment_count": 0,
            }));
            return Ok(RunMeta {
                last_row_id: Some(id),
                changes: 1,
            });
        }
        if sql.starts_with("INSERT INTO wp_postmeta") {
            let id = s.next_meta_id;
            s.next_meta_id += 1;
            // P12 column order: (site_id, post_id, meta_key, meta_value).
            s.meta.push(json!({
                "meta_id": id,
                "site_id": param_int(&params[0]),
                "post_id": param_int(&params[1]),
                "meta_key": param_text(&params[2]),
                "meta_value": param_text(&params[3]),
            }));
            return Ok(RunMeta {
                last_row_id: Some(id),
                changes: 1,
            });
        }
        if sql.starts_with("UPDATE wp_postmeta") {
            let (value, post_id, key) = (
                param_text(&params[0]),
                param_int(&params[1]),
                param_text(&params[2]),
            );
            let mut changes = 0u64;
            for row in s.meta.iter_mut() {
                if row.get("post_id").and_then(|v| v.as_i64()) == Some(post_id)
                    && row.get("meta_key").and_then(|v| v.as_str()) == Some(key.as_str())
                {
                    row["meta_value"] = Value::String(value.clone());
                    changes += 1;
                }
            }
            return Ok(RunMeta {
                last_row_id: None,
                changes,
            });
        }
        if sql.starts_with("DELETE FROM wp_postmeta") {
            let post_id = param_int(&params[0]);
            let before = s.meta.len();
            s.meta
                .retain(|r| r.get("post_id").and_then(|v| v.as_i64()) != Some(post_id));
            return Ok(RunMeta {
                last_row_id: None,
                changes: (before - s.meta.len()) as u64,
            });
        }
        if sql.starts_with("DELETE FROM wp_posts WHERE post_parent") {
            return Ok(RunMeta {
                last_row_id: None,
                changes: 0,
            });
        }
        if sql.starts_with("DELETE FROM wp_posts WHERE ID") {
            let id = param_int(&params[0]);
            let before = s.posts.len();
            s.posts
                .retain(|r| r.get("ID").and_then(|v| v.as_i64()) != Some(id));
            return Ok(RunMeta {
                last_row_id: None,
                changes: (before - s.posts.len()) as u64,
            });
        }
        if sql.starts_with("INSERT INTO wl_media_derivatives") {
            let id = s.next_deriv_id;
            s.next_deriv_id += 1;
            s.derivatives.push(json!({
                "id": id,
                "media_id": param_int(&params[0]),
                "width": param_int(&params[1]),
                "format": param_text(&params[2]),
                "object_key": param_text(&params[3]),
                "size": param_int(&params[4]),
                "created_at": param_text(&params[5]),
            }));
            return Ok(RunMeta {
                last_row_id: Some(id),
                changes: 1,
            });
        }
        Ok(RunMeta {
            last_row_id: None,
            changes: 0,
        })
    }

    async fn batch(&self, stmts: &[(String, Vec<Param>)]) -> Result<(), DbError> {
        for (sql, params) in stmts {
            self.exec(sql, params).await?;
        }
        Ok(())
    }
}

/// In-memory `ObjectStore` — real bytes, lexicographic listing with
/// cursor pagination, like R2.
pub struct FakeStore {
    #[allow(clippy::type_complexity)]
    objects: RefCell<HashMap<String, (Vec<u8>, String, String)>>,
}

impl FakeStore {
    pub fn new() -> Self {
        Self {
            objects: RefCell::new(HashMap::new()),
        }
    }

    /// Synchronous test helper: does the key exist?
    pub fn contains(&self, key: &str) -> bool {
        self.objects.borrow().contains_key(key)
    }

    /// Synchronous test helper: number of stored objects.
    pub fn len(&self) -> usize {
        self.objects.borrow().len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for FakeStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait(?Send)]
impl ObjectStore for FakeStore {
    async fn count_objects(&self, limit: usize) -> Result<(usize, bool), WordlessError> {
        let total = self.objects.borrow().len();
        Ok((total.min(limit), total > limit))
    }

    async fn get_object(
        &self,
        key: &str,
    ) -> Result<Option<(Vec<u8>, Option<String>)>, WordlessError> {
        Ok(self
            .objects
            .borrow()
            .get(key)
            .map(|(bytes, ct, _)| (bytes.clone(), Some(ct.clone()))))
    }

    async fn put_object(
        &self,
        key: &str,
        bytes: Vec<u8>,
        content_type: &str,
    ) -> Result<(), WordlessError> {
        self.objects.borrow_mut().insert(
            key.to_string(),
            (bytes, content_type.to_string(), now_iso()),
        );
        Ok(())
    }

    async fn delete_object(&self, key: &str) -> Result<(), WordlessError> {
        self.objects.borrow_mut().remove(key);
        Ok(())
    }

    async fn head_object(&self, key: &str) -> Result<Option<ObjectMeta>, WordlessError> {
        Ok(self
            .objects
            .borrow()
            .get(key)
            .map(|(bytes, ct, uploaded)| ObjectMeta {
                key: key.to_string(),
                size: bytes.len() as u64,
                content_type: Some(ct.clone()),
                uploaded_at: Some(uploaded.clone()),
            }))
    }

    async fn list_objects(
        &self,
        prefix: &str,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<ListPage, WordlessError> {
        let objects = self.objects.borrow();
        let mut keys: Vec<&String> = objects.keys().filter(|k| k.starts_with(prefix)).collect();
        keys.sort();
        let start = cursor.and_then(|c| c.parse::<usize>().ok()).unwrap_or(0);
        let limit = if limit == 0 { 1000 } else { limit };
        let end = (start + limit).min(keys.len());
        let page_objects: Vec<ObjectMeta> = keys[start..end]
            .iter()
            .map(|k| {
                let (bytes, ct, uploaded) = &objects[*k];
                ObjectMeta {
                    key: (*k).clone(),
                    size: bytes.len() as u64,
                    content_type: Some(ct.clone()),
                    uploaded_at: Some(uploaded.clone()),
                }
            })
            .collect();
        let truncated = end < keys.len();
        Ok(ListPage {
            objects: page_objects,
            truncated,
            cursor: if truncated {
                Some(end.to_string())
            } else {
                None
            },
        })
    }
}
