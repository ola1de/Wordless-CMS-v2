//! Password hashing with PBKDF2.
//! Stored format: `pbkdf2$<iterations>$<base64url salt>$<base64url hash>`
//! (Port of v1 `src/modules/users/password.ts` — same parameters:
//! PBKDF2-HMAC-SHA256, 100,000 iterations, 16-byte salt, 32-byte hash.)

use wordless_kernel::base64url;
use wordless_kernel::crypto;
use wordless_kernel::error::{internal_error, WordlessError};

pub const ITERATIONS: u32 = 100_000;
pub const HASH_BYTES: usize = 32;
pub const SALT_BYTES: usize = 16;

/// Parse a stored hash into (iterations, salt, hash). `None` on any
/// malformed input — a malformed stored hash never verifies.
pub fn parse_stored(stored: &str) -> Option<(u32, Vec<u8>, Vec<u8>)> {
    let parts: Vec<&str> = stored.split('$').collect();
    if parts.len() != 4 || parts[0] != "pbkdf2" {
        return None;
    }
    let iterations: u32 = parts[1].parse().ok()?;
    if iterations == 0 {
        return None;
    }
    let salt = base64url::decode(parts[2]).ok()?;
    let hash = base64url::decode(parts[3]).ok()?;
    Some((iterations, salt, hash))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    // Accumulate differences, no early exit (same as v1).
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// Hash a password for storage. Salt is fresh random bytes.
pub async fn hash_password(password: &str) -> Result<String, WordlessError> {
    let salt = crypto::random_bytes(SALT_BYTES)
        .map_err(|e| internal_error(&format!("random salt failed: {e}")))?;
    let hash = crypto::pbkdf2_sha256(password, &salt, ITERATIONS, HASH_BYTES)
        .await
        .map_err(|e| internal_error(&format!("password hashing failed: {e}")))?;
    Ok(format!(
        "pbkdf2${ITERATIONS}${}${}",
        base64url::encode(&salt),
        base64url::encode(&hash)
    ))
}

/// Verify a password against a stored hash. Uses the platform engine
/// (WebCrypto on wasm32, pure Rust on native targets).
pub async fn verify_password(password: &str, stored: &str) -> bool {
    let Some((iterations, salt, expected)) = parse_stored(stored) else {
        return false;
    };
    let actual = match crypto::pbkdf2_sha256(password, &salt, iterations, HASH_BYTES).await {
        Ok(v) => v,
        Err(_) => return false,
    };
    constant_time_eq(&actual, &expected)
}

/// Verify with the pure-Rust engine on every target. This is the
/// fallback engine on wasm32 and the function the WASM timing test
/// measures (doc §7 R3).
pub fn verify_password_rust(password: &str, stored: &str) -> bool {
    let Some((iterations, salt, expected)) = parse_stored(stored) else {
        return false;
    };
    let actual = crypto::pbkdf2_sha256_rust(password, &salt, iterations, HASH_BYTES);
    constant_time_eq(&actual, &expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same Node-generated vector as `wordless-kernel`'s crypto test:
    /// password "correct-horse-battery-staple", salt bytes 0..15,
    /// 100,000 iterations, SHA-256, 32-byte hash.
    /// (Regenerate with `node scripts/gen-pbkdf2-vector.mjs`.)
    const VECTOR_STORED: &str =
        "pbkdf2$100000$AAECAwQFBgcICQoLDA0ODw$B0ykfR9n7-dmQoY_sDnYh1fCuNlfUm6pcVo49btIvh8";

    #[test]
    fn parses_the_v1_format() {
        let (iter, salt, hash) = parse_stored(VECTOR_STORED).unwrap();
        assert_eq!(iter, 100_000);
        assert_eq!(salt, (0u8..16).collect::<Vec<u8>>());
        assert_eq!(hash.len(), 32);
        assert!(parse_stored("pbkdf2$100000$AAEC").is_none());
        assert!(parse_stored("bcrypt$10$abc$def").is_none());
        assert!(parse_stored(
            "pbkdf2$0$AAECAwQFBgcICQoLDA0ODw$B0ykfR9n7-dmQoY_sDnYh1fCuNlfUm6pcVo49btIvh8"
        )
        .is_none());
        assert!(parse_stored("").is_none());
    }

    #[test]
    fn rust_engine_verifies_the_node_vector() {
        assert!(verify_password_rust(
            "correct-horse-battery-staple",
            VECTOR_STORED
        ));
        assert!(!verify_password_rust("wrong-password", VECTOR_STORED));
        assert!(!verify_password_rust(
            "correct-horse-battery-staple",
            "not a hash"
        ));
    }

    #[test]
    fn hash_round_trips_through_both_engines() {
        let stored = futures_executor::block_on(hash_password("s3cret-passphrase")).unwrap();
        assert!(stored.starts_with("pbkdf2$100000$"));
        assert!(futures_executor::block_on(verify_password(
            "s3cret-passphrase",
            &stored
        )));
        assert!(!futures_executor::block_on(verify_password(
            "other", &stored
        )));
        assert!(verify_password_rust("s3cret-passphrase", &stored));
    }
}
