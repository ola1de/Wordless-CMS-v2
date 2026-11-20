//! Mock provider — port of v1 `src/ai/providers/mock.ts` (Milestone 6).
//!
//! Deterministic canned responses. Used by tests and by local
//! development when no real provider is configured. It is NEVER
//! presented as real AI: the command centre labels it "Mock (local
//! development only — canned responses, no real AI)" and every result
//! it produces is deterministic for a given prompt, so tests can
//! assert on it.
//!
//! Determinism: the response is built from the prompt itself (a short
//! excerpt + a stable hash), never from randomness or the clock.

use async_trait::async_trait;
use wordless_kernel::error::WordlessError;

use crate::env::AiEnv;
use crate::providers::types::{
    ai_provider_error, CompleteOptions, CompleteResult, Provider, ProviderId, ProviderProbe,
    ProviderUsage,
};

/// Model name the mock reports, mirroring v1 `MOCK_MODEL`.
pub const MOCK_MODEL: &str = "mock-1.0";

/// Tiny FNV-1a hash, stable across runs (no randomness, no clock).
/// Port of v1 `stableHash` in `mock.ts`, over UTF-8 bytes (v1 hashes
/// UTF-16 code units, so values agree with v1 for ASCII prompts and
/// differ for non-ASCII ones; determinism — same prompt, same hash —
/// holds either way).
pub fn stable_hash(text: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for b in text.bytes() {
        hash ^= u32::from(b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// The mock provider (unit struct — no state).
pub struct MockProvider;

#[async_trait(?Send)]
impl Provider for MockProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Mock
    }

    fn name(&self) -> &'static str {
        "Mock provider"
    }

    fn description(&self) -> &'static str {
        "Local development only — deterministic canned responses, no real AI. Use it for tests and for trying the command centre without a Cloudflare account."
    }

    fn recommended(&self) -> bool {
        false
    }

    fn requires_key(&self) -> bool {
        false
    }

    fn is_configured(&self, _env: &AiEnv) -> bool {
        true
    }

    fn configured_detail(&self, _env: &AiEnv) -> String {
        "Always available in local development and tests. Canned responses only.".to_string()
    }

    async fn complete(
        &self,
        _env: &AiEnv,
        prompt: &str,
        opts: &CompleteOptions,
    ) -> Result<CompleteResult, WordlessError> {
        let flat: String = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
        let excerpt: String = flat.chars().take(120).collect();
        let hash = stable_hash(prompt);
        let ellipsis = if prompt.len() > 120 { "…" } else { "" };
        let text = format!(
            "[Mock AI draft — not a real model response]\nRequest #{hash} (prompt: \"{excerpt}{ellipsis}\").\nThis is a deterministic placeholder so the orchestrator, approvals and command centre can be exercised without a configured provider. Configure Workers AI (the default) to get a real response."
        );
        Ok(CompleteResult {
            text: text.clone(),
            model: opts.model.clone().unwrap_or_else(|| MOCK_MODEL.to_string()),
            usage: ProviderUsage {
                prompt_tokens: Some(prompt.len() as u64),
                completion_tokens: Some(text.len() as u64),
                total_tokens: Some((prompt.len() + text.len()) as u64),
            },
        })
    }

    async fn embed(&self, _env: &AiEnv, text: &str) -> Result<Vec<f64>, WordlessError> {
        // Deterministic 8-dimensional pseudo-embedding from the text hash.
        let mut out = Vec::with_capacity(8);
        let mut h = stable_hash(text);
        for _ in 0..8 {
            h = h.wrapping_mul(0x0100_0193) ^ (h >> 13);
            out.push(((h % 1000) as f64 / 1000.0) * 2.0 - 1.0);
        }
        Ok(out)
    }

    async fn test(&self, _env: &AiEnv) -> ProviderProbe {
        ProviderProbe {
            ok: true,
            message: "Mock provider is always available (canned responses, no real AI)."
                .to_string(),
        }
    }
}

/// The default `embed` implementation on the `Provider` trait reports
/// "not supported" for providers without embeddings; the mock is the
/// only provider that implements it. This helper keeps that rule in
/// one place for providers that do not override `embed`.
pub fn embed_not_supported() -> WordlessError {
    ai_provider_error("This provider does not support embeddings.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::types::ProviderId;

    #[test]
    fn stable_hash_matches_fnv1a_vectors() {
        // FNV-1a 32-bit: empty string is the offset basis.
        assert_eq!(stable_hash(""), 0x811c_9dc5);
        assert_eq!(stable_hash("hello"), stable_hash("hello"));
        assert_ne!(stable_hash("hello"), stable_hash("world"));
    }

    #[test]
    fn mock_model_name() {
        assert_eq!(MOCK_MODEL, "mock-1.0");
        assert_eq!(MockProvider.id(), ProviderId::Mock);
    }
}
