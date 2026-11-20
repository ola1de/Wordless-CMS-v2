//! OpenAI-compatible provider — port of v1 `src/ai/providers/openai.ts`
//! (Milestone 6). OPTIONAL fallback only (user decision, 2026-11-19).
//! Workers AI is the default and only required provider; this one
//! exists for sites that already have an OpenAI-compatible key and
//! prefer it.
//!
//! The key comes ONLY from env/secrets (`OPENAI_API_KEY`, set with
//! `wrangler secret put OPENAI_API_KEY` — never in code, never in the
//! repo). The base URL is configurable (`OPENAI_BASE_URL`, default
//! `https://api.openai.com/v1`) so compatible endpoints work. Without
//! a key the provider reports `configured: false` and fails honestly.
//! The key is never logged and never appears in error messages.
//!
//! HTTP goes through the [`HttpClient`] trait, implemented on wasm32
//! by `crate::platform::WorkerFetchHttp` (Workers `fetch` — `reqwest`
//! does not work in Workers, no sockets). A 30 s timeout is applied
//! by the platform implementation where the runtime supports it; the
//! provider itself never embeds the key in a URL or an error.

use async_trait::async_trait;
use wordless_kernel::error::WordlessError;

use crate::env::AiEnv;
use crate::providers::types::{
    ai_not_configured, ai_provider_error, CompleteOptions, CompleteResult, Provider, ProviderId,
    ProviderProbe,
};

/// Default base URL for OpenAI-compatible chat endpoints.
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
/// Default model when neither the call nor the env sets one.
pub const DEFAULT_MODEL: &str = "gpt-4o-mini";

pub const NOT_CONFIGURED_MESSAGE: &str = "OpenAI-compatible provider is not configured: no OPENAI_API_KEY secret is set. Set it with `wrangler secret put OPENAI_API_KEY`, or use Workers AI (the default, no key needed).";

/// The OpenAI-compatible provider (unit struct — no state; the key and
/// the HTTP implementation come from `&AiEnv` per call).
pub struct OpenAiProvider;

#[async_trait(?Send)]
impl Provider for OpenAiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Openai
    }

    fn name(&self) -> &'static str {
        "OpenAI-compatible"
    }

    fn description(&self) -> &'static str {
        "Optional fallback only — any OpenAI-compatible chat endpoint. Needs an OPENAI_API_KEY secret; Workers AI is the default and needs no key."
    }

    fn recommended(&self) -> bool {
        false
    }

    fn requires_key(&self) -> bool {
        true
    }

    fn is_configured(&self, env: &AiEnv) -> bool {
        env.openai_api_key
            .as_deref()
            .map(|k| !k.is_empty())
            .unwrap_or(false)
    }

    fn configured_detail(&self, env: &AiEnv) -> String {
        if self.is_configured(env) {
            "OPENAI_API_KEY secret is set.".to_string()
        } else {
            "No OPENAI_API_KEY secret is set.".to_string()
        }
    }

    async fn complete(
        &self,
        env: &AiEnv,
        prompt: &str,
        opts: &CompleteOptions,
    ) -> Result<CompleteResult, WordlessError> {
        let Some(key) = env.openai_api_key.as_deref().filter(|k| !k.is_empty()) else {
            return Err(ai_not_configured(NOT_CONFIGURED_MESSAGE));
        };
        let Some(http) = &env.http else {
            return Err(ai_provider_error(
                "OpenAI-compatible provider has no HTTP implementation in this build, so it cannot complete a request.",
            ));
        };
        let base_url = env
            .openai_base_url
            .as_deref()
            .unwrap_or(DEFAULT_BASE_URL)
            .trim_end_matches('/')
            .to_string();
        let model = opts
            .model
            .clone()
            .or_else(|| env.openai_model.clone())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());
        let mut messages = Vec::new();
        if let Some(system) = &opts.system {
            if !system.is_empty() {
                messages.push(serde_json::json!({"role": "system", "content": system}));
            }
        }
        messages.push(serde_json::json!({"role": "user", "content": prompt}));
        let body = serde_json::json!({
            "model": model,
            "messages": messages,
            "max_tokens": opts.max_tokens.unwrap_or(1024),
            "temperature": opts.temperature.unwrap_or(0.7),
        });
        let (status, text) = http
            .post_json(&format!("{base_url}/chat/completions"), key, &body)
            .await
            .map_err(|e| {
                ai_provider_error(&format!("OpenAI-compatible request failed: {}", e.message))
            })?;
        if !(200..300).contains(&status) {
            // Never include the key or the raw body (it can echo credentials).
            return Err(ai_provider_error(&format!(
                "OpenAI-compatible provider returned HTTP {status}. Check the API key and base URL."
            )));
        }
        let parsed: serde_json::Value = serde_json::from_str(&text)
            .map_err(|_| ai_provider_error("OpenAI-compatible provider returned invalid JSON."))?;
        let content = parsed
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|c| c.first())
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .map(str::to_string);
        let Some(content) = content.filter(|c| !c.is_empty()) else {
            return Err(ai_provider_error(
                "OpenAI-compatible provider returned an empty response.",
            ));
        };
        let usage = parsed.get("usage");
        Ok(CompleteResult {
            text: content,
            model,
            usage: crate::providers::ProviderUsage {
                prompt_tokens: usage
                    .and_then(|u| u.get("prompt_tokens"))
                    .and_then(|v| v.as_u64()),
                completion_tokens: usage
                    .and_then(|u| u.get("completion_tokens"))
                    .and_then(|v| v.as_u64()),
                total_tokens: usage
                    .and_then(|u| u.get("total_tokens"))
                    .and_then(|v| v.as_u64()),
            },
        })
    }

    async fn embed(&self, _env: &AiEnv, _text: &str) -> Result<Vec<f64>, WordlessError> {
        Err(crate::providers::mock::embed_not_supported())
    }

    async fn test(&self, env: &AiEnv) -> ProviderProbe {
        if !self.is_configured(env) {
            return ProviderProbe {
                ok: false,
                message: NOT_CONFIGURED_MESSAGE.to_string(),
            };
        }
        match self
            .complete(
                env,
                "Reply with exactly the single word: ok",
                &CompleteOptions::new()
                    .with_max_tokens(8)
                    .with_temperature(0.0),
            )
            .await
        {
            Ok(result) => ProviderProbe {
                ok: true,
                message: format!("OpenAI-compatible provider responded ({}).", result.model),
            },
            Err(e) => ProviderProbe {
                ok: false,
                message: format!("OpenAI-compatible probe failed: {}", e.message),
            },
        }
    }
}
