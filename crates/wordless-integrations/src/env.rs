//! Environment inputs as plain data (no Worker types).
//!
//! The worker entry builds these from its `Env` bindings and passes
//! them into the service functions, so every function in this crate
//! stays testable on native targets. Secrets travel only in these
//! structs — they are never written to D1, never logged, and never
//! stored in config (see [`crate::config`]).

/// Secrets read from the Worker environment (`wrangler secret`).
/// `None`/empty means "not set".
#[derive(Debug, Clone, Default)]
pub struct EnvSecrets {
    /// `EMAIL_API_KEY` — Bearer key for the generic HTTP email
    /// provider. The URL itself is non-secret config
    /// (`wl_integration_email.apiUrl`).
    pub email_api_key: Option<String>,
    /// `PAYSTACK_SECRET_KEY`.
    pub paystack_secret_key: Option<String>,
    /// `FLUTTERWAVE_SECRET_KEY`.
    pub flutterwave_secret_key: Option<String>,
    /// `STRIPE_SECRET_KEY`.
    pub stripe_secret_key: Option<String>,
}

impl EnvSecrets {
    /// True when the value is present and non-empty.
    pub fn has(&self, key: &Option<String>) -> bool {
        key.as_deref().map(|s| !s.is_empty()).unwrap_or(false)
    }
}

/// Which Worker bindings are present in this environment, as plain
/// booleans. A binding is present only when the env object has it —
/// nothing is invented (v1 `cloudflare.ts`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Bindings {
    /// `DB` (D1).
    pub d1: bool,
    /// `MEDIA` (R2).
    pub r2: bool,
    /// `CONFIG` (KV).
    pub kv: bool,
    /// `ASSETS` (Static Assets).
    pub assets: bool,
    /// `AI` (Workers AI) — absent until the user deploys with their
    /// own account.
    pub ai: bool,
}
