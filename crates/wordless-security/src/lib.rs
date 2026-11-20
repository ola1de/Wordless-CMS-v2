//! WORDLESS v2 Security Centre (P3).
//!
//! One consolidated overview for the admin Security Centre — v1's
//! `SecurityOverview` shape (sessions, 2FA coverage across all
//! administrators, lockout, headers, recent failed logins). The audit
//! log is a separate endpoint in v1 (`GET /api/activity`) and stays
//! one here; P5 F7 restored the v1 shape after the P3 consolidation
//! had extended the struct (audit log, per-user TOTP flag, headers
//! array), which matched neither v1 nor the admin SPA.
//!
//! This crate is **aggregation only**. It reimplements nothing:
//!
//! * lockout state and config — `wordless-auth::lockout` (reads
//!   `wl_login_attempts` exactly the way `wordless-auth/src/lockout.rs`
//!   does, so a v1 database keeps its counters);
//! * TOTP status — `wordless-auth::totp::is_totp_enabled`;
//! * user listing + roles — `wordless-auth::list_users` (2FA
//!   coverage is reported for every administrator, as in v1);
//! * the security-headers list — `wordless-kernel::security`
//!   (byte-identical v1 values; never duplicated here);
//! * active sessions — a count over `wl_sessions` (migration 0001).
//!
//! ## Public API
//!
//! * [`overview::security_overview`] —
//!   `pub async fn security_overview(db: &dyn Db) -> Result<SecurityOverview, WordlessError>`.
//!   The lockout config plus currently-locked keys, 2FA coverage for
//!   every administrator, the count of active (unrevoked, unexpired)
//!   sessions, the CSP policy + applied flag, recent failed logins,
//!   and whether a user with login `admin` still exists.
//!   [`overview::security_overview_at`] is the same with an explicit
//!   `now_ms` (tests pin time through it; the plain function uses the
//!   platform clock).
//!
//! The overview reports measured posture, never a claim of "secure"
//! (same rule as v1 `getSecurityOverview`).

pub mod overview;

#[cfg(test)]
pub mod test_support;

pub use overview::{security_overview, security_overview_at};
pub use overview::{
    FailedLogin, HeadersState, LockoutState, SecurityOverview, SessionsState, TwoFactorUser,
};
