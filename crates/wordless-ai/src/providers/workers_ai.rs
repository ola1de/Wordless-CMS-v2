//! Workers AI provider — port of v1 `src/ai/providers/workers-ai.ts`
//! (Milestone 6). The DEFAULT and only required provider (user
//! decision, 2026-11-19). No API key: it runs on the Cloudflare
//! account's Workers AI binding (`[ai]` in wrangler.toml).
//!
//! **Which path this crate uses (measured, 2026-11-19):** the native
//! `worker::Ai` binding. The pinned `worker` 0.9.0 crate exposes
//! `worker::Ai` with `Ai::run(model, input)` and `Env::ai("AI")`
//! (vendored source: `worker-0.9.0/src/ai.rs`, `src/env.rs:44`), so
//! no REST fallback is needed and none is implemented — a REST call
//! would need an account id + API token the Worker does not have.
//! The binding is reached through the [`AiBackend`] trait
//! (`crate::platform::WorkerAiBackend` on wasm32), so this file is
//! platform-free and unit-testable with a stub backend.
//!
//! The binding is OPTIONAL in wrangler.toml (commented out by
//! default, because no Cloudflare account is connected yet). When it
//! is absent the provider reports `configured: false` and every
//! completion fails with a clear "not configured" error — it never
//! crashes and never pretends. Live Workers AI calls cannot be
//! verified in this environment (no account, no binding), so this
//! provider is covered by interface-level tests (not configured
//! without the binding) plus the mock provider for behaviour.

use async_trait::async_trait;
use wordless_kernel::error::WordlessError;

use crate::env::AiEnv;
use crate::providers::backend::ChatMessage;
use crate::providers::types::{
    ai_not_configured, ai_provider_error, CompleteOptions, CompleteResult, Provider, ProviderId,
    ProviderProbe, ProviderUsage,
};

/// Default chat model. Override per call via `CompleteOptions.model`.
pub const WORKERS_AI_DEFAULT_MODEL: &str = "@cf/meta/llama-3-8b-instruct";

pub const NOT_CONFIGURED_MESSAGE: &str = "Workers AI is not configured. Uncomment the [ai] binding in wrangler.toml and deploy to Cloudflare, or select the mock provider for local development (canned responses only).";

/// The Workers AI provider (unit struct — no state; the binding comes
/// from `&AiEnv` per call).
pub struct WorkersAiProvider;

#[async_trait(?Send)]
impl Provider for WorkersAiProvider {
    fn id(&self) -> ProviderId {
        ProviderId::WorkersAi
    }

    fn name(&self) -> &'static str {
        "Workers AI (Cloudflare)"
    }

    fn description(&self) -> &'static str {
        "Cloudflare's built-in AI — the default and only required provider. No API key; runs on your Cloudflare account via the [ai] binding."
    }

    fn recommended(&self) -> bool {
        true
    }

    fn requires_key(&self) -> bool {
        false
    }

    fn is_configured(&self, env: &AiEnv) -> bool {
        env.workers_ai.is_some()
    }

    fn configured_detail(&self, env: &AiEnv) -> String {
        if self.is_configured(env) {
            format!("Workers AI binding is present (default model {WORKERS_AI_DEFAULT_MODEL}).")
        } else {
            "Workers AI binding is not present. Uncomment the [ai] binding in wrangler.toml and deploy to Cloudflare.".to_string()
        }
    }

    async fn complete(
        &self,
        env: &AiEnv,
        prompt: &str,
        opts: &CompleteOptions,
    ) -> Result<CompleteResult, WordlessError> {
        let Some(backend) = &env.workers_ai else {
            return Err(ai_not_configured(NOT_CONFIGURED_MESSAGE));
        };
        let model = opts
            .model
            .clone()
            .unwrap_or_else(|| WORKERS_AI_DEFAULT_MODEL.to_string());
        let mut messages = Vec::new();
        if let Some(system) = &opts.system {
            if !system.is_empty() {
                messages.push(ChatMessage::system(system.clone()));
            }
        }
        messages.push(ChatMessage::user(prompt.to_string()));
        let out = backend
            .run_chat(
                &model,
                &messages,
                opts.max_tokens.unwrap_or(1024),
                opts.temperature.unwrap_or(0.7),
            )
            .await
            .map_err(|e| {
                if e.code == "ai_not_configured" {
                    e
                } else {
                    ai_provider_error(&format!("Workers AI call failed: {}", e.message))
                }
            })?;
        if out.response.is_empty() {
            return Err(ai_provider_error("Workers AI returned an empty response."));
        }
        Ok(CompleteResult {
            text: out.response,
            model,
            usage: ProviderUsage {
                prompt_tokens: out.usage.prompt_tokens,
                completion_tokens: out.usage.completion_tokens,
                total_tokens: out.usage.total_tokens,
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
                message: format!("Workers AI responded ({}).", result.model),
            },
            Err(e) => ProviderProbe {
                ok: false,
                message: format!("Workers AI probe failed: {}", e.message),
            },
        }
    }
}
