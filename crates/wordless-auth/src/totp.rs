//! TOTP two-factor authentication (RFC 6238 over HMAC-SHA1,
//! 30-second steps, 6-digit codes, ±1 step of clock skew accepted).
//! (Port of v1 `src/modules/security/totp.ts` and the TOTP parts of
//! `src/modules/security/service.ts` — same secrets, same codes, same
//! recovery-code storage, same error messages, so a v1 database keeps
//! working and v1-configured authenticator apps keep producing valid
//! codes after cutover.)
//!
//! Secrets are 20 random bytes (160 bits), base32-encoded per RFC 4648
//! (hand-rolled here — there is no base32 crate in the offline registry
//! cache). Recovery codes are 10-character codes (displayed
//! `XXXXX-XXXXX`); only their SHA-256 hashes are stored, and each code
//! works once.
//!
//! Usermeta keys (all JSON in `wp_usermeta`, as in v1):
//! - `wl_2fa_secret`: the enabled TOTP secret (base32).
//! - `wl_2fa_pending_secret`: a setup secret awaiting confirmation.
//! - `wl_2fa_recovery_hashes`: JSON array of SHA-256 hashes of recovery
//!   codes (plaintext codes are shown once, at setup, never stored).
//! - `wl_password_reset_required`: `"1"` while an imported user must
//!   reset their password (WordPress phpass hashes are not portable).

use hmac::{Hmac, Mac};
use sha1::Sha1;

use wordless_db::Db;
use wordless_kernel::crypto;
use wordless_kernel::error::{bad_request, internal_error, WordlessError};

use crate::session::revoke_all_sessions_for_user;
use crate::users::{
    check_user_password, delete_user_meta, get_user_by_id, get_user_meta, update_user_meta,
};

/// Set while an imported user must reset their password before they
/// can sign in (v1 `PASSWORD_RESET_META_KEY`).
pub const PASSWORD_RESET_META_KEY: &str = "wl_password_reset_required";
const TOTP_SECRET_KEY: &str = "wl_2fa_secret";
const TOTP_PENDING_KEY: &str = "wl_2fa_pending_secret";
const TOTP_RECOVERY_KEY: &str = "wl_2fa_recovery_hashes";

const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
const RECOVERY_CHARS: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

/// RFC 4648 base32 encode (same algorithm as v1 `base32Encode`,
/// including `=` padding to a multiple of 8 characters).
pub fn base32_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut bits = 0u32;
    let mut value = 0u32;
    for &b in bytes {
        value = (value << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            out.push(BASE32_ALPHABET[((value >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(BASE32_ALPHABET[((value << (5 - bits)) & 31) as usize] as char);
    }
    while out.len() % 8 != 0 {
        out.push('=');
    }
    out
}

/// RFC 4648 base32 decode (same as v1 `base32Decode`: trims, strips
/// trailing `=`, uppercases, returns `None` on empty or invalid input).
pub fn base32_decode(input: &str) -> Option<Vec<u8>> {
    let clean = input.trim().trim_end_matches('=').to_uppercase();
    if clean.is_empty() {
        return None;
    }
    let mut bits = 0u32;
    let mut value = 0u32;
    let mut bytes = Vec::new();
    for ch in clean.chars() {
        let idx = BASE32_ALPHABET.iter().position(|&c| c as char == ch)?;
        value = (value << 5) | idx as u32;
        bits += 5;
        if bits >= 8 {
            bytes.push(((value >> (bits - 8)) & 0xff) as u8);
            bits -= 8;
        }
    }
    Some(bytes)
}

fn hmac_sha1(key: &[u8], message: &[u8]) -> [u8; 20] {
    // HMAC accepts any key length, so `new_from_slice` cannot fail.
    let mut mac = Hmac::<Sha1>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(message);
    mac.finalize().into_bytes().into()
}

/// HOTP code for one counter value (RFC 4226 dynamic truncation).
/// Returns `None` for a malformed secret.
pub fn hotp_code(secret: &str, counter: i64) -> Option<String> {
    let key = base32_decode(secret)?;
    // 8-byte big-endian counter. v1 builds the bytes with
    // `Math.floor(c / 256)` per byte, so a negative counter fills with
    // 0xff — the arithmetic shift below does exactly the same.
    let mut msg = [0u8; 8];
    let mut c = counter;
    for i in (0..8).rev() {
        msg[i] = (c & 0xff) as u8;
        c >>= 8;
    }
    let digest = hmac_sha1(&key, &msg);
    let offset = (digest[19] & 0x0f) as usize;
    let binary = (((digest[offset] & 0x7f) as u32) << 24)
        | ((digest[offset + 1] as u32) << 16)
        | ((digest[offset + 2] as u32) << 8)
        | (digest[offset + 3] as u32);
    Some(format!("{:06}", binary % 1_000_000))
}

/// The TOTP code for a moment in time (30 s steps).
pub fn totp_code(secret: &str, now_ms: u64) -> Option<String> {
    hotp_code(secret, (now_ms / 30_000) as i64)
}

/// Verify a 6-digit code against a secret, accepting ±1 step of clock
/// skew. Returns false for malformed codes and malformed secrets.
pub fn verify_totp(secret: &str, code: &str, now_ms: u64) -> bool {
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let step = (now_ms / 30_000) as i64;
    for offset in [-1i64, 0, 1] {
        if let Some(expected) = hotp_code(secret, step + offset) {
            if expected == code {
                return true;
            }
        }
    }
    false
}

/// Percent-encode like JavaScript `encodeURIComponent`: everything
/// except `A-Z a-z 0-9 - _ . ! ~ * ' ( )` becomes `%XX` (upper hex).
pub fn percent_encode_uri_component(s: &str) -> String {
    let mut out = String::new();
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Generate a fresh 20-byte TOTP secret (base32).
pub fn generate_totp_secret() -> Result<String, WordlessError> {
    let bytes = crypto::random_bytes(20)
        .map_err(|e| internal_error(&format!("random TOTP secret failed: {e}")))?;
    Ok(base32_encode(&bytes))
}

/// The `otpauth://` URL an authenticator app scans (also shown as
/// manual code). Same shape as v1 `totpSetupUrl`.
pub fn totp_setup_url(secret: &str, account: &str, issuer: &str) -> String {
    let label = format!(
        "{}:{}",
        percent_encode_uri_component(issuer),
        percent_encode_uri_component(account)
    );
    format!(
        "otpauth://totp/{label}?secret={secret}&issuer={}&algorithm=SHA1&digits=6&period=30",
        percent_encode_uri_component(issuer)
    )
}

/// Generate recovery codes (displayed once, at setup). v1 generates 8.
pub fn generate_recovery_codes(count: usize) -> Result<Vec<String>, WordlessError> {
    let mut codes = Vec::with_capacity(count);
    for _ in 0..count {
        let bytes = crypto::random_bytes(10)
            .map_err(|e| internal_error(&format!("random recovery code failed: {e}")))?;
        let mut raw = String::with_capacity(10);
        for b in bytes {
            raw.push(RECOVERY_CHARS[(b as usize) % RECOVERY_CHARS.len()] as char);
        }
        codes.push(format!("{}-{}", &raw[..5], &raw[5..]));
    }
    Ok(codes)
}

/// Normalise a recovery code as typed (case-insensitive, dash optional).
pub fn normalize_recovery_code(code: &str) -> String {
    code.trim().to_uppercase().replace('-', "")
}

/// Hash a recovery code for storage (only hashes are stored).
pub async fn hash_recovery_code(code: &str) -> Result<String, WordlessError> {
    let normalized = normalize_recovery_code(code);
    crypto::sha256_hex(normalized.as_bytes())
        .await
        .map_err(|e| internal_error(&format!("recovery code hash failed: {e}")))
}

/// The user's enabled TOTP secret, or `""` when 2FA is not enabled.
/// (`pub(crate)`: the coordinator needs `is_totp_enabled`, not the
/// secret itself.)
pub(crate) async fn totp_secret(db: &dyn Db, user_id: i64) -> Result<String, WordlessError> {
    get_user_meta::<String>(db, user_id, TOTP_SECRET_KEY, String::new()).await
}

/// Consume a recovery code (one-time). Returns true when the code matched.
pub async fn consume_recovery_code(
    db: &dyn Db,
    user_id: i64,
    code: &str,
) -> Result<bool, WordlessError> {
    let mut hashes =
        get_user_meta::<Vec<String>>(db, user_id, TOTP_RECOVERY_KEY, Vec::new()).await?;
    let hash = hash_recovery_code(code).await?;
    let Some(idx) = hashes.iter().position(|h| *h == hash) else {
        return Ok(false);
    };
    hashes.remove(idx);
    update_user_meta(db, user_id, TOTP_RECOVERY_KEY, &hashes).await?;
    Ok(true)
}

pub async fn is_totp_enabled(db: &dyn Db, user_id: i64) -> Result<bool, WordlessError> {
    Ok(!totp_secret(db, user_id).await?.is_empty())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TotpSetup {
    pub secret: String,
    pub url: String,
}

/// Start 2FA setup: generate a secret, store it as pending, return the
/// `otpauth://` URL. (v1 `setupTotp`.)
pub async fn setup_totp(
    db: &dyn Db,
    user_id: i64,
    account_label: &str,
) -> Result<TotpSetup, WordlessError> {
    if get_user_by_id(db, user_id).await?.is_none() {
        return Err(bad_request("User not found."));
    }
    let secret = generate_totp_secret()?;
    update_user_meta(db, user_id, TOTP_PENDING_KEY, &secret).await?;
    let url = totp_setup_url(&secret, account_label, "Wordless");
    Ok(TotpSetup { secret, url })
}

/// Confirm setup with a code from the authenticator app. Returns the
/// recovery codes (shown once). (v1 `confirmTotp`.)
pub async fn confirm_totp(
    db: &dyn Db,
    user_id: i64,
    code: &str,
    now_ms: u64,
) -> Result<Vec<String>, WordlessError> {
    let pending = get_user_meta::<String>(db, user_id, TOTP_PENDING_KEY, String::new()).await?;
    if pending.is_empty() {
        return Err(bad_request(
            "No two-factor setup in progress. Start setup first.",
        ));
    }
    if !verify_totp(&pending, code, now_ms) {
        return Err(bad_request(
            "That code is not valid. Check the time on your device and try again.",
        ));
    }
    update_user_meta(db, user_id, TOTP_SECRET_KEY, &pending).await?;
    delete_user_meta(db, user_id, TOTP_PENDING_KEY).await?;
    let recovery_codes = generate_recovery_codes(8)?;
    let mut hashes = Vec::with_capacity(recovery_codes.len());
    for c in &recovery_codes {
        hashes.push(hash_recovery_code(c).await?);
    }
    update_user_meta(db, user_id, TOTP_RECOVERY_KEY, &hashes).await?;
    Ok(recovery_codes)
}

/// Disable 2FA (needs the account password). Signs the user out
/// everywhere. Returns `true` — the route body is `{ "disabled": true }`.
/// (v1 `disableTotp`.)
pub async fn disable_totp(
    db: &dyn Db,
    user_id: i64,
    password: &str,
) -> Result<bool, WordlessError> {
    let Some(user) = get_user_by_id(db, user_id).await? else {
        return Err(bad_request("User not found."));
    };
    if check_user_password(db, &user.login, password)
        .await?
        .is_none()
    {
        return Err(bad_request("That password is not correct."));
    }
    delete_user_meta(db, user_id, TOTP_SECRET_KEY).await?;
    delete_user_meta(db, user_id, TOTP_PENDING_KEY).await?;
    delete_user_meta(db, user_id, TOTP_RECOVERY_KEY).await?;
    revoke_all_sessions_for_user(db, user_id).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 test vectors (SHA-1, secret ASCII "12345678901234567890",
    /// base32 GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ). The 8-digit RFC values
    /// are 94287082 / 07081804 / 14050471 / 89005924 / 69279037 /
    /// 65353130; v1 uses 6 digits, i.e. `binary % 1_000_000`. The
    /// 6-digit values below were cross-checked against Python's
    /// `hmac`/`hashlib` (an independent implementation) before being
    /// written here.
    const RFC_SECRET: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
    const RFC_VECTORS: [(u64, &str); 6] = [
        (59 * 1000, "287082"),
        (1_111_111_109 * 1000, "081804"),
        (1_111_111_111 * 1000, "050471"),
        (1_234_567_890 * 1000, "005924"),
        (2_000_000_000 * 1000, "279037"),
        (20_000_000_000 * 1000, "353130"),
    ];

    #[test]
    fn base32_round_trips_like_v1() {
        assert_eq!(
            base32_encode(b"12345678901234567890"),
            "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"
        );
        assert_eq!(
            base32_decode("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ").unwrap(),
            b"12345678901234567890"
        );
        // Padding + case-insensitivity, as in v1's decoder.
        assert_eq!(base32_encode(b"f"), "MY======");
        assert_eq!(base32_decode("MY======").unwrap(), b"f");
        assert_eq!(base32_decode("my======").unwrap(), b"f");
        assert_eq!(base32_decode("  MY======  ").unwrap(), b"f");
        // Empty / invalid input decodes to None, as in v1.
        assert_eq!(base32_decode(""), None);
        assert_eq!(base32_decode("======"), None);
        assert_eq!(base32_decode("MY*====="), None);
        assert_eq!(base32_decode("not!valid"), None);
    }

    #[test]
    fn rfc6238_vectors_and_skew_window() {
        for (now_ms, expected) in RFC_VECTORS {
            assert_eq!(
                totp_code(RFC_SECRET, now_ms).as_deref(),
                Some(expected),
                "T={now_ms}"
            );
            assert!(verify_totp(RFC_SECRET, expected, now_ms));
            // ±1 step accepted, ±3 steps rejected (same as v1's test).
            assert!(verify_totp(RFC_SECRET, expected, now_ms + 30_000));
            assert!(verify_totp(
                RFC_SECRET,
                expected,
                now_ms.saturating_sub(30_000)
            ));
            assert!(!verify_totp(RFC_SECRET, expected, now_ms + 90_000));
            // -90 s is only meaningful when it stays after the epoch:
            // at time 0, step 0's +1 skew legitimately reaches counter 1
            // (the 59 s code), so the 59 s vector skips this check.
            if now_ms > 90_000 {
                assert!(!verify_totp(RFC_SECRET, expected, now_ms - 90_000));
            }
        }
        assert_eq!(totp_code(RFC_SECRET, 59 * 1000).as_deref(), Some("287082"));
        assert!(!verify_totp(RFC_SECRET, "000000", 59 * 1000));
        assert!(!verify_totp(RFC_SECRET, "not-a-code", 59 * 1000));
        assert!(!verify_totp(RFC_SECRET, "28708", 59 * 1000));
        assert!(!verify_totp(RFC_SECRET, "2870821", 59 * 1000));
        assert_eq!(hotp_code(RFC_SECRET, 1).as_deref(), Some("287082"));
        assert_eq!(hotp_code("not a secret!!", 1), None);
        assert_eq!(totp_code("not a secret!!", 59 * 1000), None);
    }

    #[test]
    fn setup_url_shape_matches_v1() {
        assert_eq!(
            totp_setup_url("JBSWY3DPEHPK3PXP", "admin", "Wordless"),
            "otpauth://totp/Wordless:admin?secret=JBSWY3DPEHPK3PXP&issuer=Wordless&algorithm=SHA1&digits=6&period=30"
        );
        // encodeURIComponent behaviour: space and @ are encoded.
        assert_eq!(
            totp_setup_url("JBSWY3DPEHPK3PXP", "john doe@example", "Wordless"),
            "otpauth://totp/Wordless:john%20doe%40example?secret=JBSWY3DPEHPK3PXP&issuer=Wordless&algorithm=SHA1&digits=6&period=30"
        );
        assert_eq!(
            percent_encode_uri_component("a-b_c.d!~*'()"),
            "a-b_c.d!~*'()"
        );
        assert_eq!(
            percent_encode_uri_component("a/b?c=d&e"),
            "a%2Fb%3Fc%3Dd%26e"
        );
    }

    #[test]
    fn secret_generation_is_20_random_base32_bytes() {
        let a = generate_totp_secret().unwrap();
        let b = generate_totp_secret().unwrap();
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| BASE32_ALPHABET.contains(&c)));
        assert_eq!(base32_decode(&a).unwrap().len(), 20);
        assert_ne!(a, b);
    }

    #[test]
    fn recovery_codes_shape_and_hashing() {
        let codes = generate_recovery_codes(8).unwrap();
        assert_eq!(codes.len(), 8);
        for code in &codes {
            assert_eq!(code.len(), 11);
            assert_eq!(&code[5..6], "-");
            assert!(code
                .chars()
                .filter(|c| *c != '-')
                .all(|c| RECOVERY_CHARS.contains(&(c as u8))));
        }
        // All distinct.
        let mut sorted = codes.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 8);

        assert_eq!(normalize_recovery_code("abcde-fghij"), "ABCDEFGHIJ");
        assert_eq!(normalize_recovery_code("  Ab-Cd-Ef  "), "ABCDEF");
        // Hash is SHA-256 of the normalised code. sha256("ABC") was
        // verified with `printf 'ABC' | sha256sum` on the host.
        let hashed = futures_executor::block_on(hash_recovery_code("abc")).unwrap();
        assert_eq!(
            hashed,
            "b5d4045c3f466fa91fe2cc6abe79232a1a57cdf104f7a26e716e0a1e2789df78"
        );
        assert_eq!(
            futures_executor::block_on(hash_recovery_code("ABC")).unwrap(),
            hashed
        );
        assert_eq!(
            futures_executor::block_on(hash_recovery_code("a-b-c")).unwrap(),
            hashed
        );
    }
}
