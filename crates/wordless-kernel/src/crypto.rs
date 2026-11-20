//! Platform crypto — one interface, two engines.
//!
//! * `pbkdf2_sha256` — the PRIMARY engine. On wasm32 it calls WebCrypto
//!   (`SubtleCrypto`) — native speed, exactly what v1 uses. On native
//!   targets it is the pure-Rust implementation (there is no WebCrypto
//!   in a plain test binary).
//! * `pbkdf2_sha256_rust` — pure-Rust PBKDF2-HMAC-SHA256 on EVERY target.
//!   On wasm32 it is the fallback engine, and it is what the WASM
//!   timing test measures (doc §7 R3: pure-Rust PBKDF2 at 100k
//!   iterations vs the 10 ms free-tier CPU limit).
//! * `sha256_hex` — SHA-256 hex digest (WebCrypto on wasm32, `sha2`
//!   elsewhere). Only the token hash is stored, never the token.
//! * `random_bytes` — `getrandom` (JS `crypto.getRandomValues` on
//!   wasm32, OS RNG elsewhere). Session tokens and salts depend on it.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("platform crypto is unavailable: {0}")]
    Unavailable(String),
    #[error("crypto operation failed: {0}")]
    Failed(String),
}

/// PBKDF2-HMAC-SHA256, pure Rust. Same output as WebCrypto for the same
/// inputs (verified by the cross-engine test below on native, and by
/// the WASM test against the Node-generated vector).
pub fn pbkdf2_sha256_rust(password: &str, salt: &[u8], iterations: u32, out_len: usize) -> Vec<u8> {
    use pbkdf2::pbkdf2_hmac;
    use sha2::Sha256;
    let mut out = vec![0u8; out_len];
    pbkdf2_hmac::<Sha256>(password.as_bytes(), salt, iterations, &mut out);
    out
}

#[cfg(target_arch = "wasm32")]
mod wasm_crypto {
    use super::CryptoError;
    use js_sys::Uint8Array;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    fn subtle() -> Result<web_sys::SubtleCrypto, CryptoError> {
        let global = js_sys::global();
        let crypto_val = js_sys::Reflect::get(&global, &wasm_bindgen::JsValue::from_str("crypto"))
            .map_err(|e| CryptoError::Unavailable(format!("no global crypto: {e:?}")))?;
        let crypto: web_sys::Crypto = crypto_val
            .dyn_into()
            .map_err(|_| CryptoError::Unavailable("global crypto is not a Crypto object".into()))?;
        Ok(crypto.subtle())
    }

    pub async fn pbkdf2_sha256(
        password: &str,
        salt: &[u8],
        iterations: u32,
        out_len: usize,
    ) -> Result<Vec<u8>, CryptoError> {
        let subtle = subtle()?;
        // importKey("raw", <password bytes>, "PBKDF2", false, ["deriveBits"])
        // NOTE: web-sys's `import_key_with_str` takes the ALGORITHM as
        // the string; the key data is a Uint8Array (an Object).
        let key_data = Uint8Array::from(password.as_bytes());
        let usages = {
            let arr = js_sys::Array::new();
            arr.push(&wasm_bindgen::JsValue::from_str("deriveBits"));
            wasm_bindgen::JsValue::from(arr)
        };
        let key = JsFuture::from(
            subtle
                .import_key_with_str("raw", &key_data, "PBKDF2", false, &usages)
                .map_err(|e| CryptoError::Failed(format!("importKey failed: {e:?}")))?,
        )
        .await
        .map_err(|e| CryptoError::Failed(format!("importKey failed: {e:?}")))?;
        let key: web_sys::CryptoKey = key
            .dyn_into()
            .map_err(|_| CryptoError::Failed("importKey did not return a CryptoKey".into()))?;

        let params = js_sys::Object::new();
        js_sys::Reflect::set(&params, &"name".into(), &"PBKDF2".into())
            .map_err(|e| CryptoError::Failed(format!("{e:?}")))?;
        js_sys::Reflect::set(&params, &"salt".into(), &Uint8Array::from(salt))
            .map_err(|e| CryptoError::Failed(format!("{e:?}")))?;
        js_sys::Reflect::set(&params, &"iterations".into(), &iterations.into())
            .map_err(|e| CryptoError::Failed(format!("{e:?}")))?;
        js_sys::Reflect::set(&params, &"hash".into(), &"SHA-256".into())
            .map_err(|e| CryptoError::Failed(format!("{e:?}")))?;

        let bits = JsFuture::from(
            subtle
                .derive_bits_with_object(&params, &key, (out_len * 8) as u32)
                .map_err(|e| CryptoError::Failed(format!("deriveBits failed: {e:?}")))?,
        )
        .await
        .map_err(|e| CryptoError::Failed(format!("deriveBits failed: {e:?}")))?;
        Ok(Uint8Array::new(&bits).to_vec())
    }

    pub async fn sha256_hex(data: &[u8]) -> Result<String, CryptoError> {
        let subtle = subtle()?;
        let input = Uint8Array::from(data);
        let digest = JsFuture::from(
            subtle
                .digest_with_str_and_buffer_source("SHA-256", &input)
                .map_err(|e| CryptoError::Failed(format!("digest failed: {e:?}")))?,
        )
        .await
        .map_err(|e| CryptoError::Failed(format!("digest failed: {e:?}")))?;
        let bytes = Uint8Array::new(&digest).to_vec();
        Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }
}

/// Primary PBKDF2: WebCrypto on wasm32, pure Rust on native targets.
pub async fn pbkdf2_sha256(
    password: &str,
    salt: &[u8],
    iterations: u32,
    out_len: usize,
) -> Result<Vec<u8>, CryptoError> {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_crypto::pbkdf2_sha256(password, salt, iterations, out_len).await
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        Ok(pbkdf2_sha256_rust(password, salt, iterations, out_len))
    }
}

/// SHA-256 hex digest.
pub async fn sha256_hex(data: &[u8]) -> Result<String, CryptoError> {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_crypto::sha256_hex(data).await
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(data);
        Ok(hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect())
    }
}

/// `n` cryptographically random bytes.
pub fn random_bytes(n: usize) -> Result<Vec<u8>, CryptoError> {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf)
        .map_err(|e| CryptoError::Unavailable(format!("getrandom failed: {e}")))?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base64url;

    /// Test vector generated with Node `crypto.pbkdf2Sync` in the v1
    /// format (PBKDF2-HMAC-SHA256, 100,000 iterations, 16-byte salt,
    /// 32-byte hash):
    ///   password = "correct-horse-battery-staple"
    ///   salt (base64url) = "AAECAwQFBgcICQoLDA0ODw"
    ///   hash (base64url) = <see below>
    /// Generated 2026-11-19 with:
    ///   node -e "const c=require('crypto');
    ///     const salt=Buffer.from([0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15]);
    ///     const h=c.pbkdf2Sync('correct-horse-battery-staple',salt,100000,32,'sha256');
    ///     console.log(h.toString('base64url'))"
    const VECTOR_PASSWORD: &str = "correct-horse-battery-staple";
    const VECTOR_SALT_B64: &str = "AAECAwQFBgcICQoLDA0ODw";
    const VECTOR_HASH_B64: &str = "B0ykfR9n7-dmQoY_sDnYh1fCuNlfUm6pcVo49btIvh8";

    #[test]
    fn pure_rust_pbkdf2_matches_node_vector() {
        let salt = base64url::decode(VECTOR_SALT_B64).unwrap();
        let expected = base64url::decode(VECTOR_HASH_B64).unwrap();
        let actual = pbkdf2_sha256_rust(VECTOR_PASSWORD, &salt, 100_000, 32);
        assert_eq!(
            actual, expected,
            "pure-Rust PBKDF2 does not match the Node vector"
        );
    }

    /// Timing of the pure-Rust engine on the native target (x86_64).
    /// Printed, not asserted — the number is a measurement, and the
    /// WASM number (the one that matters for the 10 ms limit) comes
    /// from the wasm test below. Run with `cargo test -- --nocapture`.
    #[test]
    fn pure_rust_pbkdf2_native_timing() {
        let salt = base64url::decode(VECTOR_SALT_B64).unwrap();
        let mut times = Vec::new();
        for _ in 0..3 {
            let start = std::time::Instant::now();
            let _ = pbkdf2_sha256_rust(VECTOR_PASSWORD, &salt, 100_000, 32);
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        let mean = times.iter().sum::<f64>() / times.len() as f64;
        println!("pure-Rust PBKDF2 100k iters, native x86_64: {mean:.1} ms (runs: {times:?})");
    }
}

/// WASM-only tests, run with `wasm-pack test --node` in wordless-kernel.
/// These measure the two engines where it matters: inside WebAssembly.
/// The WebCrypto test also proves the wasm path produces the same bytes
/// as Node (the same vector the native test checks).
#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use super::*;
    use crate::base64url;
    use wasm_bindgen_test::wasm_bindgen_test;

    const VECTOR_PASSWORD: &str = "correct-horse-battery-staple";
    const VECTOR_SALT_B64: &str = "AAECAwQFBgcICQoLDA0ODw";
    const VECTOR_HASH_B64: &str = "B0ykfR9n7-dmQoY_sDnYh1fCuNlfUm6pcVo49btIvh8";

    #[wasm_bindgen_test]
    async fn webcrypto_pbkdf2_matches_node_vector_and_timing() {
        let salt = base64url::decode(VECTOR_SALT_B64).unwrap();
        let expected = base64url::decode(VECTOR_HASH_B64).unwrap();
        let start = js_sys::Date::now();
        let actual = pbkdf2_sha256(VECTOR_PASSWORD, &salt, 100_000, 32)
            .await
            .expect("WebCrypto PBKDF2 failed in WASM");
        let elapsed = js_sys::Date::now() - start;
        assert_eq!(
            actual, expected,
            "WebCrypto PBKDF2 does not match the Node vector"
        );
        web_sys::console::log_1(
            &format!("webcrypto pbkdf2 100k iters, wasm32: {elapsed:.1} ms").into(),
        );
    }

    #[wasm_bindgen_test]
    fn rust_pbkdf2_wasm_timing() {
        let salt = base64url::decode(VECTOR_SALT_B64).unwrap();
        let mut times = Vec::new();
        for _ in 0..3 {
            let start = js_sys::Date::now();
            let actual = pbkdf2_sha256_rust(VECTOR_PASSWORD, &salt, 100_000, 32);
            times.push(js_sys::Date::now() - start);
            let expected = base64url::decode(VECTOR_HASH_B64).unwrap();
            assert_eq!(actual, expected);
        }
        let mean = times.iter().sum::<f64>() / times.len() as f64;
        web_sys::console::log_1(
            &format!("pure-Rust pbkdf2 100k iters, wasm32: {mean:.1} ms (runs: {times:?})").into(),
        );
    }
}
