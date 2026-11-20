//! Platform implementations of the provider seams — wasm32 only.
//!
//! This is the ONLY module in the crate that touches Worker types,
//! following the P2 rule that JS-environment access lives behind a
//! narrow interface so the rest of the code is plain Rust testable
//! with `cargo test` on native targets.
//!
//! **Workers AI path (measured 2026-11-19): the native `worker::Ai`
//! binding.** The pinned `worker` 0.9.0 crate exposes `worker::Ai`
//! with `Ai::run(model, input)` and `Env::ai("AI")` (vendored source
//! `worker-0.9.0/src/ai.rs`, `src/env.rs:44`). `WorkerAiBackend`
//! calls it directly. There is NO REST fallback for Workers AI: the
//! REST endpoint would need an account id + API token the Worker
//! does not have, and the binding makes it unnecessary.
//!
//! **OpenAI path: `worker::Fetch`** (`Fetch::Request(req).send()`),
//! the only HTTP option in Workers — `reqwest` does not work here
//! (no sockets). One honest gap vs v1: v1 applies a 30 s
//! `AbortSignal.timeout`; the pinned `worker` 0.9.0 `AbortSignal`
//! exposes no timeout constructor and the workspace `web-sys`
//! features do not include `AbortSignal`, so no client-side timeout
//! is applied — the request is still bounded by the Worker's own
//! runtime limits. The bearer key is set on the request headers only
//! and never logged or included in any error message.
//!
//! On non-wasm32 targets this module is empty: native tests use stub
//! backends instead. Live Workers AI calls are never observed in this
//! VM; nothing here claims otherwise.

#![cfg(target_arch = "wasm32")]

use async_trait::async_trait;
use serde_json::Value;
use wasm_bindgen::JsValue;
use wordless_kernel::error::WordlessError;
use worker::{Fetch, Headers, Method, Request, RequestInit};

use crate::providers::backend::{AiBackend, AiChatOutput, ChatMessage, HttpClient};
use crate::providers::types::{ai_provider_error, ProviderUsage};

/// Workers AI via the native `worker::Ai` binding.
pub struct WorkerAiBackend {
    ai: worker::Ai,
}

impl WorkerAiBackend {
    /// Wrap a binding obtained from `env.ai("AI")`.
    pub fn new(ai: worker::Ai) -> Self {
        Self { ai }
    }
}

#[async_trait(?Send)]
impl AiBackend for WorkerAiBackend {
    async fn run_chat(
        &self,
        model: &str,
        messages: &[ChatMessage],
        max_tokens: u32,
        temperature: f32,
    ) -> Result<AiChatOutput, WordlessError> {
        let input = serde_json::json!({
            "messages": messages,
            "max_tokens": max_tokens,
            "temperature": temperature,
        });
        let out: Value = self
            .ai
            .run(model, input)
            .await
            .map_err(|e| ai_provider_error(&format!("Workers AI call failed: {e}")))?;
        let response = out
            .get("response")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let usage = out.get("usage");
        Ok(AiChatOutput {
            response,
            usage: ProviderUsage {
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
}

/// Outbound HTTP via Workers `fetch`.
pub struct WorkerFetchHttp;

#[async_trait(?Send)]
impl HttpClient for WorkerFetchHttp {
    async fn post_json(
        &self,
        url: &str,
        bearer: &str,
        body: &Value,
    ) -> Result<(u16, String), WordlessError> {
        let mut init = RequestInit::new();
        init.with_method(Method::Post);
        let headers = Headers::new();
        headers
            .set("Content-Type", "application/json")
            .and_then(|()| headers.set("Authorization", &format!("Bearer {bearer}")))
            .map_err(|e| ai_provider_error(&format!("Failed to build request headers: {e}")))?;
        init.with_headers(headers);
        let body_json = serde_json::to_string(body)
            .map_err(|e| ai_provider_error(&format!("Failed to serialise request body: {e}")))?;
        init.with_body(Some(JsValue::from_str(&body_json)));
        let req = Request::new_with_init(url, &init)
            .map_err(|e| ai_provider_error(&format!("Failed to build request: {e}")))?;
        let mut resp = Fetch::Request(req)
            .send()
            .await
            .map_err(|e| ai_provider_error(&format!("OpenAI-compatible request failed: {e}")))?;
        let status = resp.status_code();
        let text = resp
            .text()
            .await
            .map_err(|e| ai_provider_error(&format!("Failed to read response body: {e}")))?;
        Ok((status, text))
    }
}
