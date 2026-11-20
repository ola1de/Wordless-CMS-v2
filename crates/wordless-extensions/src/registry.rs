//! Extension registry — D1 tables for P6 (`wl_extension_instances`,
//! `wl_extension_runs`, `wl_extension_logs`, migration `0012_p6.sql`).
//!
//! The registry is the kill switch: [`InstanceStatus::Disabled`]
//! instances are never dispatched, and the status is re-checked on
//! every invocation (disabling stops hooks within one request).
//!
//! Install metadata stays in `wl_extensions` (P3) — its `report_json`
//! is the load gate's truth. This registry records only runtime state:
//! what is loaded, with which permissions, under which host token, and
//! what happened when it ran.

use serde::{Deserialize, Serialize};
use wordless_db::{
    row::{row_int, row_opt_text, row_text},
    Db, Param, Row,
};
use wordless_kernel::{error::WordlessError, time::now_iso};

use crate::capabilities::Capability;

/// Runtime status of a loaded extension instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstanceStatus {
    /// Loaded: dispatchable (subject to the per-call capability checks).
    Loaded,
    /// Disabled by an admin: never dispatched. The kill switch.
    Disabled,
}

impl InstanceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            InstanceStatus::Loaded => "loaded",
            InstanceStatus::Disabled => "disabled",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "loaded" => Some(InstanceStatus::Loaded),
            "disabled" => Some(InstanceStatus::Disabled),
            _ => None,
        }
    }
}

/// One loaded extension instance (one row per slug).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionInstance {
    pub slug: String,
    pub version: String,
    pub status: String,
    /// Granted capability ids (`permissions_json` in D1).
    pub permissions: Vec<String>,
    /// R2 key of the generated extension Worker project entry
    /// (`extensions/<slug>/worker/worker-entry.ts`).
    pub script_key: String,
    /// SHA-256 hex of the per-load host bearer token. Never the token.
    pub host_token_hash: String,
    /// Token epoch: incremented on every (re)load, so a reload rotates
    /// the derived host token (see [`crate::host::derive_host_token`]).
    pub token_epoch: i64,
    /// Local-dev override: base URL of a locally running extension
    /// Worker (service-binding / HTTP executor). Null in production.
    pub dev_url: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// One hook invocation (or attempted invocation) of an extension.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionRun {
    pub id: i64,
    pub slug: String,
    pub hook: String,
    /// `ok` | `error` | `timeout` | `denied` | `skipped_disabled`.
    pub status: String,
    pub duration_ms: i64,
    pub output_summary: Option<String>,
    pub error: Option<String>,
    pub created_at: String,
}

/// One log line from (or about) an extension.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionLog {
    pub id: i64,
    pub slug: String,
    /// `info` | `warn` | `error` | `denied`.
    pub level: String,
    pub message: String,
    pub created_at: String,
}

fn instance_from_row(row: &Row) -> ExtensionInstance {
    let permissions: Vec<String> =
        serde_json::from_str(&row_opt_text(row, "permissions_json").unwrap_or_default())
            .unwrap_or_default();
    ExtensionInstance {
        slug: row_text(row, "slug"),
        version: row_text(row, "version"),
        status: row_text(row, "status"),
        permissions,
        script_key: row_text(row, "script_key"),
        host_token_hash: row_text(row, "host_token_hash"),
        token_epoch: row_int(row, "token_epoch"),
        dev_url: row_opt_text(row, "dev_url"),
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

/// Record (or re-record) a loaded instance. Re-loading rotates the
/// host token (the epoch increments, so [`crate::host::derive_host_token`]
/// yields a new token) and replaces the granted permissions.
#[allow(clippy::too_many_arguments)]
pub async fn record_instance(
    db: &dyn Db,
    slug: &str,
    version: &str,
    permissions: &[Capability],
    script_key: &str,
    host_token_hash: &str,
    dev_url: Option<&str>,
) -> Result<ExtensionInstance, WordlessError> {
    let now = now_iso();
    let permissions_json =
        serde_json::to_string(&permissions.iter().map(|c| c.id()).collect::<Vec<_>>())
            .map_err(|e| WordlessError::new("internal_error", 500, &e.to_string()))?;
    db.exec(
        "INSERT INTO wl_extension_instances (slug, version, status, permissions_json, script_key, host_token_hash, token_epoch, dev_url, created_at, updated_at) \
         VALUES (?, ?, 'loaded', ?, ?, ?, 0, ?, ?, ?) \
         ON CONFLICT(slug) DO UPDATE SET version = excluded.version, status = 'loaded', \
         permissions_json = excluded.permissions_json, script_key = excluded.script_key, \
         host_token_hash = excluded.host_token_hash, \
         token_epoch = wl_extension_instances.token_epoch + 1, \
         dev_url = excluded.dev_url, updated_at = excluded.updated_at",
        &[
            Param::Text(slug.to_string()),
            Param::Text(version.to_string()),
            Param::Text(permissions_json),
            Param::Text(script_key.to_string()),
            Param::Text(host_token_hash.to_string()),
            Param::from(dev_url.map(|s| s.to_string())),
            Param::Text(now.clone()),
            Param::Text(now),
        ],
    )
    .await?;
    get_instance(db, slug).await?.ok_or_else(|| {
        WordlessError::new("internal_error", 500, "instance row missing after insert")
    })
}

pub async fn get_instance(
    db: &dyn Db,
    slug: &str,
) -> Result<Option<ExtensionInstance>, WordlessError> {
    let row = db
        .query_one(
            "SELECT * FROM wl_extension_instances WHERE slug = ? LIMIT 1",
            &[Param::Text(slug.to_string())],
        )
        .await?;
    Ok(row.as_ref().map(instance_from_row))
}

pub async fn list_instances(db: &dyn Db) -> Result<Vec<ExtensionInstance>, WordlessError> {
    let rows = db
        .query_all(
            "SELECT * FROM wl_extension_instances ORDER BY slug ASC",
            &[],
        )
        .await?;
    Ok(rows.iter().map(instance_from_row).collect())
}

/// Set an instance's status. This is the kill switch: `disabled`
/// takes effect on the very next invocation (status is re-read per
/// dispatch — see [`crate::hooks`]).
pub async fn set_instance_status(
    db: &dyn Db,
    slug: &str,
    status: InstanceStatus,
) -> Result<ExtensionInstance, WordlessError> {
    let existing = get_instance(db, slug)
        .await?
        .ok_or_else(|| WordlessError::new("not_found", 404, "extension instance not found"))?;
    db.exec(
        "UPDATE wl_extension_instances SET status = ?, updated_at = ? WHERE slug = ?",
        &[
            Param::Text(status.as_str().to_string()),
            Param::Text(now_iso()),
            Param::Text(slug.to_string()),
        ],
    )
    .await?;
    Ok(ExtensionInstance {
        status: status.as_str().to_string(),
        ..existing
    })
}

/// Replace an instance's granted permissions (admin grant/revoke).
/// Unknown capability ids are rejected — a grant that does not exist
/// cannot be honoured silently.
pub async fn set_instance_permissions(
    db: &dyn Db,
    slug: &str,
    permissions: &[Capability],
) -> Result<ExtensionInstance, WordlessError> {
    let existing = get_instance(db, slug)
        .await?
        .ok_or_else(|| WordlessError::new("not_found", 404, "extension instance not found"))?;
    let permissions_json =
        serde_json::to_string(&permissions.iter().map(|c| c.id()).collect::<Vec<_>>())
            .map_err(|e| WordlessError::new("internal_error", 500, &e.to_string()))?;
    db.exec(
        "UPDATE wl_extension_instances SET permissions_json = ?, updated_at = ? WHERE slug = ?",
        &[
            Param::Text(permissions_json),
            Param::Text(now_iso()),
            Param::Text(slug.to_string()),
        ],
    )
    .await?;
    Ok(ExtensionInstance {
        permissions: permissions.iter().map(|c| c.id().to_string()).collect(),
        ..existing
    })
}

/// Delete an instance row (unload). The extension's code stays in R2
/// and its `wl_extensions` record stays installed; loading again
/// re-runs the gate and re-derives the token.
pub async fn delete_instance(db: &dyn Db, slug: &str) -> Result<bool, WordlessError> {
    let meta = db
        .exec(
            "DELETE FROM wl_extension_instances WHERE slug = ?",
            &[Param::Text(slug.to_string())],
        )
        .await?;
    Ok(meta.changes > 0)
}

pub async fn record_run(
    db: &dyn Db,
    slug: &str,
    hook: &str,
    status: &str,
    duration_ms: i64,
    output_summary: Option<&str>,
    error: Option<&str>,
) -> Result<(), WordlessError> {
    db.exec(
        "INSERT INTO wl_extension_runs (slug, hook, status, duration_ms, output_summary, error, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        &[
            Param::Text(slug.to_string()),
            Param::Text(hook.to_string()),
            Param::Text(status.to_string()),
            Param::Integer(duration_ms),
            Param::from(output_summary.map(|s| s.to_string())),
            Param::from(error.map(|s| s.to_string())),
            Param::Text(now_iso()),
        ],
    )
    .await?;
    Ok(())
}

pub async fn list_runs(
    db: &dyn Db,
    slug: &str,
    limit: i64,
) -> Result<Vec<ExtensionRun>, WordlessError> {
    let rows = db
        .query_all(
            "SELECT * FROM wl_extension_runs WHERE slug = ? ORDER BY id DESC LIMIT ?",
            &[Param::Text(slug.to_string()), Param::Integer(limit)],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|row| ExtensionRun {
            id: row_int(row, "id"),
            slug: row_text(row, "slug"),
            hook: row_text(row, "hook"),
            status: row_text(row, "status"),
            duration_ms: row_int(row, "duration_ms"),
            output_summary: row_opt_text(row, "output_summary"),
            error: row_opt_text(row, "error"),
            created_at: row_text(row, "created_at"),
        })
        .collect())
}

pub async fn record_log(
    db: &dyn Db,
    slug: &str,
    level: &str,
    message: &str,
) -> Result<(), WordlessError> {
    db.exec(
        "INSERT INTO wl_extension_logs (slug, level, message, created_at) VALUES (?, ?, ?, ?)",
        &[
            Param::Text(slug.to_string()),
            Param::Text(level.to_string()),
            Param::Text(message.to_string()),
            Param::Text(now_iso()),
        ],
    )
    .await?;
    Ok(())
}

pub async fn list_logs(
    db: &dyn Db,
    slug: &str,
    limit: i64,
) -> Result<Vec<ExtensionLog>, WordlessError> {
    let rows = db
        .query_all(
            "SELECT * FROM wl_extension_logs WHERE slug = ? ORDER BY id DESC LIMIT ?",
            &[Param::Text(slug.to_string()), Param::Integer(limit)],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|row| ExtensionLog {
            id: row_int(row, "id"),
            slug: row_text(row, "slug"),
            level: row_text(row, "level"),
            message: row_text(row, "message"),
            created_at: row_text(row, "created_at"),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{block_on, test_db, SCHEMA_P6};

    fn db() -> impl Db {
        let db = test_db();
        block_on(async {
            db.exec(SCHEMA_P6, &[]).await.unwrap();
        });
        db
    }

    #[test]
    fn record_and_kill_switch_round_trip() {
        let db = db();
        block_on(async {
            let inst = record_instance(
                &db,
                "hello-dolly",
                "1.7.2",
                &[Capability::HooksRegister, Capability::Log],
                "extensions/hello-dolly/worker/worker-entry.ts",
                "deadbeef",
                None,
            )
            .await
            .unwrap();
            assert_eq!(inst.status, "loaded");
            assert_eq!(inst.permissions, vec!["hooks.register", "log"]);
            assert_eq!(inst.token_epoch, 0);
            // Reload rotates the epoch (and therefore the token).
            let reloaded = record_instance(
                &db,
                "hello-dolly",
                "1.7.2",
                &[Capability::HooksRegister, Capability::Log],
                "extensions/hello-dolly/worker/worker-entry.ts",
                "cafe",
                None,
            )
            .await
            .unwrap();
            assert_eq!(reloaded.token_epoch, 1);
            assert_eq!(reloaded.host_token_hash, "cafe");
            // Kill switch: disable, then re-read — the dispatch path
            // reads this row on every invocation.
            let disabled = set_instance_status(&db, "hello-dolly", InstanceStatus::Disabled)
                .await
                .unwrap();
            assert_eq!(disabled.status, "disabled");
            let again = get_instance(&db, "hello-dolly").await.unwrap().unwrap();
            assert_eq!(again.status, "disabled");
            // Grant/revoke round-trips.
            let granted = set_instance_permissions(&db, "hello-dolly", &[Capability::OptionsRead])
                .await
                .unwrap();
            assert_eq!(granted.permissions, vec!["options.read"]);
            // Runs + logs.
            record_run(
                &db,
                "hello-dolly",
                "admin_notices",
                "ok",
                42,
                Some("1 output"),
                None,
            )
            .await
            .unwrap();
            let runs = list_runs(&db, "hello-dolly", 10).await.unwrap();
            assert_eq!(runs.len(), 1);
            assert_eq!(runs[0].hook, "admin_notices");
            record_log(
                &db,
                "hello-dolly",
                "denied",
                "fetch to evil.example blocked",
            )
            .await
            .unwrap();
            let logs = list_logs(&db, "hello-dolly", 10).await.unwrap();
            assert_eq!(logs.len(), 1);
            assert_eq!(logs[0].level, "denied");
        });
    }

    #[test]
    fn unknown_instance_is_not_found() {
        let db = db();
        block_on(async {
            assert!(get_instance(&db, "nope").await.unwrap().is_none());
            assert!(set_instance_status(&db, "nope", InstanceStatus::Disabled)
                .await
                .is_err());
        });
    }
}
