//! Test-only in-memory fakes for `Db` and `ObjectStore`.
//!
//! The `Db` trait's `prepare` returns a `Statement` whose constructor is
//! `pub(crate)` to `wordless-db`, so no external crate can build one.
//! This crate's service code therefore uses only the primitive `Db`
//! methods (`query_all` / `query_one` / `exec` / `batch`) — never
//! `prepare` — and the fake implements `prepare` as `unreachable!()`.
//! The fake interprets exactly the SQL strings this crate issues (see
//! `registry.rs` / `customizer.rs`); anything else is an error, so a
//! typo in a service query fails loudly in tests instead of silently
//! returning empty rows.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::{json, Value};
use wordless_db::{Db, DbError, Param, Row, RunMeta, Statement};
use wordless_kernel::error::WordlessError;
use wordless_kernel::storage::{ListPage, ObjectMeta, ObjectStore};

#[derive(Default)]
struct FakeState {
    /// `wl_themes` rows, as JSON objects keyed by column name.
    themes: Vec<Value>,
    /// `wp_options` rows: name → raw JSON text.
    options: HashMap<String, String>,
}

pub struct FakeDb {
    state: Mutex<FakeState>,
}

impl FakeDb {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(FakeState::default()),
        }
    }

    fn param_text(params: &[Param], i: usize) -> String {
        match params.get(i) {
            Some(Param::Text(s)) => s.clone(),
            Some(Param::Integer(n)) => n.to_string(),
            Some(Param::Real(f)) => f.to_string(),
            _ => String::new(),
        }
    }

    fn param_opt_text(params: &[Param], i: usize) -> Option<String> {
        match params.get(i) {
            Some(Param::Text(s)) => Some(s.clone()),
            Some(Param::Null) | None => None,
            Some(Param::Integer(n)) => Some(n.to_string()),
            Some(Param::Real(f)) => Some(f.to_string()),
            Some(Param::Blob(_)) => None,
        }
    }

    fn query_rows(&self, sql: &str, params: &[Param]) -> Result<Vec<Row>, DbError> {
        let state = self.state.lock().unwrap();
        if sql.contains("SELECT COUNT(*) AS n FROM wl_themes") {
            return Ok(vec![json!({ "n": state.themes.len() })]);
        }
        if sql.contains("SELECT * FROM wl_themes ORDER BY slug ASC") {
            let mut rows = state.themes.clone();
            rows.sort_by_key(|r| r["slug"].as_str().unwrap_or("").to_string());
            return Ok(rows);
        }
        if sql.contains("SELECT * FROM wl_themes WHERE slug = ?") {
            let slug = Self::param_text(params, 0);
            return Ok(state
                .themes
                .iter()
                .filter(|r| r["slug"].as_str().unwrap_or("") == slug)
                .cloned()
                .collect());
        }
        if sql.contains("SELECT slug FROM wl_themes WHERE slug = ?") {
            let slug = Self::param_text(params, 0);
            return Ok(state
                .themes
                .iter()
                .filter(|r| r["slug"].as_str().unwrap_or("") == slug)
                .map(|r| json!({ "slug": r["slug"] }))
                .collect());
        }
        if sql.contains("SELECT version FROM wl_themes WHERE slug = ?") {
            let slug = Self::param_text(params, 0);
            return Ok(state
                .themes
                .iter()
                .filter(|r| r["slug"].as_str().unwrap_or("") == slug && r["source"] == "builtin")
                .map(|r| json!({ "version": r["version"] }))
                .collect());
        }
        // P12: `theme_active_anywhere` asks whether any *other* site has
        // a theme active. This double models a single-site install
        // (site 1 only, no wl_site_options rows), so the answer is
        // always "no rows".
        if sql.contains("FROM wl_site_options WHERE option_name = 'active_theme'") {
            return Ok(Vec::new());
        }
        if sql.contains("SELECT option_value FROM wp_options WHERE option_name = ?") {
            let name = Self::param_text(params, 0);
            return Ok(match state.options.get(&name) {
                Some(v) => vec![json!({ "option_value": v })],
                None => Vec::new(),
            });
        }
        Err(DbError::D1(format!("FakeDb: unexpected query: {sql}")))
    }

    fn exec_sql(&self, sql: &str, params: &[Param]) -> Result<RunMeta, DbError> {
        let mut state = self.state.lock().unwrap();
        if sql.contains("INSERT OR IGNORE INTO wl_themes") {
            let slug = Self::param_text(params, 0);
            if state.themes.iter().any(|r| r["slug"] == slug) {
                return Ok(RunMeta {
                    last_row_id: None,
                    changes: 0,
                });
            }
            state.themes.push(theme_row_from_params(params));
            return Ok(RunMeta {
                last_row_id: None,
                changes: 1,
            });
        }
        if sql.contains("INSERT INTO wl_themes") {
            let slug = Self::param_text(params, 0);
            if let Some(row) = state
                .themes
                .iter_mut()
                .find(|r| r["slug"].as_str().unwrap_or("") == slug)
            {
                // ON CONFLICT DO UPDATE SET — status and created_at stay.
                row["name"] = json!(Self::param_text(params, 1));
                row["version"] = json!(Self::param_text(params, 2));
                row["author"] = json!(Self::param_text(params, 3));
                row["kind"] = json!(Self::param_text(params, 4));
                row["theme_json"] = json!(Self::param_opt_text(params, 6));
                row["stylesheet"] = json!(Self::param_text(params, 7));
                row["stylesheet_hash"] = json!(Self::param_text(params, 8));
                row["source"] = json!(Self::param_text(params, 9));
                row["report_json"] = json!(Self::param_opt_text(params, 10));
                row["updated_at"] = json!(Self::param_text(params, 12));
                return Ok(RunMeta {
                    last_row_id: None,
                    changes: 1,
                });
            }
            state.themes.push(theme_row_from_params(params));
            return Ok(RunMeta {
                last_row_id: None,
                changes: 1,
            });
        }
        if sql.contains("UPDATE wl_themes") && sql.contains("SET theme_json") {
            let slug = Self::param_text(params, 5);
            let mut changes = 0;
            for row in state.themes.iter_mut() {
                if row["slug"].as_str().unwrap_or("") == slug && row["source"] == "builtin" {
                    row["theme_json"] = json!(Self::param_opt_text(params, 0));
                    row["stylesheet"] = json!(Self::param_text(params, 1));
                    row["stylesheet_hash"] = json!(Self::param_text(params, 2));
                    row["version"] = json!(Self::param_text(params, 3));
                    row["updated_at"] = json!(Self::param_text(params, 4));
                    changes += 1;
                }
            }
            return Ok(RunMeta {
                last_row_id: None,
                changes,
            });
        }
        if sql.contains("UPDATE wl_themes SET status = 'inactive'") {
            let now = Self::param_text(params, 0);
            let mut changes = 0;
            for row in state.themes.iter_mut() {
                if row["status"] == "active" {
                    row["status"] = json!("inactive");
                    row["updated_at"] = json!(now.clone());
                    changes += 1;
                }
            }
            return Ok(RunMeta {
                last_row_id: None,
                changes,
            });
        }
        if sql.contains("UPDATE wl_themes SET status = 'active'") {
            let now = Self::param_text(params, 0);
            let slug = Self::param_text(params, 1);
            let mut changes = 0;
            for row in state.themes.iter_mut() {
                if row["slug"].as_str().unwrap_or("") == slug {
                    row["status"] = json!("active");
                    row["updated_at"] = json!(now.clone());
                    changes += 1;
                }
            }
            return Ok(RunMeta {
                last_row_id: None,
                changes,
            });
        }
        if sql.contains("DELETE FROM wl_themes WHERE slug = ?") {
            let slug = Self::param_text(params, 0);
            let before = state.themes.len();
            state
                .themes
                .retain(|r| r["slug"].as_str().unwrap_or("") != slug);
            return Ok(RunMeta {
                last_row_id: None,
                changes: (before - state.themes.len()) as u64,
            });
        }
        if sql.contains("INSERT OR IGNORE INTO wp_options") {
            let name = Self::param_text(params, 0);
            let value = Self::param_text(params, 1);
            if state.options.contains_key(&name) {
                return Ok(RunMeta {
                    last_row_id: None,
                    changes: 0,
                });
            }
            state.options.insert(name, value);
            return Ok(RunMeta {
                last_row_id: None,
                changes: 1,
            });
        }
        if sql.contains("INSERT INTO wp_options") {
            let name = Self::param_text(params, 0);
            let value = Self::param_text(params, 1);
            state.options.insert(name, value);
            return Ok(RunMeta {
                last_row_id: None,
                changes: 1,
            });
        }
        // P12: `delete_theme` removes the theme's variation option
        // from every site. This double models a single-site install,
        // so per-site variation options live in `state.options`
        // (site 1) — remove the key there; there are no other sites.
        if sql.contains("DELETE FROM wl_site_options WHERE option_name = ?") {
            let name = Self::param_text(params, 0);
            let removed = state.options.remove(&name).is_some();
            return Ok(RunMeta {
                last_row_id: None,
                changes: u64::from(removed),
            });
        }
        if sql.contains("DELETE FROM wp_options WHERE option_name = ?") {
            let name = Self::param_text(params, 0);
            let removed = state.options.remove(&name).is_some();
            return Ok(RunMeta {
                last_row_id: None,
                changes: u64::from(removed),
            });
        }
        Err(DbError::D1(format!("FakeDb: unexpected exec: {sql}")))
    }
}

/// Build a `wl_themes` row object from the 13 upsert parameters, in the
/// column order the service uses:
/// slug, name, version, author, kind, status, theme_json, stylesheet,
/// stylesheet_hash, source, report_json, created_at, updated_at.
fn theme_row_from_params(params: &[Param]) -> Value {
    json!({
        "slug": FakeDb::param_text(params, 0),
        "name": FakeDb::param_text(params, 1),
        "version": FakeDb::param_text(params, 2),
        "author": FakeDb::param_text(params, 3),
        "kind": FakeDb::param_text(params, 4),
        "status": FakeDb::param_text(params, 5),
        "theme_json": FakeDb::param_opt_text(params, 6),
        "stylesheet": FakeDb::param_text(params, 7),
        "stylesheet_hash": FakeDb::param_text(params, 8),
        "source": FakeDb::param_text(params, 9),
        "report_json": FakeDb::param_opt_text(params, 10),
        "created_at": FakeDb::param_text(params, 11),
        "updated_at": FakeDb::param_text(params, 12),
    })
}

impl Default for FakeDb {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait(?Send)]
impl Db for FakeDb {
    fn prepare(&self, _sql: &str) -> Statement<'_> {
        unreachable!(
            "FakeDb does not implement prepare; service code uses the primitive query methods"
        )
    }

    async fn query_all(&self, sql: &str, params: &[Param]) -> Result<Vec<Row>, DbError> {
        self.query_rows(sql, params)
    }

    async fn query_one(&self, sql: &str, params: &[Param]) -> Result<Option<Row>, DbError> {
        Ok(self.query_rows(sql, params)?.into_iter().next())
    }

    async fn exec(&self, sql: &str, params: &[Param]) -> Result<RunMeta, DbError> {
        self.exec_sql(sql, params)
    }

    async fn batch(&self, stmts: &[(String, Vec<Param>)]) -> Result<(), DbError> {
        for (sql, params) in stmts {
            self.exec_sql(sql, params)?;
        }
        Ok(())
    }
}

/// In-memory `ObjectStore` fake.
pub struct FakeStore {
    objects: Mutex<HashMap<String, (Vec<u8>, String)>>,
}

impl FakeStore {
    pub fn new() -> Self {
        Self {
            objects: Mutex::new(HashMap::new()),
        }
    }

    /// Keys currently stored, sorted.
    pub fn keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.objects.lock().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    }

    /// Keys under `prefix`, sorted.
    pub fn keys_under(&self, prefix: &str) -> Vec<String> {
        self.keys()
            .into_iter()
            .filter(|k| k.starts_with(prefix))
            .collect()
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
        let total = self.objects.lock().unwrap().len();
        Ok((total.min(limit), total > limit))
    }

    async fn get_object(
        &self,
        key: &str,
    ) -> Result<Option<(Vec<u8>, Option<String>)>, WordlessError> {
        Ok(self
            .objects
            .lock()
            .unwrap()
            .get(key)
            .map(|(bytes, ct)| (bytes.clone(), Some(ct.clone()))))
    }

    async fn put_object(
        &self,
        key: &str,
        bytes: Vec<u8>,
        content_type: &str,
    ) -> Result<(), WordlessError> {
        self.objects
            .lock()
            .unwrap()
            .insert(key.to_string(), (bytes, content_type.to_string()));
        Ok(())
    }

    async fn delete_object(&self, key: &str) -> Result<(), WordlessError> {
        self.objects.lock().unwrap().remove(key);
        Ok(())
    }

    async fn head_object(&self, key: &str) -> Result<Option<ObjectMeta>, WordlessError> {
        Ok(self
            .objects
            .lock()
            .unwrap()
            .get(key)
            .map(|(bytes, ct)| ObjectMeta {
                key: key.to_string(),
                size: bytes.len() as u64,
                content_type: Some(ct.clone()),
                uploaded_at: None,
            }))
    }

    async fn list_objects(
        &self,
        prefix: &str,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<ListPage, WordlessError> {
        let keys = self.keys_under(prefix);
        let start: usize = cursor.and_then(|c| c.parse().ok()).unwrap_or(0);
        let page: Vec<ObjectMeta> = keys
            .iter()
            .skip(start)
            .take(limit)
            .map(|k| {
                let (bytes, ct) = self.objects.lock().unwrap().get(k).unwrap().clone();
                ObjectMeta {
                    key: k.clone(),
                    size: bytes.len() as u64,
                    content_type: Some(ct),
                    uploaded_at: None,
                }
            })
            .collect();
        let next = start + page.len();
        let truncated = next < keys.len();
        Ok(ListPage {
            objects: page,
            truncated,
            cursor: if truncated {
                Some(next.to_string())
            } else {
                None
            },
        })
    }
}
