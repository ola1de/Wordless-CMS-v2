//! Site Health — real checks, no fake "all good".
//! (Port of v1 `src/modules/health/service.ts` + the performance
//! section from `src/modules/performance/service.ts`.)
//!
//! NOTE: this module lives in the worker crate for P1 only. In P2 it
//! moves to `wordless-cms` with the rest of the health module.

use serde::Serialize;
use wordless_db::row::row_int;
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::WordlessError;
use wordless_kernel::storage::ObjectStore;

use crate::context::Bindings;

#[derive(Debug, Clone, Serialize)]
pub struct HealthCheck {
    pub name: &'static str,
    pub status: &'static str,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SiteHealth {
    pub status: &'static str,
    pub version: String,
    pub checks: Vec<HealthCheck>,
}

/// The 27 tables v1 requires (same list, same order — counted from
/// `src/modules/health/service.ts`).
pub const REQUIRED_TABLES: [&str; 27] = [
    "wp_posts",
    "wp_postmeta",
    "wp_users",
    "wp_usermeta",
    "wp_terms",
    "wp_term_taxonomy",
    "wp_term_relationships",
    "wp_comments",
    "wp_commentmeta",
    "wp_options",
    "wp_termmeta",
    "wl_sessions",
    "wl_audit_log",
    "wl_redirects",
    "wl_themes",
    "wl_ai_approvals",
    "wl_ai_memory",
    "wl_404_log",
    "wl_products",
    "wl_orders",
    "wl_order_items",
    "wl_carts",
    "wl_mail_log",
    "wl_webhook_log",
    "wl_login_attempts",
    "wl_backups",
    "wl_media_derivatives",
];

/// Tables counted for the performance section (same as v1).
pub const COUNTED_TABLES: [&str; 10] = [
    "wp_posts",
    "wp_users",
    "wp_comments",
    "wp_terms",
    "wp_options",
    "wl_products",
    "wl_orders",
    "wl_carts",
    "wl_media_derivatives",
    "wl_backups",
];

pub const PUBLIC_CACHE_MAX_AGE: u64 = 60;
pub const PUBLIC_CACHE_STALE_WHILE_REVALIDATE: u64 = 60;
pub const DERIVATIVE_WIDTHS: [u32; 4] = [320, 640, 1024, 1600];

async fn count(db: &dyn Db, sql: &str) -> Result<i64, WordlessError> {
    let row: Option<Row> = db.prepare(sql).bind(vec![]).first::<Row>().await?;
    Ok(row.as_ref().map(|r| row_int(r, "total")).unwrap_or(0))
}

pub async fn site_health(
    db: &dyn Db,
    media: Option<&dyn ObjectStore>,
    bindings: &Bindings,
    version: &str,
) -> SiteHealth {
    let mut checks: Vec<HealthCheck> = Vec::new();

    // 1. Database reachable.
    match count(db, "SELECT 1 AS total").await {
        Ok(_) => checks.push(HealthCheck {
            name: "database",
            status: "pass",
            detail: "D1 responded to SELECT 1.".to_string(),
        }),
        Err(e) => checks.push(HealthCheck {
            name: "database",
            status: "fail",
            detail: format!("D1 query failed: {e}"),
        }),
    }

    // 2. Required tables present (migrations applied).
    match tables_check(db).await {
        Ok(missing) => {
            if missing.is_empty() {
                checks.push(HealthCheck {
                    name: "tables",
                    status: "pass",
                    detail: format!("{} required tables present.", REQUIRED_TABLES.len()),
                });
            } else {
                checks.push(HealthCheck {
                    name: "tables",
                    status: "fail",
                    detail: format!("Missing tables: {}", missing.join(", ")),
                });
            }
        }
        Err(e) => checks.push(HealthCheck {
            name: "tables",
            status: "fail",
            detail: format!("Table check failed: {e}"),
        }),
    }

    // 3. Bindings present (DB / MEDIA / CONFIG declared in wrangler.toml).
    let missing: Vec<&str> = [
        ("DB", bindings.db),
        ("MEDIA", bindings.media),
        ("CONFIG", bindings.config),
    ]
    .iter()
    .filter(|(_, present)| !*present)
    .map(|(name, _)| *name)
    .collect();
    if missing.is_empty() {
        checks.push(HealthCheck {
            name: "bindings",
            status: "pass",
            detail: "DB, MEDIA and CONFIG bindings are present.".to_string(),
        });
    } else {
        checks.push(HealthCheck {
            name: "bindings",
            status: "fail",
            detail: format!("Missing bindings: {}", missing.join(", ")),
        });
    }

    // 4. Content counts (real numbers, not placeholders).
    match content_check(db).await {
        Ok(detail) => checks.push(HealthCheck {
            name: "content",
            status: "pass",
            detail,
        }),
        Err(e) => checks.push(HealthCheck {
            name: "content",
            status: "fail",
            detail: format!("Count queries failed: {e}"),
        }),
    }

    // 5. Runtime version (informational; always passes when reached).
    checks.push(HealthCheck {
        name: "runtime",
        status: "pass",
        detail: format!("Wordless {version}; D1/SQLite backend."),
    });

    // 6. Performance — measured numbers, never invented.
    match performance_check(db, media).await {
        Ok(detail) => checks.push(HealthCheck {
            name: "performance",
            status: "pass",
            detail,
        }),
        Err(e) => checks.push(HealthCheck {
            name: "performance",
            status: "fail",
            detail: format!("Performance section failed: {e}"),
        }),
    }

    let status = if checks.iter().any(|c| c.status == "fail") {
        "degraded"
    } else {
        "ok"
    };
    SiteHealth {
        status,
        version: version.to_string(),
        checks,
    }
}

async fn tables_check(db: &dyn Db) -> Result<Vec<String>, WordlessError> {
    let mut missing = Vec::new();
    for table in REQUIRED_TABLES {
        let row: Option<Row> = db
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ? LIMIT 1")
            .bind(vec![Param::Text(table.to_string())])
            .first::<Row>()
            .await?;
        if row.is_none() {
            missing.push(table.to_string());
        }
    }
    Ok(missing)
}

async fn content_check(db: &dyn Db) -> Result<String, WordlessError> {
    let posts = count(
        db,
        "SELECT COUNT(*) AS total FROM wp_posts WHERE post_type IN ('post', 'page') AND post_status != 'trash'",
    )
    .await?;
    let users = count(db, "SELECT COUNT(*) AS total FROM wp_users").await?;
    let comments = count(
        db,
        "SELECT COUNT(*) AS total FROM wp_comments WHERE comment_approved = '1'",
    )
    .await?;
    Ok(format!(
        "{posts} posts/pages, {users} users, {comments} approved comments."
    ))
}

async fn performance_check(
    db: &dyn Db,
    media: Option<&dyn ObjectStore>,
) -> Result<String, WordlessError> {
    let mut tables: Vec<(String, i64)> = Vec::new();
    for table in COUNTED_TABLES {
        let n = count(db, &format!("SELECT COUNT(*) AS total FROM {table}")).await?;
        tables.push((table.to_string(), n));
    }
    let get = |name: &str| -> i64 {
        tables
            .iter()
            .find(|(t, _)| t == name)
            .map(|(_, n)| *n)
            .unwrap_or(0)
    };

    let (media_counted, capped) = match media {
        Some(store) => store.count_objects(1000).await?,
        None => (0, false),
    };

    let derivatives_stored =
        count(db, "SELECT COUNT(*) AS total FROM wl_media_derivatives").await?;
    let detail = format!(
        "{} posts, {} users, {} comments, {} products, {} orders; R2: {} object(s){}; public cache: anonymous GETs {}s + stale-while-revalidate {}s; image derivatives: Images NOT bound — originals served ({} stored); backups on record: {}.",
        get("wp_posts"),
        get("wp_users"),
        get("wp_comments"),
        get("wl_products"),
        get("wl_orders"),
        media_counted,
        if capped { " (count capped at 1,000)" } else { "" },
        PUBLIC_CACHE_MAX_AGE,
        PUBLIC_CACHE_STALE_WHILE_REVALIDATE,
        derivatives_stored,
        get("wl_backups"),
    );
    Ok(detail)
}
