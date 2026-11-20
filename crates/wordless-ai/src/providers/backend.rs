//! Platform seams for the real providers.
//!
//! The provider implementations in this crate are pure logic over two
//! tiny traits, so they run under plain `cargo test` with stubs:
//!
//! - [`AiBackend`]: one Workers AI chat call. On wasm32 the platform
//!   module implements it with the native `worker::Ai` binding
//!   (`Ai::run`); there is deliberately NO REST fallback for Workers
//!   AI — the binding exists in the pinned `worker` 0.9.0 crate (see
//!   `crate::platform`), and a REST call would need an account id +
//!   token the Worker does not have.
//! - [`HttpClient`]: one authenticated JSON POST, for the
//!   OpenAI-compatible provider. On wasm32 the platform module
//!   implements it with `worker::Fetch` — `reqwest` does not work in
//!   Workers (no sockets), so it is never used.
//!
//! Both traits are `?Send` (the Workers runtime is single-threaded;
//! `JsFuture` is `!Send`), matching the `wordless-db` `Db` trait.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use wordless_kernel::error::WordlessError;

use crate::providers::ProviderUsage;

/// One chat message, in Workers AI / OpenAI wire shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }
}

/// What a Workers AI chat call returns: the text plus token usage
/// when the platform reports it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiChatOutput {
    pub response: String,
    #[serde(default)]
    pub usage: ProviderUsage,
}

/// One Workers AI chat call. Implemented by
/// `crate::platform::WorkerAiBackend` (native `worker::Ai` binding)
/// on wasm32, and by test stubs on native targets.
#[async_trait(?Send)]
pub trait AiBackend {
    /// Run `model` with `messages`. `max_tokens` and `temperature`
    /// are passed through to the model.
    async fn run_chat(
        &self,
        model: &str,
        messages: &[ChatMessage],
        max_tokens: u32,
        temperature: f32,
    ) -> Result<AiChatOutput, WordlessError>;
}

/// One authenticated JSON POST. Implemented by
/// `crate::platform::WorkerFetchHttp` (Workers `fetch`) on wasm32,
/// and by test stubs on native targets.
///
/// Returns the HTTP status and the raw response body. The caller
/// parses the body and decides what a non-2xx status means; the
/// implementation must never log the bearer token or the body.
#[async_trait(?Send)]
pub trait HttpClient {
    async fn post_json(
        &self,
        url: &str,
        bearer: &str,
        body: &serde_json::Value,
    ) -> Result<(u16, String), WordlessError>;
}
