//! Guarded login: lockout → password → forced-reset → TOTP.
//! (Port of v1 `src/modules/security/service.ts` `loginWithSecurity`.
//! The coordinator wires `LoginOutcome` to the exact HTTP responses
//! documented on each variant; see the crate docs for the response
//! table.)
//!
//! Order matters and matches v1 exactly:
//! 1. lockout is checked BEFORE password verification, for both the
//!    `login:<name>` key and the `ip:<sha256-hex>` key;
//! 2. a wrong password, a wrong TOTP, or a wrong recovery code counts
//!    as a failed login on BOTH keys and writes an `auth.login_failed`
//!    audit row (audit logging never breaks the login response);
//! 3. a missing TOTP does NOT count as a failure (the password was
//!    right, the second factor simply was not offered yet);
//! 4. a successful login clears both keys.
//! Nothing here reveals whether the login name exists or whether the
//! password was right.

use wordless_db::{Db, Param};
use wordless_kernel::crypto;
use wordless_kernel::error::{internal_error, WordlessError};
use wordless_kernel::time::now_iso;

use crate::lockout::{
    clear_failed_logins, get_lockout_config, lockout_until, record_failed_login, LockoutConfig,
};
use crate::session::{create_session, Session, SessionOptions};
use crate::totp::{consume_recovery_code, totp_secret, verify_totp, PASSWORD_RESET_META_KEY};
use crate::users::{check_user_password, get_user_meta, User};

/// The outcome of a guarded login. The coordinator maps each variant
/// to v1's exact HTTP response (from `src/index.ts` `/api/auth/login`):
///
/// | Variant | Status | Code | Message |
/// |---|---|---|---|
/// | `Ok` | 200 | — | `{ "user": { id, login, email, displayName, role } }` + `Set-Cookie: wordless_session=<token>; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=604800` |
/// | `Invalid` | 401 | `unauthorized` | `Invalid login or password.` |
/// | `TotpRequired` | 401 | `totp_required` | `Two-factor authentication is required for this account.` |
/// | `LockedUntil` | 429 | `locked_out` | `Too many failed login attempts. Try again after <locked_until>.` |
/// | `ForcedReset` | 403 | `password_reset_required` | `This account was imported from WordPress and needs a password reset before it can sign in. Ask an administrator to reset the password.` |
#[derive(Debug)]
pub enum LoginOutcome {
    /// Password (+ TOTP where enabled) verified; a session was minted.
    Ok {
        user: User,
        token: String,
        session: Session,
    },
    /// Generic failure — deliberately does not say whether the login
    /// name exists or the password was right.
    Invalid,
    /// Password was right but the account has TOTP enabled and no
    /// (valid) code was supplied.
    TotpRequired,
    /// The login-name or IP key is locked until this ISO-8601 time.
    LockedUntil { locked_until: String },
    /// Imported user: `wl_password_reset_required` is `"1"`.
    ForcedReset,
}

/// SHA-256 hex of an IP address (raw IPs are never stored).
/// (v1 `hashIp`.)
pub async fn hash_ip(ip: &str) -> Result<String, WordlessError> {
    crypto::sha256_hex(ip.as_bytes())
        .await
        .map_err(|e| internal_error(&format!("IP hash failed: {e}")))
}

/// Best-effort audit row for a failed login (v1 `logActivity` inside
/// `fail()` — audit logging must never break the login response, so
/// errors are swallowed here, exactly as in v1).
async fn log_failed_login(db: &dyn Db, login_name: &str, ip_hash: Option<&str>) {
    let details = format!("Failed login for \"{login_name}\"");
    let _ = db
        .prepare(
            "INSERT INTO wl_audit_log (user_id, action, object_type, object_id, details, ip_hash, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(vec![
            Param::Integer(0),
            Param::Text("auth.login_failed".to_string()),
            Param::Text("user".to_string()),
            Param::Text("0".to_string()),
            Param::Text(details),
            match ip_hash {
                Some(h) => Param::Text(h.to_string()),
                None => Param::Null,
            },
            Param::Text(now_iso()),
        ])
        .run()
        .await;
}

/// Record one failed login on both keys and return the generic
/// `Invalid` outcome. (v1 `fail()` inside `loginWithSecurity`.)
async fn record_failure(
    db: &dyn Db,
    login_key: &str,
    ip_key: Option<&str>,
    config: LockoutConfig,
    login_name: &str,
    ip_hash: Option<&str>,
    now_ms: u64,
) -> Result<LoginOutcome, WordlessError> {
    record_failed_login(db, login_key, config, now_ms).await?;
    if let Some(ip_key) = ip_key {
        record_failed_login(db, ip_key, config, now_ms).await?;
    }
    log_failed_login(db, login_name, ip_hash).await;
    Ok(LoginOutcome::Invalid)
}

/// Full login guard: lockout → password → forced-reset → TOTP.
///
/// * `ip` is the raw client IP (e.g. `CF-Connecting-IP`); it is hashed
///   inside and only the hash is stored or used as a lockout key.
/// * `totp_code` is the 6-digit authenticator code, or a one-time
///   recovery code (`XXXXX-XXXXX`), when the account has 2FA enabled.
pub async fn login_guarded(
    db: &dyn Db,
    login_name: &str,
    password: &str,
    totp_code: Option<&str>,
    ip: Option<&str>,
    user_agent: Option<&str>,
    now_ms: u64,
) -> Result<LoginOutcome, WordlessError> {
    let config: LockoutConfig = get_lockout_config(db).await?;
    let ip_hash: Option<String> = match ip {
        Some(raw) if !raw.is_empty() => Some(hash_ip(raw).await?),
        _ => None,
    };
    let login_key = format!("login:{login_name}");
    let ip_key: Option<String> = ip_hash.as_ref().map(|h| format!("ip:{h}"));

    if let Some(locked) = lockout_until(db, &login_key, now_ms).await? {
        return Ok(LoginOutcome::LockedUntil {
            locked_until: locked,
        });
    }
    if let Some(ip_key) = &ip_key {
        if let Some(locked) = lockout_until(db, ip_key, now_ms).await? {
            return Ok(LoginOutcome::LockedUntil {
                locked_until: locked,
            });
        }
    }

    let Some(user) = check_user_password(db, login_name, password).await? else {
        return record_failure(
            db,
            &login_key,
            ip_key.as_deref(),
            config,
            login_name,
            ip_hash.as_deref(),
            now_ms,
        )
        .await;
    };

    let reset_required =
        get_user_meta::<String>(db, user.id, PASSWORD_RESET_META_KEY, String::new()).await?;
    if reset_required == "1" {
        return Ok(LoginOutcome::ForcedReset);
    }

    let secret = totp_secret(db, user.id).await?;
    if !secret.is_empty() {
        match totp_code {
            None => return Ok(LoginOutcome::TotpRequired),
            Some(code) if code.is_empty() => return Ok(LoginOutcome::TotpRequired),
            Some(code) => {
                let code = code.trim();
                if verify_totp(&secret, code, now_ms) {
                    // Correct TOTP — proceed to a session below.
                } else if consume_recovery_code(db, user.id, code).await? {
                    // A recovery code was used — proceed to a session below.
                } else {
                    return record_failure(
                        db,
                        &login_key,
                        ip_key.as_deref(),
                        config,
                        login_name,
                        ip_hash.as_deref(),
                        now_ms,
                    )
                    .await;
                }
            }
        }
    }

    clear_failed_logins(db, &login_key).await?;
    if let Some(ip_key) = &ip_key {
        clear_failed_logins(db, ip_key).await?;
    }
    let (token, session) = create_session(
        db,
        user.id,
        SessionOptions {
            user_agent: user_agent.map(str::to_string),
            ip_hash,
            ttl_days: None,
        },
    )
    .await?;
    Ok(LoginOutcome::Ok {
        user,
        token,
        session,
    })
}
