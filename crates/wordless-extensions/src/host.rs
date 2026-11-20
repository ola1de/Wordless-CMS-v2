//! Host-call protocol — the wire format between an extension Worker
//! and the main Worker.
//!
//! The extension Worker's shim implements the synchronous `wp.*` methods
//! locally. Every asynchronous `wp.*` method is a host call: the shim
//! `fetch()`es `POST /api/extensions/:slug/host` with the per-load
//! bearer token, which the main Worker checks before the capability,
//! then runs the real implementation. A denied call returns
//! `capability_denied` — the extension can see the denial, never the
//! data.
//!
//! Token lifecycle (stateless Workers cannot keep a token in memory
//! between requests, so the token is *derived*, not remembered):
//! `derive_host_token(secret, slug, epoch)` is HMAC-SHA256 over
//! `"wordless-ext-host:v1:<slug>:<epoch>"` with the deployment secret
//! (`WORDLESS_EXT_HOST_SECRET`, set by the deploy scripts — auto-
//! generated if absent). Loading an extension stores only the SHA-256
//! hash of the derived token in D1
//! (`wl_extension_instances.host_token_hash`) and increments the
//! epoch, so a reload rotates the token; dispatch re-derives the same
//! token from the same inputs and sends it to the extension Worker
//! only in the dispatch request (never baked into the deployed script,
//! never stored in R2). The host endpoint verifies by comparing the
//! hash of the presented token against the stored hash — if the secret
//! rotated after load, the derived token no longer matches and host
//! calls fail closed until the extension is reloaded.
//!
//! Token storage: only the SHA-256 hex of the token is stored in D1,
//! the same pattern as the session-token hashes in `wordless-auth`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// One host call from an extension Worker to the main Worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostCall {
    /// `wp.<name>` without the `wp.` prefix, e.g. `"get_option"`.
    pub call: String,
    /// Positional arguments, JSON values.
    #[serde(default)]
    pub args: Vec<Value>,
}

/// Result of a host call, in the v1 `{ok, data, error}` envelope shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<HostError>,
}

/// A denied or failed host call. `code` is machine-readable;
/// `message` is safe to show to an admin (never contains secrets).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostError {
    pub code: String,
    pub message: String,
}

impl HostResult {
    pub fn ok(data: Value) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn denied(capability: &str, call: &str) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(HostError {
                code: "capability_denied".to_string(),
                message: format!(
                    "wp_{call}() needs the {capability} capability, which this extension has not been granted"
                ),
            }),
        }
    }

    pub fn not_implemented(call: &str) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(HostError {
                code: "not_implemented".to_string(),
                message: format!("wp_{call}() is not available in the P6 host API"),
            }),
        }
    }

    pub fn bad_call(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(HostError {
                code: "bad_call".to_string(),
                message: message.into(),
            }),
        }
    }
}

/// Derive the per-load bearer token for an extension from the
/// deployment secret, the slug, and the instance's token epoch.
/// Deterministic: loading and dispatch derive the same token from the
/// same inputs, so a stateless Worker can reproduce it. A reload
/// increments the epoch and therefore rotates the token.
pub fn derive_host_token(secret: &str, slug: &str, epoch: i64) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(format!("wordless-ext-host:v1:{slug}:{epoch}").as_bytes());
    wordless_kernel::base64url::encode(&mac.finalize().into_bytes())
}

/// Generate a per-load bearer token: 32 random bytes, base64url.
/// Randomness comes from `wordless_kernel::crypto::random_bytes`
/// (getrandom — WebCrypto-backed in the Worker). The caller must store
/// only [`token_hash`] in D1.
///
/// Kept for tests and for callers that need a one-off token; the
/// dispatch path uses [`derive_host_token`] instead, so the token can
/// be reproduced at dispatch time.
pub fn generate_host_token() -> Result<String, wordless_kernel::error::WordlessError> {
    let bytes = wordless_kernel::crypto::random_bytes(32).map_err(|e| {
        wordless_kernel::error::WordlessError::new("internal_error", 500, &e.to_string())
    })?;
    Ok(wordless_kernel::base64url::encode(&bytes))
}

/// SHA-256 hex of a host token, for D1 storage and comparison.
pub fn token_hash(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex_encode(&hasher.finalize())
}

/// Constant-time-ish comparison of a presented token against a stored
/// hash. (Length check first; byte-wise compare — adequate for a
/// 256-bit random token behind a per-load rotation.)
pub fn token_matches(presented: &str, stored_hash: &str) -> bool {
    let presented_hash = token_hash(presented);
    presented_hash.len() == stored_hash.len()
        && presented_hash
            .bytes()
            .zip(stored_hash.bytes())
            .fold(0, |acc, (a, b)| acc | (a ^ b))
            == 0
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_token_is_deterministic_and_epoch_scoped() {
        let a = derive_host_token("secret", "hello-dolly", 0);
        let b = derive_host_token("secret", "hello-dolly", 0);
        assert_eq!(a, b);
        assert_ne!(a, derive_host_token("secret", "hello-dolly", 1));
        assert_ne!(a, derive_host_token("secret", "other", 0));
        assert_ne!(a, derive_host_token("other-secret", "hello-dolly", 0));
        // The derived token verifies against its stored hash.
        assert!(token_matches(&a, &token_hash(&a)));
    }

    #[test]
    fn token_round_trip_and_mismatch() {
        let token = generate_host_token().expect("token");
        assert_eq!(token.len(), 43); // 32 bytes base64url, no padding
        let hash = token_hash(&token);
        assert_eq!(hash.len(), 64);
        assert!(token_matches(&token, &hash));
        assert!(!token_matches("wrong-token", &hash));
        assert!(!token_matches(&token, "00"));
    }

    #[test]
    fn host_result_shapes() {
        let denied = HostResult::denied("options.read", "get_option");
        assert!(!denied.ok);
        assert_eq!(denied.error.as_ref().unwrap().code, "capability_denied");
        assert!(denied
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("get_option"));
        let ok = HostResult::ok(Value::String("x".to_string()));
        assert!(ok.ok);
    }
}
