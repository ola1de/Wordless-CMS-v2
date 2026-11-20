//! Provider abstraction — port of v1 `src/ai/providers/types.ts`
//! (Milestone 6).
//!
//! Providers are the only place that talks to an AI model. Everything
//! else (orchestrator, agents, command centre) talks to this interface,
//! so a site can switch providers without touching agent code.
//!
//! DEFAULT PROVIDER (user decision, 2026-11-19): Cloudflare Workers
//! AI. It needs no API key — only the `[ai]` binding in wrangler.toml.
//! The OpenAI-compatible provider stays in code as an OPTIONAL
//! fallback only. The mock provider backs tests and local development
//! with deterministic canned responses — it is never presented as real
//! AI.
//!
//! Honesty rule: a provider that is not configured reports
//! `configured: false` and any completion attempt fails with a clear
//! "not configured" error. Nothing in this layer ever pretends AI
//! works when no provider is configured, and no API key or secret is
//! ever stored in code — keys come only from env/secrets bindings.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use wordless_kernel::error::WordlessError;

use crate::env::AiEnv;

/// Provider ids, in v1 registry order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderId {
    WorkersAi,
    Openai,
    Mock,
}

impl ProviderId {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderId::WorkersAi => "workers-ai",
            ProviderId::Openai => "openai",
            ProviderId::Mock => "mock",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<ProviderId> {
        match s {
            "workers-ai" => Some(ProviderId::WorkersAi),
            "openai" => Some(ProviderId::Openai),
            "mock" => Some(ProviderId::Mock),
            _ => None,
        }
    }
}

/// All provider ids, in registry order.
pub const PROVIDER_IDS: [ProviderId; 3] =
    [ProviderId::WorkersAi, ProviderId::Openai, ProviderId::Mock];

/// The default provider when the site has never chosen one.
pub const DEFAULT_PROVIDER_ID: ProviderId = ProviderId::WorkersAi;

pub fn is_provider_id(value: &str) -> bool {
    ProviderId::from_str(value).is_some()
}

/// Token usage reported by a provider, when it reports any.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
}

/// Options for one completion call.
#[derive(Debug, Clone, Default)]
pub struct CompleteOptions {
    /// Override the provider's default model (Workers AI / OpenAI only).
    pub model: Option<String>,
    /// System prompt prepended to the user prompt.
    pub system: Option<String>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

impl CompleteOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }
}

/// One completed completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompleteResult {
    pub text: String,
    pub model: String,
    pub usage: ProviderUsage,
}

/// A tiny probe result: never an error for "not configured", it
/// reports it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderProbe {
    pub ok: bool,
    pub message: String,
}

/// Public provider info for the command centre.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub id: ProviderId,
    pub name: String,
    /// Short honest description shown in the command centre.
    pub description: String,
    /// True when the provider can actually complete a request right now.
    pub configured: bool,
    /// Why it is (not) configured, in plain language.
    pub configured_detail: String,
    /// Workers AI is the recommended/default route (user decision).
    pub recommended: bool,
    /// True when the provider needs an API key from env/secrets.
    pub requires_key: bool,
}

/// A provider: the only interface the rest of the AI system uses to
/// talk to a model. Implementations are unit structs; everything that
/// varies by deployment (bindings, keys) comes from `&AiEnv` per call,
/// so the same provider value is safe to share across requests.
#[async_trait(?Send)]
pub trait Provider {
    fn id(&self) -> ProviderId;
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn recommended(&self) -> bool;
    fn requires_key(&self) -> bool;
    /// True only when a completion call could succeed right now.
    fn is_configured(&self, env: &AiEnv) -> bool;
    /// Plain-language reason for the current configured state.
    fn configured_detail(&self, env: &AiEnv) -> String;
    /// Complete one prompt. Fails with `ai_not_configured` (503) when
    /// the provider is not configured, `ai_provider_error` (502) when
    /// the model call itself fails.
    async fn complete(
        &self,
        env: &AiEnv,
        prompt: &str,
        opts: &CompleteOptions,
    ) -> Result<CompleteResult, WordlessError>;
    /// Optional embeddings (used by website memory search in a later
    /// milestone). The default implementation reports "not supported"
    /// instead of faking vectors.
    async fn embed(&self, env: &AiEnv, text: &str) -> Result<Vec<f64>, WordlessError>;
    /// Small probe: mock always passes; real providers report their
    /// real state. Never fails for "not configured" — it reports it.
    async fn test(&self, env: &AiEnv) -> ProviderProbe;
}

/// `ai_not_configured` (503): no provider is configured, or the
/// selected provider cannot run here. The message tells the admin what
/// to do; it never contains a secret.
pub fn ai_not_configured(message: &str) -> WordlessError {
    WordlessError::new("ai_not_configured", 503, message)
}

/// `ai_provider_error` (502): the provider was configured but the
/// model call failed. The message never contains a key or a raw
/// response body (either can echo credentials).
pub fn ai_provider_error(message: &str) -> WordlessError {
    WordlessError::new("ai_provider_error", 502, message)
}
