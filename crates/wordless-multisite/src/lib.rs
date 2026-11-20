//! Multisite (P12) — one install running many sites.
//!
//! Q14 (decided 2026-11-20, parent decision at P12 start):
//! - **Data model:** site-id column on the `wp_*` tables + per-site
//!   options (`wl_site_options`); single-site installs carry site id 1
//!   by default so nothing migrates.
//! - **Domain routing:** PATH-BASED sites by default
//!   (`example.com/<site-slug>/...` — zero config, works on the free
//!   tier). Subdomain routing (`<slug>.example.com`) via Workers routes
//!   as a supported option. Custom-hostname domain mapping is OUT
//!   (needs Cloudflare for SaaS, paid — ruled out by the user's
//!   no-money constraint).
//!
//! Isolation is the load-bearing property: every core CMS query takes
//! a `site_id` and filters on it, so a per-site query can never leak
//! content across sites. Users are SHARED across sites (one login for
//! the whole install); only content, settings and themes are
//! per-site. The sub-worker tables (`wl_social_*`, `wl_woo_*`,
//! `wl_courses`/`wl_exams`, `wl_seo_audit_runs`) are NOT site-scoped
//! in P12 — they are network-wide, recorded as an honest limit in
//! the P12 outcome block.
//!
//! Pure logic over `wordless-db` — no Worker types, cargo-testable.

use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, not_found, WordlessError};
use wordless_kernel::time::now_iso;

/// The main site. Single-site installs only ever have this site, and
/// every `site_id` column defaults to it, so the P12 migration is a
/// no-op for them.
pub const MAIN_SITE_ID: i64 = 1;

/// Request header the admin SPA sends (`X-Wordless-Site: <id|slug>`)
/// to pick the site its API calls act on. Honoured only for
/// authenticated users with `edit_posts` (enforced by the caller in
/// `wordless-worker`); public requests resolve by path/host only.
pub const SITE_HEADER: &str = "x-wordless-site";

/// Option keys that are per-site. For site 1 they keep living in
/// `wp_options` exactly as before; for other sites they live in
/// `wl_site_options` and fall back to `wp_options` when unset.
pub const SITE_SCOPED_KEYS: [&str; 13] = [
    "blogname",
    "blogdescription",
    "template",
    "stylesheet",
    "posts_per_page",
    "permalink_structure",
    "timezone_string",
    "default_comment_status",
    "comment_max_links",
    "active_theme",
    "siteurl",
    "wl_robots_txt",
    "wl_cwv_budgets",
];

/// Exact-list keys plus the per-site prefixed families: login
/// branding (`wl_login_*`) and theme variations
/// (`wl_theme_variation_*`) are per-site too.
pub fn is_site_scoped_key(key: &str) -> bool {
    SITE_SCOPED_KEYS.contains(&key)
        || key.starts_with("wl_login_")
        || key.starts_with("wl_theme_variation_")
}

/// First path segments that can never be a site slug: they are real
/// top-level routes, and a site with that slug would shadow them.
/// Enforced at site creation; resolution also skips them so a
/// pre-existing route always wins over a site row.
pub const RESERVED_SLUGS: [&str; 22] = [
    "admin", "api", "login", "wp-admin", "wp-json", "shop", "school",
    "members", "groups", "activity", "builder", "sitemap", "sitemap.xml",
    "sitemap-index.xml", "sitemap-images.xml", "sitemap-videos.xml",
    "robots.txt", "llms.txt", "llms-full.txt", "feed", "search", "favicon.ico",
];

/// A `wl_sites` row.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Site {
    pub id: i64,
    pub slug: String,
    pub domain: String,
    pub title: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

pub fn row_to_site(row: &Row) -> Site {
    Site {
        id: row_int(row, "id"),
        slug: row_text(row, "slug"),
        domain: row_text(row, "domain"),
        title: row_text(row, "title"),
        status: row_text(row, "status"),
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

/// Validate a site slug: lowercase letters, digits and hyphens,
/// 1–63 chars, not reserved, not starting/ending with a hyphen.
pub fn validate_slug(raw: &str) -> Result<String, WordlessError> {
    let slug = raw.trim().to_ascii_lowercase();
    if slug.is_empty() || slug.len() > 63 {
        return Err(bad_request("Site slug must be 1-63 characters."));
    }
    if slug.starts_with('-') || slug.ends_with('-') {
        return Err(bad_request("Site slug must not start or end with a hyphen."));
    }
    if !slug
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(bad_request(
            "Site slug may only contain lowercase letters, digits and hyphens.",
        ));
    }
    if RESERVED_SLUGS.contains(&slug.as_str()) {
        return Err(bad_request(&format!(
            "\"{slug}\" is a reserved word and cannot be a site slug."
        )));
    }
    Ok(slug)
}

/// The outcome of resolving a request to a site.
#[derive(Debug, Clone)]
pub struct SiteResolution {
    /// The resolved site id (1 when nothing matched).
    pub site_id: i64,
    /// The resolved site slug ("" for the main site).
    pub slug: String,
    /// The resolved site status ("public" / "archived" / "deleted").
    /// "public" for the main site.
    pub status: String,
    /// The request path with any `/<site-slug>` prefix stripped, so
    /// the router sees the same path it would for the main site.
    /// Unchanged when the site came from a subdomain or header.
    pub stripped_path: String,
}

fn main_site(path: &str) -> SiteResolution {
    SiteResolution {
        site_id: MAIN_SITE_ID,
        slug: String::new(),
        status: "public".to_string(),
        stripped_path: path.to_string(),
    }
}

/// Look a site up by id (any status — callers decide what to do with
/// archived/deleted rows).
pub async fn get_site_by_id(db: &dyn Db, id: i64) -> Result<Option<Site>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_sites WHERE id = ? LIMIT 1")
        .bind(vec![Param::Integer(id)])
        .first::<Row>()
        .await?;
    Ok(row.map(|r| row_to_site(&r)))
}

/// Look a site up by slug (any status).
pub async fn get_site_by_slug(db: &dyn Db, slug: &str) -> Result<Option<Site>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_sites WHERE slug = ? LIMIT 1")
        .bind(vec![Param::Text(slug.to_string())])
        .first::<Row>()
        .await?;
    Ok(row.map(|r| row_to_site(&r)))
}

pub async fn list_sites(db: &dyn Db) -> Result<Vec<Site>, WordlessError> {
    let rows: Vec<Row> = db
        .prepare("SELECT * FROM wl_sites ORDER BY id ASC")
        .all::<Row>()
        .await?;
    Ok(rows.iter().map(row_to_site).collect())
}

/// Resolve a request to a site.
///
/// Order (recorded in the P12 outcome block):
/// 1. `header_site` (`X-Wordless-Site: <id|slug>`) — only when the
///    caller has already authenticated an editor; path is NOT
///    stripped, so the admin API keeps its `/api/...` paths.
/// 2. Subdomain — `Host: <slug>.<network_domain>` when
///    `network_domain` is configured (env `WORDLESS_NETWORK_DOMAIN`
///    or the `network_domain` option). `www` never resolves a site.
/// 3. Path prefix — the first path segment matches a site slug;
///    the prefix is stripped from the returned path.
/// 4. Default: the main site (id 1).
///
/// Archived sites resolve (their content is still addressable by an
/// admin) but public rendering refuses them — enforced in the
/// worker entry, not here, so the API can still administer them.
pub async fn resolve_site(
    db: &dyn Db,
    host: &str,
    path: &str,
    network_domain: &str,
    header_site: Option<&str>,
) -> Result<SiteResolution, WordlessError> {
    // 1. Admin header (id or slug).
    if let Some(raw) = header_site {
        let raw = raw.trim();
        if !raw.is_empty() {
            let found = if let Ok(id) = raw.parse::<i64>() {
                get_site_by_id(db, id).await?
            } else {
                get_site_by_slug(db, raw).await?
            };
            if let Some(site) = found {
                if site.status == "public" || site.status == "archived" {
                    return Ok(SiteResolution {
                        site_id: site.id,
                        slug: site.slug,
                        status: site.status.clone(),
                        stripped_path: path.to_string(),
                    });
                }
            }
            // Unknown/deleted header site: fall through to default
            // rather than failing the request — the switcher only
            // ever sends sites the admin listed.
        }
    }

    // 2. Subdomain: <slug>.<network_domain>.
    let network_domain = network_domain.trim().trim_start_matches('.').to_ascii_lowercase();
    if !network_domain.is_empty() {
        let host = host.split(':').next().unwrap_or("").to_ascii_lowercase();
        if let Some(label) = host.strip_suffix(&format!(".{network_domain}")) {
            if !label.is_empty() && label != "www" && !label.contains('.') {
                if let Some(site) = get_site_by_slug(db, label).await? {
                    if site.status == "public" || site.status == "archived" {
                        return Ok(SiteResolution {
                            site_id: site.id,
                            slug: site.slug,
                            status: site.status.clone(),
                            stripped_path: path.to_string(),
                        });
                    }
                }
            }
        }
    }

    // 3. Path prefix: /<site-slug>/... — never for /api or /admin,
    // which are network-level, and never for a reserved word.
    if !path.starts_with("/api") && !path.starts_with("/admin") {
        let first = path
            .trim_start_matches('/')
            .split('/')
            .next()
            .unwrap_or("");
        if !first.is_empty() && !RESERVED_SLUGS.contains(&first) {
            if let Some(site) = get_site_by_slug(db, first).await? {
                if site.status == "public" || site.status == "archived" {
                    // `path[first.len() + 1..]` is the remainder after
                    // "/<slug>": it starts with '/' ("/hello") or is
                    // empty ("/news" exactly). Never prefix another '/'.
                    let rest = &path[first.len() + 1..];
                    let stripped = if rest.is_empty() {
                        "/".to_string()
                    } else {
                        rest.to_string()
                    };
                    // The prefix must be a whole segment: "/news/x"
                    // strips to "/x"; "/news" strips to "/".
                    return Ok(SiteResolution {
                        site_id: site.id,
                        slug: site.slug,
                        status: site.status.clone(),
                        stripped_path: stripped,
                    });
                }
            }
        }
    }

    // 4. Default.
    Ok(main_site(path))
}

// ---------------------------------------------------------------- CRUD

pub async fn create_site(db: &dyn Db, raw_slug: &str, title: &str) -> Result<Site, WordlessError> {
    let slug = validate_slug(raw_slug)?;
    let title = title.trim();
    if title.is_empty() || title.len() > 191 {
        return Err(bad_request("Site title must be 1-191 characters."));
    }
    if get_site_by_slug(db, &slug).await?.is_some() {
        return Err(bad_request(&format!(
            "A site with slug \"{slug}\" already exists."
        )));
    }
    let now = now_iso();
    let meta = db
        .prepare(
            "INSERT INTO wl_sites (slug, domain, title, status, created_at, updated_at) \
             VALUES (?, '', ?, 'public', ?, ?)",
        )
        .bind(vec![
            Param::Text(slug),
            Param::Text(title.to_string()),
            Param::Text(now.clone()),
            Param::Text(now),
        ])
        .run()
        .await?;
    let id = meta.last_row_id.ok_or_else(|| {
        WordlessError::new("internal_error", 500, "Site insert returned no id.")
    })?;
    get_site_by_id(db, id)
        .await?
        .ok_or_else(|| WordlessError::new("internal_error", 500, "Site vanished after insert."))
}

pub async fn update_site(
    db: &dyn Db,
    id: i64,
    title: Option<&str>,
    status: Option<&str>,
) -> Result<Site, WordlessError> {
    let site = get_site_by_id(db, id)
        .await?
        .ok_or_else(|| not_found("Site not found."))?;
    let new_title = match title {
        Some(t) => {
            let t = t.trim();
            if t.is_empty() || t.len() > 191 {
                return Err(bad_request("Site title must be 1-191 characters."));
            }
            t.to_string()
        }
        None => site.title,
    };
    let new_status = match status {
        Some(s) => match s {
            "public" | "archived" => s.to_string(),
            _ => {
                return Err(bad_request(
                    "Site status must be \"public\" or \"archived\".",
                ))
            }
        },
        None => site.status,
    };
    db.prepare("UPDATE wl_sites SET title = ?, status = ?, updated_at = ? WHERE id = ?")
        .bind(vec![
            Param::Text(new_title),
            Param::Text(new_status),
            Param::Text(now_iso()),
            Param::Integer(id),
        ])
        .run()
        .await?;
    get_site_by_id(db, id)
        .await?
        .ok_or_else(|| WordlessError::new("internal_error", 500, "Site vanished after update."))
}

/// Delete a site and ALL of its content rows (posts, postmeta, terms,
/// taxonomies, relationships, comments, commentmeta, redirects, site
/// options). The main site (id 1) can never be deleted. Returns the
/// number of post rows removed, as the visible measure of the wipe.
pub async fn delete_site(db: &dyn Db, id: i64) -> Result<i64, WordlessError> {
    if id == MAIN_SITE_ID {
        return Err(bad_request("The main site cannot be deleted."));
    }
    let site = get_site_by_id(db, id)
        .await?
        .ok_or_else(|| not_found("Site not found."))?;
    let _ = site;
    let tables = [
        "wp_posts",
        "wp_postmeta",
        "wp_terms",
        "wp_term_taxonomy",
        "wp_term_relationships",
        "wp_comments",
        "wp_commentmeta",
        "wl_redirects",
        "wl_site_options",
    ];
    let mut posts_removed = 0i64;
    for table in tables {
        let meta = db
            .prepare(&format!("DELETE FROM {table} WHERE site_id = ?"))
            .bind(vec![Param::Integer(id)])
            .run()
            .await?;
        if table == "wp_posts" {
            posts_removed = meta.changes as i64;
        }
    }
    db.prepare("DELETE FROM wl_sites WHERE id = ?")
        .bind(vec![Param::Integer(id)])
        .run()
        .await?;
    Ok(posts_removed)
}

// ------------------------------------------------------- per-site options

/// Read a site-scoped option. Site 1 reads `wp_options` exactly as
/// before; other sites read `wl_site_options` first and fall back to
/// `wp_options` when unset (so a new site inherits the network
/// defaults until an admin changes one of its settings).
pub async fn get_site_option<T: DeserializeOwned>(
    db: &dyn Db,
    site_id: i64,
    key: &str,
    default: T,
) -> Result<T, WordlessError> {
    if site_id != MAIN_SITE_ID {
        let row: Option<Row> = db
            .query_one(
                "SELECT option_value FROM wl_site_options WHERE site_id = ? AND option_name = ? LIMIT 1",
                &[Param::Integer(site_id), Param::Text(key.to_string())],
            )
            .await?;
        if let Some(row) = row {
            let raw = row_text(&row, "option_value");
            if let Ok(v) = serde_json::from_str::<T>(&raw) {
                return Ok(v);
            }
        }
    }
    // Network default from wp_options. Uses the `query_one` primitive
    // (not `wordless_db::options::get_option`, which is `prepare`-based)
    // so in-memory test doubles that implement only the primitives keep
    // working. Same semantics: missing row or bad JSON yields `default`.
    let row: Option<Row> = db
        .query_one(
            "SELECT option_value FROM wp_options WHERE option_name = ? LIMIT 1",
            &[Param::Text(key.to_string())],
        )
        .await?;
    let Some(row) = row else {
        return Ok(default);
    };
    let raw = row_text(&row, "option_value");
    match serde_json::from_str::<T>(&raw) {
        Ok(v) => Ok(v),
        Err(_) => Ok(default),
    }
}

/// Write a site-scoped option. Site 1 writes `wp_options`; other
/// sites write `wl_site_options`.
pub async fn set_site_option<T: Serialize>(
    db: &dyn Db,
    site_id: i64,
    key: &str,
    value: &T,
) -> Result<(), WordlessError> {
    let json = serde_json::to_string(value)
        .map_err(|e| WordlessError::new("internal_error", 500, &e.to_string()))?;
    if site_id == MAIN_SITE_ID {
        // `exec` primitive, not the `prepare`-based options helper, for
        // the same test-double reason as `get_site_option` above.
        db.exec(
            "INSERT INTO wp_options (option_name, option_value, autoload) VALUES (?, ?, 'yes') \
             ON CONFLICT(option_name) DO UPDATE SET option_value = excluded.option_value",
            &[Param::Text(key.to_string()), Param::Text(json)],
        )
        .await?;
        return Ok(());
    }
    db.exec(
        "INSERT INTO wl_site_options (site_id, option_name, option_value) VALUES (?, ?, ?) \
         ON CONFLICT(site_id, option_name) DO UPDATE SET option_value = excluded.option_value",
        &[
            Param::Integer(site_id),
            Param::Text(key.to_string()),
            Param::Text(json),
        ],
    )
    .await?;
    Ok(())
}

/// Delete a site-scoped option. Site 1 deletes from `wp_options`;
/// other sites delete from `wl_site_options` (the value then falls
/// back to the network default on read). Returns true when a row
/// was removed.
pub async fn delete_site_option(
    db: &dyn Db,
    site_id: i64,
    key: &str,
) -> Result<bool, WordlessError> {
    // Use the `exec` primitive (not `prepare`): some in-memory test
    // doubles implement only the primitive query methods.
    let meta = if site_id == MAIN_SITE_ID {
        db.exec(
            "DELETE FROM wp_options WHERE option_name = ?",
            &[Param::Text(key.to_string())],
        )
        .await?
    } else {
        db.exec(
            "DELETE FROM wl_site_options WHERE site_id = ? AND option_name = ?",
            &[Param::Integer(site_id), Param::Text(key.to_string())],
        )
        .await?
    };
    Ok(meta.changes > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_executor::block_on;
    use rusqlite::Connection;
    use std::cell::RefCell;
    use wordless_db::{DbError, RunMeta};

    struct MemDb {
        conn: RefCell<Connection>,
    }

    impl MemDb {
        fn new() -> Self {
            let conn = Connection::open_in_memory().unwrap();
            // P12: apply every migration in order, exactly as D1
            // does — 0018 touches tables created as late as 0016
            // (wl_seo_audit_runs) and 0007 (wl_404_log).
            for (name, sql) in [
                ("0001", include_str!("../../../migrations/0001_init.sql")),
                ("0002", include_str!("../../../migrations/0002_m2.sql")),
                ("0005", include_str!("../../../migrations/0005_m5.sql")),
                ("0006", include_str!("../../../migrations/0006_m6.sql")),
                ("0007", include_str!("../../../migrations/0007_m7.sql")),
                ("0008", include_str!("../../../migrations/0008_m8.sql")),
                ("0009", include_str!("../../../migrations/0009_m9.sql")),
                ("0010", include_str!("../../../migrations/0010_m11.sql")),
                ("0011", include_str!("../../../migrations/0011_p3.sql")),
                ("0012", include_str!("../../../migrations/0012_p6.sql")),
                ("0013", include_str!("../../../migrations/0013_p7.sql")),
                ("0014", include_str!("../../../migrations/0014_p8.sql")),
                ("0015", include_str!("../../../migrations/0015_p9.sql")),
                ("0016", include_str!("../../../migrations/0016_p10.sql")),
                ("0017", include_str!("../../../migrations/0017_p11.sql")),
                ("0018", include_str!("../../../migrations/0018_p12.sql")),
            ] {
                conn.execute_batch(sql)
                    .unwrap_or_else(|e| panic!("migration {name}: {e}"));
            }
            Self {
                conn: RefCell::new(conn),
            }
        }
    }

    #[async_trait::async_trait(?Send)]
    impl Db for MemDb {
        fn prepare(&self, sql: &str) -> wordless_db::Statement<'_> {
            wordless_db::Statement::new(self, sql.to_string())
        }
        async fn query_all(&self, sql: &str, params: &[Param]) -> Result<Vec<Row>, DbError> {
            let conn = self.conn.borrow();
            let mut stmt = conn.prepare(sql).map_err(|e| DbError::D1(e.to_string()))?;
            let names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
            let mut rows = stmt
                .query(rusqlite::params_from_iter(params.iter().map(|p| match p {
                    Param::Null => rusqlite::types::Value::Null,
                    Param::Integer(i) => rusqlite::types::Value::Integer(*i),
                    Param::Real(f) => rusqlite::types::Value::Real(*f),
                    Param::Text(s) => rusqlite::types::Value::Text(s.clone()),
                    Param::Blob(b) => rusqlite::types::Value::Blob(b.clone()),
                })))
                .map_err(|e| DbError::D1(e.to_string()))?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().map_err(|e| DbError::D1(e.to_string()))? {
                let mut obj = serde_json::Map::new();
                for (i, name) in names.iter().enumerate() {
                    let v: rusqlite::types::Value =
                        row.get(i).map_err(|e| DbError::D1(e.to_string()))?;
                    obj.insert(
                        name.clone(),
                        match v {
                            rusqlite::types::Value::Null => serde_json::Value::Null,
                            rusqlite::types::Value::Integer(i) => serde_json::json!(i),
                            rusqlite::types::Value::Real(f) => {
                                serde_json::Number::from_f64(f)
                                    .map(serde_json::Value::Number)
                                    .unwrap_or(serde_json::Value::Null)
                            }
                            rusqlite::types::Value::Text(s) => serde_json::json!(s),
                            rusqlite::types::Value::Blob(_) => serde_json::Value::Null,
                        },
                    );
                }
                out.push(serde_json::Value::Object(obj));
            }
            Ok(out)
        }
        async fn query_one(&self, sql: &str, params: &[Param]) -> Result<Option<Row>, DbError> {
            let rows = self.query_all(sql, params).await?;
            Ok(rows.into_iter().next())
        }
        async fn exec(&self, sql: &str, params: &[Param]) -> Result<RunMeta, DbError> {
            let conn = self.conn.borrow();
            let changes = conn
                .execute(
                    sql,
                    rusqlite::params_from_iter(params.iter().map(|p| match p {
                        Param::Null => rusqlite::types::Value::Null,
                        Param::Integer(i) => rusqlite::types::Value::Integer(*i),
                        Param::Real(f) => rusqlite::types::Value::Real(*f),
                        Param::Text(s) => rusqlite::types::Value::Text(s.clone()),
                        Param::Blob(b) => rusqlite::types::Value::Blob(b.clone()),
                    })),
                )
                .map_err(|e| DbError::D1(e.to_string()))?;
            Ok(RunMeta {
                last_row_id: Some(conn.last_insert_rowid()),
                changes: changes as u64,
            })
        }
        async fn batch(&self, stmts: &[(String, Vec<Param>)]) -> Result<(), DbError> {
            for (sql, params) in stmts {
                self.exec(sql, params).await?;
            }
            Ok(())
        }
    }

    #[test]
    fn main_site_seeded_by_migration() {
        let db = MemDb::new();
        let site = block_on(get_site_by_id(&db, 1)).unwrap().unwrap();
        assert_eq!(site.slug, "");
        assert_eq!(site.status, "public");
    }

    #[test]
    fn slug_validation_rejects_reserved_and_bad() {
        assert!(validate_slug("admin").is_err());
        assert!(validate_slug("api").is_err());
        assert!(validate_slug("My Site").is_err());
        assert!(validate_slug("-news").is_err());
        assert!(validate_slug("news-").is_err());
        assert!(validate_slug("").is_err());
        assert_eq!(validate_slug("  News-2 ").unwrap(), "news-2");
    }

    #[test]
    fn create_list_update_delete_site() {
        let db = MemDb::new();
        let site = block_on(create_site(&db, "news", "News Site")).unwrap();
        assert_eq!(site.id, 2);
        assert_eq!(site.slug, "news");
        assert!(block_on(create_site(&db, "news", "Dup")).is_err());
        let sites = block_on(list_sites(&db)).unwrap();
        assert_eq!(sites.len(), 2);
        let updated = block_on(update_site(&db, 2, Some("News Daily"), None)).unwrap();
        assert_eq!(updated.title, "News Daily");
        assert!(block_on(delete_site(&db, 1)).is_err());
        let removed = block_on(delete_site(&db, 2)).unwrap();
        assert_eq!(removed, 0);
        assert_eq!(block_on(list_sites(&db)).unwrap().len(), 1);
    }

    #[test]
    fn resolve_path_prefix_strips_and_subdomain_works() {
        let db = MemDb::new();
        block_on(create_site(&db, "news", "News")).unwrap();
        // Path-based.
        let r = block_on(resolve_site(&db, "example.com", "/news/hello-world", "", None)).unwrap();
        assert_eq!(r.site_id, 2);
        assert_eq!(r.stripped_path, "/hello-world");
        let r = block_on(resolve_site(&db, "example.com", "/news", "", None)).unwrap();
        assert_eq!(r.stripped_path, "/");
        // Main site untouched.
        let r = block_on(resolve_site(&db, "example.com", "/hello-world", "", None)).unwrap();
        assert_eq!(r.site_id, 1);
        assert_eq!(r.stripped_path, "/hello-world");
        // /api never strips.
        let r = block_on(resolve_site(&db, "example.com", "/api/posts", "", None)).unwrap();
        assert_eq!(r.site_id, 1);
        // Subdomain.
        let r = block_on(resolve_site(&db, "news.example.com", "/", "example.com", None)).unwrap();
        assert_eq!(r.site_id, 2);
        // www never resolves.
        let r = block_on(resolve_site(&db, "www.example.com", "/", "example.com", None)).unwrap();
        assert_eq!(r.site_id, 1);
        // Header (admin).
        let r = block_on(resolve_site(&db, "example.com", "/api/posts", "", Some("news"))).unwrap();
        assert_eq!(r.site_id, 2);
        assert_eq!(r.stripped_path, "/api/posts");
    }

    #[test]
    fn site_options_fall_back_and_isolate() {
        let db = MemDb::new();
        block_on(create_site(&db, "news", "News")).unwrap();
        // Network default visible to the new site until overridden.
        block_on(wordless_db::options::set_option(
            &db,
            "blogname",
            &"Network Blog".to_string(),
        ))
        .unwrap();
        let v: String = block_on(get_site_option(&db, 2, "blogname", "x".to_string())).unwrap();
        assert_eq!(v, "Network Blog");
        block_on(set_site_option(&db, 2, "blogname", &"News Daily".to_string())).unwrap();
        let v: String = block_on(get_site_option(&db, 2, "blogname", "x".to_string())).unwrap();
        assert_eq!(v, "News Daily");
        // Site 1 untouched by the site-2 write.
        let v: String = block_on(get_site_option(&db, 1, "blogname", "x".to_string())).unwrap();
        assert_eq!(v, "Network Blog");
    }

    #[test]
    fn delete_site_wipes_its_content() {
        let db = MemDb::new();
        block_on(create_site(&db, "news", "News")).unwrap();
        block_on(
            db.prepare("INSERT INTO wp_posts (post_title, post_name, post_status, post_type, site_id) VALUES ('t', 't', 'publish', 'post', 2)")
                .run(),
        )
        .unwrap();
        let removed = block_on(delete_site(&db, 2)).unwrap();
        assert_eq!(removed, 1);
        let rows: Vec<Row> = block_on(
            db.prepare("SELECT * FROM wp_posts WHERE site_id = 2")
                .all::<Row>(),
        )
        .unwrap();
        assert!(rows.is_empty());
    }
}
