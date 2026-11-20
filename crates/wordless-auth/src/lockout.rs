//! Login rate limiting + lockout.
//! (Port of v1 `src/modules/security/lockout.ts` — same table
//! `wl_login_attempts`, same two keys per failed login (`login:<name>`
//! and `ip:<sha256-hex>`), same defaults, same doubling lockout, same
//! SQL, so a v1 database keeps its counters and a locked key stays
//! locked across cutover.)
//!
//! Defaults (configurable in `wp_options` as `wl_security_max_attempts`,
//! `wl_security_window_minutes`, `wl_security_lockout_minutes`):
//! 5 failed attempts inside a 15-minute window → locked for 15 minutes.
//! Each consecutive lockout doubles the lockout (15 → 30 → 60 min, capped
//! at 60). The counter resets after a successful login or when the
//! window expires without a lockout.
//!
//! Nothing here stores a raw IP address — only its SHA-256 hex.

use wordless_db::options::get_option;
use wordless_db::row::{row_int, row_opt_text, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::WordlessError;
use wordless_kernel::time::{iso_from_ms, ms_from_iso};

pub const DEFAULT_MAX_ATTEMPTS: i64 = 5;
pub const DEFAULT_WINDOW_MINUTES: i64 = 15;
pub const DEFAULT_LOCKOUT_MINUTES: i64 = 15;
/// Consecutive lockouts double, but never past 60 minutes (v1 cap).
pub const MAX_LOCKOUT_MINUTES: i64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockoutConfig {
    pub max_attempts: i64,
    pub window_minutes: i64,
    pub lockout_minutes: i64,
}

impl Default for LockoutConfig {
    fn default() -> Self {
        Self {
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            window_minutes: DEFAULT_WINDOW_MINUTES,
            lockout_minutes: DEFAULT_LOCKOUT_MINUTES,
        }
    }
}

pub async fn get_lockout_config(db: &dyn Db) -> Result<LockoutConfig, WordlessError> {
    let max = get_option::<i64>(db, "wl_security_max_attempts", DEFAULT_MAX_ATTEMPTS).await?;
    let window =
        get_option::<i64>(db, "wl_security_window_minutes", DEFAULT_WINDOW_MINUTES).await?;
    let lockout =
        get_option::<i64>(db, "wl_security_lockout_minutes", DEFAULT_LOCKOUT_MINUTES).await?;
    // v1's `num()`: only a finite number > 0 counts, else the default.
    let num = |v: i64, fallback: i64| if v > 0 { v } else { fallback };
    Ok(LockoutConfig {
        max_attempts: num(max, DEFAULT_MAX_ATTEMPTS),
        window_minutes: num(window, DEFAULT_WINDOW_MINUTES),
        lockout_minutes: num(lockout, DEFAULT_LOCKOUT_MINUTES),
    })
}

/// The stored state of one lockout key, as read from `wl_login_attempts`.
#[derive(Debug, Clone, Copy)]
pub struct AttemptState {
    pub attempts: i64,
    pub first_at_ms: u64,
    pub lockout_count: i64,
}

/// The outcome of recording one more failed login.
#[derive(Debug, Clone, Copy)]
pub struct FailedAttemptOutcome {
    pub attempts: i64,
    pub first_at_ms: u64,
    /// Set when this failure locks the key (ISO-8601 expiry as ms).
    pub locked_until_ms: Option<u64>,
    pub lockout_count: i64,
}

/// Pure v1 `recordFailedLogin` transition: given the stored state (or
/// `None` for a first failure), compute the state after one more
/// failure. Kept pure so the doubling/window logic is unit-testable
/// without a database.
pub fn next_failed_attempt(
    state: Option<AttemptState>,
    config: LockoutConfig,
    now_ms: u64,
) -> FailedAttemptOutcome {
    let Some(state) = state else {
        return FailedAttemptOutcome {
            attempts: 1,
            first_at_ms: now_ms,
            locked_until_ms: None,
            lockout_count: 0,
        };
    };
    let window_start = now_ms.saturating_sub((config.window_minutes as u64) * 60 * 1000);
    let in_window = state.first_at_ms >= window_start;
    let attempts = if in_window { state.attempts + 1 } else { 1 };
    let first_at_ms = if in_window { state.first_at_ms } else { now_ms };
    if attempts >= config.max_attempts {
        // Consecutive lockouts double the lockout, capped at 60 minutes.
        let lockout_count = state.lockout_count + 1;
        let mut minutes = config.lockout_minutes;
        for _ in 1..lockout_count {
            minutes = minutes.saturating_mul(2);
        }
        let minutes = minutes.min(MAX_LOCKOUT_MINUTES);
        FailedAttemptOutcome {
            attempts,
            first_at_ms,
            locked_until_ms: Some(now_ms + (minutes as u64) * 60 * 1000),
            lockout_count,
        }
    } else {
        FailedAttemptOutcome {
            attempts,
            first_at_ms,
            locked_until_ms: None,
            lockout_count: state.lockout_count,
        }
    }
}

async fn get_row(db: &dyn Db, key: &str) -> Result<Option<Row>, WordlessError> {
    Ok(db
        .prepare("SELECT * FROM wl_login_attempts WHERE key = ? LIMIT 1")
        .bind(vec![Param::Text(key.to_string())])
        .first::<Row>()
        .await?)
}

/// Is this key currently locked? Returns the lockout expiry (ISO-8601)
/// or `None`.
pub async fn lockout_until(
    db: &dyn Db,
    key: &str,
    now_ms: u64,
) -> Result<Option<String>, WordlessError> {
    let Some(row) = get_row(db, key).await? else {
        return Ok(None);
    };
    let Some(locked_until) = row_opt_text(&row, "locked_until") else {
        return Ok(None);
    };
    match ms_from_iso(&locked_until) {
        Some(ms) if ms > now_ms => Ok(Some(locked_until)),
        _ => Ok(None),
    }
}

/// Record a failed login for one key. Returns the lockout expiry
/// (ISO-8601) when the failure locks the key.
pub async fn record_failed_login(
    db: &dyn Db,
    key: &str,
    config: LockoutConfig,
    now_ms: u64,
) -> Result<Option<String>, WordlessError> {
    let now_iso = iso_from_ms(now_ms);
    let row = get_row(db, key).await?;
    let state = row.as_ref().map(|r| AttemptState {
        attempts: row_int(r, "attempts"),
        // A malformed `first_at` reads as 0, which is outside any real
        // window — same as v1, where `NaN >= windowStart` is false.
        first_at_ms: ms_from_iso(&row_text(r, "first_at")).unwrap_or(0),
        lockout_count: row_int(r, "lockout_count"),
    });
    let outcome = next_failed_attempt(state, config, now_ms);
    if state.is_none() {
        db.prepare(
            "INSERT INTO wl_login_attempts (key, attempts, first_at, last_at, locked_until, lockout_count) VALUES (?, 1, ?, ?, NULL, 0)",
        )
        .bind(vec![
            Param::Text(key.to_string()),
            Param::Text(now_iso.clone()),
            Param::Text(now_iso),
        ])
        .run()
        .await?;
        return Ok(None);
    }
    match outcome.locked_until_ms {
        Some(locked_ms) => {
            let locked_until = iso_from_ms(locked_ms);
            db.prepare(
                "UPDATE wl_login_attempts SET attempts = ?, first_at = ?, last_at = ?, locked_until = ?, lockout_count = ? WHERE key = ?",
            )
            .bind(vec![
                Param::Integer(outcome.attempts),
                Param::Text(iso_from_ms(outcome.first_at_ms)),
                Param::Text(now_iso),
                Param::Text(locked_until.clone()),
                Param::Integer(outcome.lockout_count),
                Param::Text(key.to_string()),
            ])
            .run()
            .await?;
            Ok(Some(locked_until))
        }
        None => {
            db.prepare(
                "UPDATE wl_login_attempts SET attempts = ?, first_at = ?, last_at = ?, locked_until = NULL WHERE key = ?",
            )
            .bind(vec![
                Param::Integer(outcome.attempts),
                Param::Text(iso_from_ms(outcome.first_at_ms)),
                Param::Text(now_iso),
                Param::Text(key.to_string()),
            ])
            .run()
            .await?;
            Ok(None)
        }
    }
}

/// Clear a key's record (successful login).
pub async fn clear_failed_logins(db: &dyn Db, key: &str) -> Result<(), WordlessError> {
    db.prepare("DELETE FROM wl_login_attempts WHERE key = ?")
        .bind(vec![Param::Text(key.to_string())])
        .run()
        .await?;
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LockedKey {
    pub key: String,
    pub attempts: i64,
    pub locked_until: String,
    pub lockout_count: i64,
}

/// All currently-locked keys (for the admin Security page).
pub async fn list_locked_keys(db: &dyn Db, now_ms: u64) -> Result<Vec<LockedKey>, WordlessError> {
    let rows = db
        .prepare(
            "SELECT * FROM wl_login_attempts WHERE locked_until IS NOT NULL ORDER BY locked_until DESC LIMIT 100",
        )
        .all::<Row>()
        .await?;
    let mut out = Vec::new();
    for row in rows {
        let Some(locked_until) = row_opt_text(&row, "locked_until") else {
            continue;
        };
        if let Some(ms) = ms_from_iso(&locked_until) {
            if ms > now_ms {
                out.push(LockedKey {
                    key: row_text(&row, "key"),
                    attempts: row_int(&row, "attempts"),
                    locked_until,
                    lockout_count: row_int(&row, "lockout_count"),
                });
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;

    #[test]
    fn defaults_are_5_in_15_lock_15() {
        assert_eq!(
            LockoutConfig::default(),
            LockoutConfig {
                max_attempts: 5,
                window_minutes: 15,
                lockout_minutes: 15
            }
        );
    }

    #[test]
    fn first_failure_starts_the_window_without_locking() {
        let out = next_failed_attempt(None, LockoutConfig::default(), NOW);
        assert_eq!(out.attempts, 1);
        assert_eq!(out.first_at_ms, NOW);
        assert_eq!(out.locked_until_ms, None);
        assert_eq!(out.lockout_count, 0);
    }

    #[test]
    fn fifth_failure_in_window_locks_for_15_minutes() {
        let config = LockoutConfig::default();
        let mut state = None;
        let mut locked = None;
        for _ in 0..5 {
            let out = next_failed_attempt(state, config, NOW);
            locked = out.locked_until_ms;
            state = Some(AttemptState {
                attempts: out.attempts,
                first_at_ms: out.first_at_ms,
                lockout_count: out.lockout_count,
            });
        }
        assert_eq!(locked, Some(NOW + 15 * 60 * 1000));
        assert_eq!(state.unwrap().lockout_count, 1);
    }

    #[test]
    fn window_expiry_resets_the_counter() {
        let config = LockoutConfig::default();
        let state = Some(AttemptState {
            attempts: 4,
            first_at_ms: NOW,
            lockout_count: 0,
        });
        // 16 minutes later: outside the 15-minute window.
        let out = next_failed_attempt(state, config, NOW + 16 * 60 * 1000);
        assert_eq!(out.attempts, 1);
        assert_eq!(out.first_at_ms, NOW + 16 * 60 * 1000);
        assert_eq!(out.locked_until_ms, None);
        // Exactly at the window edge still counts as in-window (v1 `>=`).
        let out = next_failed_attempt(state, config, NOW + 15 * 60 * 1000);
        assert_eq!(out.attempts, 5);
        assert_eq!(out.locked_until_ms, Some(NOW + 30 * 60 * 1000));
    }

    #[test]
    fn consecutive_lockouts_double_capped_at_60() {
        let config = LockoutConfig::default();
        // After a first lockout (count 1), five more failures past the
        // window lock for 30 minutes; then 60, then still 60.
        let locked_once = AttemptState {
            attempts: 4,
            first_at_ms: NOW + 16 * 60 * 1000,
            lockout_count: 1,
        };
        let out = next_failed_attempt(Some(locked_once), config, NOW + 16 * 60 * 1000);
        assert_eq!(
            out.locked_until_ms,
            Some(NOW + 16 * 60 * 1000 + 30 * 60 * 1000)
        );
        assert_eq!(out.lockout_count, 2);

        let locked_twice = AttemptState {
            attempts: 4,
            first_at_ms: NOW,
            lockout_count: 2,
        };
        let out = next_failed_attempt(Some(locked_twice), config, NOW);
        assert_eq!(out.locked_until_ms, Some(NOW + 60 * 60 * 1000));
        assert_eq!(out.lockout_count, 3);

        let locked_many = AttemptState {
            attempts: 4,
            first_at_ms: NOW,
            lockout_count: 9,
        };
        let out = next_failed_attempt(Some(locked_many), config, NOW);
        assert_eq!(out.locked_until_ms, Some(NOW + 60 * 60 * 1000));
        assert_eq!(out.lockout_count, 10);
    }

    #[test]
    fn non_locking_failure_keeps_lockout_count() {
        let config = LockoutConfig::default();
        let state = Some(AttemptState {
            attempts: 2,
            first_at_ms: NOW,
            lockout_count: 1,
        });
        let out = next_failed_attempt(state, config, NOW + 1000);
        assert_eq!(out.attempts, 3);
        assert_eq!(out.locked_until_ms, None);
        assert_eq!(out.lockout_count, 1);
    }
}
