//! WORDLESS v2 auth.
//!
//! Passwords: PBKDF2-HMAC-SHA256, 100,000 iterations, 16-byte salt,
//! 32-byte hash, stored as `pbkdf2$<iterations>$<base64url salt>$<base64url hash>`
//! — byte-identical to v1 (`src/modules/users/password.ts`), so existing
//! users log in with existing passwords on day one of v2.
//!
//! Sessions: opaque 32-byte random tokens (base64url); only the SHA-256
//! hex hash is stored in `wl_sessions`; cookie `wordless_session`,
//! `HttpOnly; Secure; SameSite=Lax; Path=/`; 7-day absolute expiry and
//! a 24 h idle timeout (option `wl_session_idle_hours`).
//!
//! Guarded login (P2): lockout → password → forced-reset → TOTP, via
//! `login_guarded` (port of v1 `loginWithSecurity`). Login lockout is
//! 5 failed attempts in 15 minutes → 429, tracked per login name AND
//! per IP hash, with doubling lockouts capped at 60 minutes. TOTP is
//! RFC 6238 over HMAC-SHA1 with ±1 step of skew, 8 one-time recovery
//! codes (SHA-256 hashes stored), and the setup/confirm/disable/status
//! service functions. Imported users carry
//! `wl_password_reset_required = "1"` until an admin resets them.
//!
//! Route wiring (the coordinator owns HTTP): `login_guarded` returns a
//! `LoginOutcome`; each variant documents v1's exact status, error
//! code and message from `src/index.ts` `/api/auth/login`. The other
//! service functions mirror v1's modules one-to-one; error messages
//! are part of the API surface and match v1 byte for byte.

pub mod guarded;
pub mod lockout;
pub mod login;
pub mod password;
pub mod session;
pub mod totp;
pub mod users;

#[cfg(test)]
pub(crate) mod test_db;

pub use guarded::{hash_ip, login_guarded, LoginOutcome};
pub use lockout::{
    clear_failed_logins, get_lockout_config, list_locked_keys, lockout_until, next_failed_attempt,
    record_failed_login, AttemptState, FailedAttemptOutcome, LockedKey, LockoutConfig,
    DEFAULT_LOCKOUT_MINUTES, DEFAULT_MAX_ATTEMPTS, DEFAULT_WINDOW_MINUTES, MAX_LOCKOUT_MINUTES,
};
pub use login::{login, user_from_session};
pub use password::{hash_password, verify_password, verify_password_rust};
pub use session::{
    build_default_session_cookie, build_session_cookie, clear_session_cookie, create_session,
    list_user_sessions, parse_session_cookie, revoke_all_sessions_for_user, revoke_session,
    verify_session, Session, SessionOptions, SessionView, DEFAULT_SESSION_IDLE_HOURS,
    DEFAULT_TTL_DAYS, SESSION_COOKIE,
};
pub use totp::{
    base32_decode, base32_encode, confirm_totp, consume_recovery_code, disable_totp,
    generate_recovery_codes, generate_totp_secret, hash_recovery_code, hotp_code, is_totp_enabled,
    normalize_recovery_code, percent_encode_uri_component, setup_totp, totp_code, totp_setup_url,
    verify_totp, TotpSetup, PASSWORD_RESET_META_KEY,
};
pub use users::{
    change_password, check_user_password, clear_password_change_required,
    clear_password_reset_required, create_user, delete_user, delete_user_meta,
    generate_temporary_password, get_user_by_email, get_user_by_id, get_user_by_login,
    get_user_meta, get_user_profile, is_password_change_required, list_users, reset_user_password,
    role_from_capabilities_meta, set_password_change_required, set_user_role, update_user_meta,
    update_user_profile, validate_admin_theme, CreateUserInput, UpdateProfileInput, User,
    UserListResult, UserProfile, ADMIN_THEMES, ADMIN_THEME_META_KEY, AVATAR_META_KEY, BIO_META_KEY,
    PASSWORD_CHANGE_REQUIRED_META_KEY,
};
