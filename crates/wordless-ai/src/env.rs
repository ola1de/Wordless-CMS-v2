//! The AI environment: everything a provider needs that comes from
//! outside the database.
//!
//! This is plain data — no Worker types. The platform module
//! (`crate::platform`, wasm32-only) knows how to build one from a
//! `worker::Env`; tests and local dev build one by hand. Secrets are
//! never logged and never stored in the database; the OpenAI key lives
//! only here, for the duration of one request.
//!
//! - `workers_ai`: present when the `[ai]` binding is configured in
//!   `wrangler.toml`. The backend behind it is the native
//!   `worker::Ai` binding (see `crate::platform::WorkerAiBackend`).
//! - `openai_api_key`: the `OPENAI_API_KEY` secret, when set.
//! - `http`: the outbound fetch implementation (Workers `fetch` on
//!   wasm32, a test stub elsewhere). Without it, the OpenAI provider
//!   reports configured (the key is set) but cannot complete —
//!   exactly like a Worker without network would.

use std::rc::Rc;

use crate::providers::backend::{AiBackend, HttpClient};

/// Environment a provider call runs in. Cheap to clone (handles are
/// reference-counted); the OpenAI key string is the only owned secret.
#[derive(Clone, Default)]
pub struct AiEnv {
    /// Workers AI backend, when the `[ai]` binding is present.
    pub workers_ai: Option<Rc<dyn AiBackend>>,
    /// `OPENAI_API_KEY` secret, when set.
    pub openai_api_key: Option<String>,
    /// `OPENAI_BASE_URL` var; defaults to `https://api.openai.com/v1`.
    pub openai_base_url: Option<String>,
    /// `OPENAI_MODEL` var; defaults to `gpt-4o-mini`.
    pub openai_model: Option<String>,
    /// Outbound HTTP implementation for the OpenAI-compatible provider.
    pub http: Option<Rc<dyn HttpClient>>,
}

impl AiEnv {
    /// Empty environment: no provider is configured. This is what
    /// tests use, and what a Worker without the `[ai]` binding and
    /// without an `OPENAI_API_KEY` secret looks like.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Attach a Workers AI backend (the `[ai]` binding).
    pub fn with_workers_ai(mut self, backend: Rc<dyn AiBackend>) -> Self {
        self.workers_ai = Some(backend);
        self
    }

    /// Attach an OpenAI-compatible key (the `OPENAI_API_KEY` secret).
    pub fn with_openai_key(mut self, key: impl Into<String>) -> Self {
        self.openai_api_key = Some(key.into());
        self
    }

    /// Override the OpenAI-compatible base URL.
    pub fn with_openai_base_url(mut self, url: impl Into<String>) -> Self {
        self.openai_base_url = Some(url.into());
        self
    }

    /// Override the OpenAI-compatible model.
    pub fn with_openai_model(mut self, model: impl Into<String>) -> Self {
        self.openai_model = Some(model.into());
        self
    }

    /// Attach the outbound HTTP implementation (Workers `fetch`).
    pub fn with_http(mut self, http: Rc<dyn HttpClient>) -> Self {
        self.http = Some(http);
        self
    }

    /// Build from a live Worker `Env` (wasm32 only): the `[ai]`
    /// binding when present, the `OPENAI_API_KEY` / `OPENAI_BASE_URL` /
    /// `OPENAI_MODEL` vars when set, and the Workers `fetch`
    /// implementation for outbound calls. A missing binding or var is
    /// simply absent — never an error — so the providers can report
    /// their honest "not configured" state.
    #[cfg(target_arch = "wasm32")]
    pub fn from_worker_env(env: &worker::Env) -> Self {
        use crate::platform::{WorkerAiBackend, WorkerFetchHttp};

        let mut out = Self::empty();
        if let Ok(ai) = env.ai("AI") {
            out.workers_ai = Some(Rc::new(WorkerAiBackend::new(ai)));
        }
        if let Ok(key) = env.var("OPENAI_API_KEY") {
            let key = key.to_string();
            if !key.is_empty() {
                out.openai_api_key = Some(key);
            }
        }
        if let Ok(url) = env.var("OPENAI_BASE_URL") {
            let url = url.to_string();
            if !url.is_empty() {
                out.openai_base_url = Some(url);
            }
        }
        if let Ok(model) = env.var("OPENAI_MODEL") {
            let model = model.to_string();
            if !model.is_empty() {
                out.openai_model = Some(model);
            }
        }
        out.http = Some(Rc::new(WorkerFetchHttp));
        out
    }
}
