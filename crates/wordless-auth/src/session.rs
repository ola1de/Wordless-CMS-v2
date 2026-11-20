//! Opaque sessions stored in D1 (`wl_sessions`).
//! (Port of v1 `src/modules/auth/service.ts` — same scheme: the bearer
//! token is random 32 bytes, base64url-encoded; only its SHA-256 hash
//! is stored, so a database read alone cannot impersonate a user.
//! Sessions are revocable and expire; an idle timeout (option
//! `wl_session_idle_hours`, default 24 h) revokes idle sessions on
//! next use; `last_seen_at` is updated at most once every 5 minutes.)

use wordless_db::options::get_option;
use wordless_db::row::{row_int, row_opt_text, row_text};
use wordless_db::{Db, Param};
use wordless_kernel::base64url;
use wordless_kernel::crypto;
use wordless_kernel::error::{internal_error, WordlessError};
use wordless_kernel::time::{iso_from_ms, ms_from_iso, now_ms};

pub const SESSION_COOKIE: &str = "wordless_session";
pub const DEFAULT_TTL_DAYS: i64 = 7;
pub const DEFAULT_SESSION_IDLE_HOURS: i64 = 24;
const LAST_SEEN_WRITE_MINUTES: u64 = 5;

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub user_id: i64,
    pub token_hash: String,
    pub created_at: String,
    pub expires_at: String,
    pub last_seen_at: Option<String>,
    pub ip_hash: Option<String>,
    pub user_agent: Option<String>,
    pub revoked: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SessionOptions {
    pub user_agent: Option<String>,
    pub ip_hash: Option<String>,
    pub ttl_days: Option<i64>,
}

/// Idle timeout in hours (option `wl_session_idle_hours`, default 24).
pub async fn get_session_idle_hours(db: &dyn Db) -> Result<i64, WordlessError> {
    let hours: i64 = get_option(db, "wl_session_idle_hours", DEFAULT_SESSION_IDLE_HOURS).await?;
    if hours > 0 {
        Ok(hours)
    } else {
        Ok(DEFAULT_SESSION_IDLE_HOURS)
    }
}

fn uuid_v4() -> Result<String, WordlessError> {
    let mut b = crypto::random_bytes(16)
        .map_err(|e| internal_error(&format!("random uuid failed: {e}")))?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13],
        b[14], b[15]
    ))
}

pub async fn create_session(
    db: &dyn Db,
    user_id: i64,
    opts: SessionOptions,
) -> Result<(String, Session), WordlessError> {
    let token = base64url::encode(
        &crypto::random_bytes(32)
            .map_err(|e| internal_error(&format!("random token failed: {e}")))?,
    );
    let token_hash = crypto::sha256_hex(token.as_bytes())
        .await
        .map_err(|e| internal_error(&format!("token hash failed: {e}")))?;
    let now = now_ms();
    let ttl_days = opts.ttl_days.unwrap_or(DEFAULT_TTL_DAYS);
    let session = Session {
        id: uuid_v4()?,
        user_id,
        token_hash,
        created_at: iso_from_ms(now),
        expires_at: iso_from_ms(now + (ttl_days as u64) * 24 * 60 * 60 * 1000),
        last_seen_at: Some(iso_from_ms(now)),
        ip_hash: opts.ip_hash,
        user_agent: opts.user_agent,
        revoked: false,
    };
    db.prepare(
        "INSERT INTO wl_sessions (id, user_id, token_hash, created_at, expires_at, last_seen_at, ip_hash, user_agent, revoked)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0)",
    )
    .bind(vec![
        Param::Text(session.id.clone()),
        Param::Integer(session.user_id),
        Param::Text(session.token_hash.clone()),
        Param::Text(session.created_at.clone()),
        Param::Text(session.expires_at.clone()),
        Param::Text(session.last_seen_at.clone().unwrap_or_default()),
        opt_text(&session.ip_hash),
        opt_text(&session.user_agent),
    ])
    .run()
    .await?;
    Ok((token, session))
}

fn opt_text(v: &Option<String>) -> Param {
    match v {
        Some(s) => Param::Text(s.clone()),
        None => Param::Null,
    }
}

/// Verify a session token. Enforces the absolute expiry AND the idle
/// timeout; an idle-timed-out session is revoked. `last_seen_at` is
/// refreshed at most once every 5 minutes.
pub async fn verify_session(
    db: &dyn Db,
    token: &str,
    now: u64,
) -> Result<Option<Session>, WordlessError> {
    if token.is_empty() {
        return Ok(None);
    }
    let token_hash = crypto::sha256_hex(token.as_bytes())
        .await
        .map_err(|e| internal_error(&format!("token hash failed: {e}")))?;
    let row = db
        .prepare("SELECT * FROM wl_sessions WHERE token_hash = ? AND revoked = 0 LIMIT 1")
        .bind(vec![Param::Text(token_hash)])
        .first::<serde_json::Value>()
        .await?;
    let Some(row) = row else { return Ok(None) };
    let session = row_to_session(&row);
    let now_iso = iso_from_ms(now);
    if session.expires_at <= now_iso {
        return Ok(None);
    }
    let idle_hours = get_session_idle_hours(db).await?;
    let last_seen_ms = session
        .last_seen_at
        .as_deref()
        .and_then(ms_from_iso)
        .or_else(|| ms_from_iso(&session.created_at))
        .unwrap_or(0);
    if now.saturating_sub(last_seen_ms) > (idle_hours as u64) * 60 * 60 * 1000 {
        db.prepare("UPDATE wl_sessions SET revoked = 1 WHERE id = ?")
            .bind(vec![Param::Text(session.id.clone())])
            .run()
            .await?;
        return Ok(None);
    }
    if now.saturating_sub(last_seen_ms) > LAST_SEEN_WRITE_MINUTES * 60 * 1000 {
        db.prepare("UPDATE wl_sessions SET last_seen_at = ? WHERE id = ?")
            .bind(vec![
                Param::Text(now_iso.clone()),
                Param::Text(session.id.clone()),
            ])
            .run()
            .await?;
    }
    let mut session = session;
    session.last_seen_at = Some(now_iso);
    Ok(Some(session))
}

fn row_to_session(row: &serde_json::Value) -> Session {
    Session {
        id: row_text(row, "id"),
        user_id: row_int(row, "user_id"),
        token_hash: row_text(row, "token_hash"),
        created_at: row_text(row, "created_at"),
        expires_at: row_text(row, "expires_at"),
        last_seen_at: row_opt_text(row, "last_seen_at"),
        ip_hash: row_opt_text(row, "ip_hash"),
        user_agent: row_opt_text(row, "user_agent"),
        revoked: row_int(row, "revoked") == 1,
    }
}

pub async fn revoke_session(db: &dyn Db, token: &str) -> Result<bool, WordlessError> {
    if token.is_empty() {
        return Ok(false);
    }
    let token_hash = crypto::sha256_hex(token.as_bytes())
        .await
        .map_err(|e| internal_error(&format!("token hash failed: {e}")))?;
    let meta = db
        .prepare("UPDATE wl_sessions SET revoked = 1 WHERE token_hash = ? AND revoked = 0")
        .bind(vec![Param::Text(token_hash)])
        .run()
        .await?;
    Ok(meta.changes > 0)
}

pub async fn revoke_all_sessions_for_user(db: &dyn Db, user_id: i64) -> Result<u64, WordlessError> {
    let meta = db
        .prepare("UPDATE wl_sessions SET revoked = 1 WHERE user_id = ? AND revoked = 0")
        .bind(vec![Param::Integer(user_id)])
        .run()
        .await?;
    Ok(meta.changes)
}

/// One session as shown to its owner. No token values are ever
/// returned — only the first 8 hex chars of the IP hash, so the user
/// can recognise the device. (v1 `SessionView` in
/// `src/modules/security/service.ts`.)
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub id: String,
    pub created_at: String,
    pub expires_at: String,
    pub last_seen_at: Option<String>,
    pub user_agent: Option<String>,
    pub ip_hash_prefix: Option<String>,
    pub revoked: bool,
    pub current: bool,
}

/// The user's own sessions, newest first (v1 `listUserSessions`).
/// `current_token` marks the session the request came in on.
pub async fn list_user_sessions(
    db: &dyn Db,
    user_id: i64,
    current_token: Option<&str>,
) -> Result<Vec<SessionView>, WordlessError> {
    let current_hash: Option<String> = match current_token {
        Some(t) if !t.is_empty() => Some(
            crypto::sha256_hex(t.as_bytes())
                .await
                .map_err(|e| internal_error(&format!("token hash failed: {e}")))?,
        ),
        _ => None,
    };
    let rows = db
        .prepare("SELECT * FROM wl_sessions WHERE user_id = ? ORDER BY created_at DESC LIMIT 100")
        .bind(vec![Param::Integer(user_id)])
        .all::<serde_json::Value>()
        .await?;
    Ok(rows
        .iter()
        .map(|row| {
            let ip_hash = row_opt_text(row, "ip_hash");
            SessionView {
                id: row_text(row, "id"),
                created_at: row_text(row, "created_at"),
                expires_at: row_text(row, "expires_at"),
                last_seen_at: row_opt_text(row, "last_seen_at"),
                user_agent: row_opt_text(row, "user_agent"),
                ip_hash_prefix: match &ip_hash {
                    Some(h) if !h.is_empty() => Some(h.chars().take(8).collect()),
                    _ => None,
                },
                revoked: row_int(row, "revoked") == 1,
                current: match &current_hash {
                    Some(h) => row_text(row, "token_hash") == *h,
                    None => false,
                },
            }
        })
        .collect())
}

pub fn build_session_cookie(token: &str, max_age_seconds: i64) -> String {
    format!("{SESSION_COOKIE}={token}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={max_age_seconds}")
}

pub fn build_default_session_cookie(token: &str) -> String {
    build_session_cookie(token, DEFAULT_TTL_DAYS * 24 * 60 * 60)
}

pub fn clear_session_cookie() -> String {
    format!("{SESSION_COOKIE}=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0")
}

pub fn parse_session_cookie(cookie_header: Option<&str>) -> Option<String> {
    let header = cookie_header?;
    for part in header.split(';') {
        let part = part.trim();
        let mut split = part.splitn(2, '=');
        let name = split.next().unwrap_or("");
        if name == SESSION_COOKIE {
            let value = split.next().unwrap_or("");
            return if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            };
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_round_trips() {
        let cookie = build_default_session_cookie("tok123");
        assert!(cookie.starts_with("wordless_session=tok123; "));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Lax"));
        assert!(cookie.contains("Max-Age=604800"));
        let header = format!("other=1; {cookie}; theme=dark");
        assert_eq!(
            parse_session_cookie(Some(&header)).as_deref(),
            Some("tok123")
        );
        assert_eq!(parse_session_cookie(Some("other=1; theme=dark")), None);
        assert_eq!(parse_session_cookie(None), None);
        assert_eq!(
            clear_session_cookie(),
            "wordless_session=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0"
        );
    }

    #[test]
    fn uuid_v4_shape() {
        let id = uuid_v4().unwrap();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert!(["8", "9", "a", "b"].contains(&&id[19..20]));
        assert_ne!(uuid_v4().unwrap(), id);
    }
}
