//! Security Centre overview — aggregation over `wordless-auth`,
//! `wordless-cms`, and `wordless-kernel::security`.
//!
//! Port of v1 `getSecurityOverview`
//! (`src/modules/security/service.ts`) — the same shape, field for
//! field. (The P3 consolidation had extended this struct with the
//! audit log and a headers array; P5 F7 restored v1's shape, which
//! the P4 admin Security page also reads. The audit log stays
//! available at its own endpoint, `GET /api/activity`.)

use serde::Serialize;
use wordless_auth::LockoutConfig;
use wordless_auth::{get_lockout_config, is_totp_enabled, list_locked_keys, list_users, LockedKey};
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::capabilities::Role;
use wordless_kernel::error::WordlessError;
use wordless_kernel::security::CONTENT_SECURITY_POLICY;
use wordless_kernel::time::{iso_from_ms, now_ms};

/// Lockout configuration, serialised with v1's camelCase field names.
/// (`wordless-auth::LockoutConfig` has no `Serialize` impl, so the
/// fields are copied into this struct.)
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LockoutConfigView {
    pub max_attempts: i64,
    pub window_minutes: i64,
    pub lockout_minutes: i64,
}

impl From<LockoutConfig> for LockoutConfigView {
    fn from(c: LockoutConfig) -> Self {
        Self {
            max_attempts: c.max_attempts,
            window_minutes: c.window_minutes,
            lockout_minutes: c.lockout_minutes,
        }
    }
}

/// The lockout half of the overview: the effective config plus every
/// key that is currently locked.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LockoutState {
    pub config: LockoutConfigView,
    pub locked_keys: Vec<LockedKey>,
}

/// The security-headers half of the overview: v1 reports the CSP
/// policy string and whether headers are applied (always true — the
/// worker entry adds them to every response).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadersState {
    pub policy: String,
    pub applied: bool,
}

/// One administrator's 2FA coverage row (v1 lists ALL administrators,
/// not just the requesting user).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TwoFactorUser {
    pub user_id: i64,
    pub login: String,
    pub role: String,
    pub enabled: bool,
}

/// The sessions half of the overview.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionsState {
    pub active: i64,
}

/// One failed-login audit row (v1's shape: user, time, details only).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedLogin {
    pub user_id: i64,
    pub created_at: String,
    pub details: Option<String>,
}

/// The Security Centre overview, v1's `SecurityOverview` shape
/// (`src/modules/security/service.ts`) — the P4 admin Security page
/// reads exactly these fields (`overview.twoFactor`,
/// `overview.sessions.active`, `overview.headers.applied`).
///
/// P5 F7: the P3 consolidation had extended this struct with the
/// audit log (`audit`), the requesting user's TOTP flag
/// (`totp_enabled`), the flat session count (`active_sessions`) and
/// the headers as a name/value array. That shape matched neither v1
/// nor the admin SPA, so the struct is v1's shape again. The audit
/// log stays available at its own endpoint (`GET /api/activity`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityOverview {
    /// `{ policy, applied }` — the CSP policy string.
    pub headers: HeadersState,
    /// Lockout config + currently-locked keys.
    pub lockout: LockoutState,
    /// 2FA coverage for every administrator.
    pub two_factor: Vec<TwoFactorUser>,
    /// `{ active }` — unrevoked, unexpired sessions, site-wide.
    pub sessions: SessionsState,
    /// Most recent failed-login audit entries (up to 20).
    pub failed_logins: Vec<FailedLogin>,
    /// Whether a user with login `admin` still exists (v1 flags the
    /// default admin login as a posture item).
    pub default_admin_login_exists: bool,
}

/// Build the Security Centre overview at the current time.
pub async fn security_overview(db: &dyn Db) -> Result<SecurityOverview, WordlessError> {
    security_overview_at(db, now_ms()).await
}

/// Same as [`security_overview`], with the clock pinned — tests use
/// this so lockout/session expiry is deterministic.
pub async fn security_overview_at(
    db: &dyn Db,
    now_ms: u64,
) -> Result<SecurityOverview, WordlessError> {
    let config = get_lockout_config(db).await?;
    let locked_keys = list_locked_keys(db, now_ms).await?;
    // v1 lists the first 100 users, flags any login "admin", and
    // reports 2FA coverage for every administrator.
    let users = list_users(db, 1, 100).await?;
    let mut two_factor = Vec::new();
    let mut default_admin_login_exists = false;
    for u in &users.users {
        if u.login == "admin" {
            default_admin_login_exists = true;
        }
        if u.role != Role::Administrator {
            continue;
        }
        two_factor.push(TwoFactorUser {
            user_id: u.id,
            login: u.login.clone(),
            role: u.role.as_str().to_string(),
            enabled: is_totp_enabled(db, u.id).await?,
        });
    }
    let now_iso = iso_from_ms(now_ms);
    let active_row: Option<Row> = db
        .prepare("SELECT COUNT(*) AS n FROM wl_sessions WHERE revoked = 0 AND expires_at > ?")
        .bind(vec![Param::Text(now_iso)])
        .first()
        .await?;
    let active_sessions = active_row.as_ref().map_or(0, |r| row_int(r, "n"));
    let headers = HeadersState {
        policy: CONTENT_SECURITY_POLICY.to_string(),
        applied: true,
    };
    let failed_rows: Vec<Row> = db
        .prepare(
            "SELECT user_id, created_at, details FROM wl_audit_log WHERE action = 'auth.login_failed' \
             ORDER BY created_at DESC LIMIT 20",
        )
        .all()
        .await?;
    let failed_logins = failed_rows
        .iter()
        .map(|r| FailedLogin {
            user_id: row_int(r, "user_id"),
            created_at: row_text(r, "created_at"),
            details: {
                let d = row_text(r, "details");
                if d.is_empty() {
                    None
                } else {
                    Some(d)
                }
            },
        })
        .collect();
    Ok(SecurityOverview {
        headers,
        lockout: LockoutState {
            config: config.into(),
            locked_keys,
        },
        two_factor,
        sessions: SessionsState {
            active: active_sessions,
        },
        failed_logins,
        default_admin_login_exists,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SqliteDb;
    use futures_executor::block_on;
    use wordless_auth::{get_user_meta, update_user_meta};
    use wordless_db::Param;

    const NOW_MS: u64 = 1_795_089_600_000; // 2026-11-19T12:00:00Z, pinned for tests.

    fn db() -> SqliteDb {
        SqliteDb::new()
    }

    fn seed_audit(db: &SqliteDb, action: &str, user_id: i64, created_at: &str) {
        block_on(
            db.prepare(
                "INSERT INTO wl_audit_log (user_id, action, object_type, object_id, details, ip_hash, created_at) \
                 VALUES (?, ?, 'user', '1', 'test entry', NULL, ?)",
            )
            .bind(vec![
                Param::Integer(user_id),
                Param::Text(action.to_string()),
                Param::Text(created_at.to_string()),
            ])
            .run(),
        )
        .unwrap();
    }

    #[test]
    fn overview_reports_v1_shape() {
        let db = db();
        // One admin user (login "admin") with TOTP enabled, one
        // editor without.
        for (id, login, email) in [(1, "admin", "a@example.test"), (2, "ed", "ed@example.test")] {
            block_on(
                db.prepare(
                    "INSERT INTO wp_users (ID, user_login, user_pass, user_nicename, user_email, user_registered, display_name) \
                     VALUES (?, ?, '', ?, ?, '2026-01-01T00:00:00.000Z', ?)",
                )
                .bind(vec![
                    Param::Integer(id),
                    Param::Text(login.to_string()),
                    Param::Text(login.to_string()),
                    Param::Text(email.to_string()),
                    Param::Text(login.to_string()),
                ])
                .run(),
            )
            .unwrap();
        }
        // admin = administrator, ed = editor (wp_capabilities meta).
        block_on(
            db.prepare(
                "INSERT INTO wp_usermeta (user_id, meta_key, meta_value) VALUES (1, 'wp_capabilities', '{\"administrator\":true}')",
            )
            .run(),
        )
        .unwrap();
        block_on(
            db.prepare(
                "INSERT INTO wp_usermeta (user_id, meta_key, meta_value) VALUES (2, 'wp_capabilities', '{\"editor\":true}')",
            )
            .run(),
        )
        .unwrap();
        block_on(update_user_meta(
            &db,
            1,
            "wl_2fa_secret",
            &"JBSWY3DPEHPK3PXP",
        ))
        .unwrap();
        assert!(
            block_on(get_user_meta::<String>(
                &db,
                1,
                "wl_2fa_secret",
                String::new()
            ))
            .unwrap()
                == "JBSWY3DPEHPK3PXP"
        );
        // Audit: one normal entry + one failed login.
        seed_audit(&db, "post.create", 1, "2026-11-19T10:00:00.000Z");
        seed_audit(&db, "auth.login_failed", 0, "2026-11-19T11:00:00.000Z");
        // One locked key (locked until after NOW_MS) + one expired lock.
        for (key, locked_until) in [
            ("login:mallory", "2026-11-20T00:00:00.000Z"),
            ("login:stale", "2026-11-01T00:00:00.000Z"),
        ] {
            block_on(
                db.prepare(
                    "INSERT INTO wl_login_attempts (key, attempts, first_at, last_at, locked_until, lockout_count) \
                     VALUES (?, 5, '2026-11-19T09:00:00.000Z', '2026-11-19T09:00:00.000Z', ?, 1)",
                )
                .bind(vec![
                    Param::Text(key.to_string()),
                    Param::Text(locked_until.to_string()),
                ])
                .run(),
            )
            .unwrap();
        }
        // Sessions: one active, one revoked, one expired.
        for (id, revoked, expires) in [
            ("s-active", 0, "2026-11-20T00:00:00.000Z"),
            ("s-revoked", 1, "2026-11-20T00:00:00.000Z"),
            ("s-expired", 0, "2026-11-01T00:00:00.000Z"),
        ] {
            block_on(
                db.prepare(
                    "INSERT INTO wl_sessions (id, user_id, token_hash, created_at, expires_at, revoked) \
                     VALUES (?, 1, 'hash', '2026-11-19T09:00:00.000Z', ?, ?)",
                )
                .bind(vec![
                    Param::Text(id.to_string()),
                    Param::Text(expires.to_string()),
                    Param::Integer(revoked),
                ])
                .run(),
            )
            .unwrap();
        }

        let ov = block_on(security_overview_at(&db, NOW_MS)).unwrap();
        assert!(ov.headers.applied);
        assert!(ov.headers.policy.contains("frame-ancestors 'none'"));
        assert_eq!(ov.lockout.config.max_attempts, 5);
        assert_eq!(ov.lockout.config.window_minutes, 15);
        assert_eq!(ov.lockout.config.lockout_minutes, 15);
        assert_eq!(ov.lockout.locked_keys.len(), 1, "only the live lock");
        assert_eq!(ov.lockout.locked_keys[0].key, "login:mallory");
        assert_eq!(ov.two_factor.len(), 1, "only administrators listed");
        assert_eq!(ov.two_factor[0].login, "admin");
        assert_eq!(ov.two_factor[0].role, "administrator");
        assert!(ov.two_factor[0].enabled, "wl_2fa_secret present");
        assert_eq!(ov.sessions.active, 1, "only the live unrevoked session");
        assert_eq!(ov.failed_logins.len(), 1);
        assert_eq!(ov.failed_logins[0].user_id, 0);
        assert_eq!(ov.failed_logins[0].created_at, "2026-11-19T11:00:00.000Z");
        assert!(ov.default_admin_login_exists);
    }

    #[test]
    fn overview_with_no_data_reports_empty_posture() {
        let db = db();
        let ov = block_on(security_overview_at(&db, NOW_MS)).unwrap();
        assert!(ov.headers.applied);
        assert!(!ov.headers.policy.is_empty());
        assert!(ov.lockout.locked_keys.is_empty());
        assert!(ov.two_factor.is_empty());
        assert_eq!(ov.sessions.active, 0);
        assert!(ov.failed_logins.is_empty());
        assert!(!ov.default_admin_login_exists);
    }

    #[test]
    fn lockout_config_reads_wp_options_overrides() {
        let db = db();
        block_on(
            db.prepare(
                "INSERT INTO wp_options (option_name, option_value, autoload) VALUES ('wl_security_max_attempts', '3', 'yes')",
            )
            .run(),
        )
        .unwrap();
        let ov = block_on(security_overview_at(&db, NOW_MS)).unwrap();
        assert_eq!(ov.lockout.config.max_attempts, 3);
    }
}
