//! Login flow: check the password, mint a session.
//! (Port of v1 `src/modules/auth/service.ts` `login` / `userFromSession`.
//! The guarded flow — lockout, TOTP, forced reset — is P2 scope and is
//! NOT in this module yet; see the crate docs.)

use wordless_db::Db;
use wordless_kernel::error::WordlessError;
use wordless_kernel::time::now_ms;

use crate::session::{create_session, verify_session, Session, SessionOptions};
use crate::users::{check_user_password, get_user_by_id, User};

/// Authenticate by login + password; returns the user and a fresh
/// session token, or `None` when the credentials do not match.
pub async fn login(
    db: &dyn Db,
    login_name: &str,
    password: &str,
    opts: SessionOptions,
) -> Result<Option<(User, String, Session)>, WordlessError> {
    let Some(user) = check_user_password(db, login_name, password).await? else {
        return Ok(None);
    };
    let (token, session) = create_session(db, user.id, opts).await?;
    Ok(Some((user, token, session)))
}

/// Resolve the current user from a session token, or `None`.
pub async fn user_from_session(
    db: &dyn Db,
    token: Option<&str>,
) -> Result<Option<User>, WordlessError> {
    let Some(token) = token else {
        return Ok(None);
    };
    let Some(session) = verify_session(db, token, now_ms()).await? else {
        return Ok(None);
    };
    get_user_by_id(db, session.user_id).await
}
