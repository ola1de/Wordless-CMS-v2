//! Integration registry (port of v1 `registry.ts`).
//!
//! One definition per integration: what it is, what kind it is, what
//! it needs, and how to tell whether it is configured — computed live
//! from env + stored config, never hardcoded. The admin Integrations
//! page and `GET /api/integrations` both read this list, so they can
//! never disagree about status.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use wordless_db::Db;
use wordless_kernel::WordlessError;

use crate::{
    config::{get_integration_config, INTEGRATION_IDS},
    env::{Bindings, EnvSecrets},
};

/// Integration kind (v1 `IntegrationKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IntegrationKind {
    Email,
    Payment,
    Analytics,
    Social,
    Platform,
    Marketplace,
}

/// One integration with its live configured state + stored config
/// (non-secret fields only).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationStatus {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: IntegrationKind,
    pub description: &'static str,
    /// What the admin must do to make it work (plain language).
    pub setup_hint: &'static str,
    pub configured: bool,
    pub configured_detail: String,
    pub config: Map<String, Value>,
}

/// Static definition of one integration (v1 `INTEGRATIONS`).
struct Definition {
    id: &'static str,
    name: &'static str,
    kind: IntegrationKind,
    description: &'static str,
    setup_hint: &'static str,
}

const DEFINITIONS: [Definition; 8] = [
    Definition {
        id: "email",
        name: "Email",
        kind: IntegrationKind::Email,
        description: "Transactional email (order confirmations, comment notifications, password resets).",
        setup_hint: "Set EMAIL_API_URL in the config below and EMAIL_API_KEY with `wrangler secret put EMAIL_API_KEY`. Until then, messages are logged locally (test mode) and never delivered.",
    },
    Definition {
        id: "paystack",
        name: "Paystack",
        kind: IntegrationKind::Payment,
        description: "Payments — cards, bank transfer, mobile money (NGN and more).",
        setup_hint: "Set PAYSTACK_SECRET_KEY with `wrangler secret put PAYSTACK_SECRET_KEY`, then select Paystack in Commerce → Payments.",
    },
    Definition {
        id: "flutterwave",
        name: "Flutterwave",
        kind: IntegrationKind::Payment,
        description: "Payments — cards, bank transfer, mobile money across Africa.",
        setup_hint: "Set FLUTTERWAVE_SECRET_KEY and FLUTTERWAVE_SECRET_HASH with `wrangler secret put`, then select Flutterwave in Commerce → Payments.",
    },
    Definition {
        id: "stripe",
        name: "Stripe",
        kind: IntegrationKind::Payment,
        description: "Payments — cards and wallets worldwide.",
        setup_hint: "Set STRIPE_SECRET_KEY and STRIPE_WEBHOOK_SECRET with `wrangler secret put`, then select Stripe in Commerce → Payments.",
    },
    Definition {
        id: "analytics",
        name: "Analytics",
        kind: IntegrationKind::Analytics,
        description: "Page-view analytics (Google Analytics 4 or Plausible). Counts live in the provider's dashboard, never in Wordless.",
        setup_hint: "Choose a provider and enter its measurement ID (GA4) or domain (Plausible) in the config below. No snippet is injected until then.",
    },
    Definition {
        id: "social",
        name: "Social sharing",
        kind: IntegrationKind::Social,
        description: "Share links on posts and products (X, Facebook, WhatsApp, Telegram). Plain links — no API, no share counts.",
        setup_hint: "No setup needed. Share links are always available.",
    },
    Definition {
        id: "cloudflare",
        name: "Cloudflare",
        kind: IntegrationKind::Platform,
        description: "Worker bindings in this environment (D1, R2, KV, Static Assets, Workers AI). Read-only status.",
        setup_hint: "Bindings come from wrangler.toml and your Cloudflare account. The AI binding appears after deploying with your own account.",
    },
    Definition {
        id: "marketplace",
        name: "Marketplace",
        kind: IntegrationKind::Marketplace,
        description: "Extension marketplace (skeleton): install extensions from a ZIP upload, activate/deactivate them. No remote directory yet.",
        setup_hint: "Upload an extension ZIP (with a wordless.json manifest) in Commerce → Marketplace. A remote directory is a later milestone.",
    },
];

fn config_str(config: &Map<String, Value>, key: &str) -> String {
    config
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn payment_state(present: bool, set_detail: &str, unset_detail: &str) -> (bool, String) {
    if present {
        (true, set_detail.to_string())
    } else {
        (false, unset_detail.to_string())
    }
}

/// The full integration list with live configured state + stored
/// config (non-secret fields only).
///
/// `secrets` carries the environment secrets (presence only — values
/// are never copied into the output); `bindings` carries Worker
/// binding presence for the Cloudflare entry. Payment configured
/// state mirrors v1's provider `isConfigured` /
/// `configuredDetail` (secret key set via `wrangler secret`).
/// The payment providers themselves live in `wordless-commerce`
/// (P3, parallel crate); this registry only reports the same
/// secret-presence rule v1's providers use, so the two can never
/// disagree about "configured".
///
/// # Errors
/// Returns `Err` only on a database failure.
pub async fn list_integrations(
    db: &dyn Db,
    secrets: &EnvSecrets,
    bindings: &Bindings,
) -> Result<Vec<IntegrationStatus>, WordlessError> {
    debug_assert_eq!(INTEGRATION_IDS.len(), DEFINITIONS.len());
    let mut out = Vec::with_capacity(DEFINITIONS.len());
    for def in &DEFINITIONS {
        let config = get_integration_config(db, def.id).await?;
        let (configured, detail) = match def.id {
            "email" => {
                let api_url = config_str(&config, "apiUrl");
                if !api_url.is_empty() && secrets.has(&secrets.email_api_key) {
                    (
                        true,
                        format!("HTTP email provider configured ({api_url}). Messages are delivered and logged."),
                    )
                } else {
                    (
                        false,
                        "No email provider is configured — messages are recorded in the mail log (test mode), not delivered.".to_string(),
                    )
                }
            }
            "paystack" => payment_state(
                secrets.has(&secrets.paystack_secret_key),
                "Secret key is set (wrangler secret). Webhooks are verified with HMAC-SHA512.",
                "Not configured: set PAYSTACK_SECRET_KEY with `wrangler secret put PAYSTACK_SECRET_KEY`.",
            ),
            "flutterwave" => payment_state(
                secrets.has(&secrets.flutterwave_secret_key),
                "Secret key is set (wrangler secret). Webhooks are verified against the FLUTTERWAVE_SECRET_HASH you set in the Flutterwave dashboard.",
                "Not configured: set FLUTTERWAVE_SECRET_KEY and FLUTTERWAVE_SECRET_HASH with `wrangler secret put`.",
            ),
            "stripe" => payment_state(
                secrets.has(&secrets.stripe_secret_key),
                "Secret key is set (wrangler secret). Webhooks are verified with HMAC-SHA256 against STRIPE_WEBHOOK_SECRET (300 s tolerance).",
                "Not configured: set STRIPE_SECRET_KEY and STRIPE_WEBHOOK_SECRET with `wrangler secret put`.",
            ),
            "analytics" => {
                let provider = config_str(&config, "provider");
                let id = config_str(&config, "measurementId");
                if (provider == "ga4" || provider == "plausible") && !id.is_empty() {
                    (
                        true,
                        if provider == "ga4" {
                            format!("Google Analytics 4 is configured ({id}).")
                        } else {
                            format!("Plausible is configured ({id}).")
                        },
                    )
                } else {
                    (
                        false,
                        "No analytics provider is configured — public pages carry no tracking snippet.".to_string(),
                    )
                }
            }
            "social" => (
                true,
                "Share links need no setup — they are plain links on posts and products.".to_string(),
            ),
            "cloudflare" => (
                true,
                if bindings.ai {
                    "All bindings present, including Workers AI.".to_string()
                } else {
                    "D1, R2, KV and Static Assets are bound. Workers AI is not bound yet (local dev uses the mock AI provider.)".to_string()
                },
            ),
            "marketplace" => (
                true,
                "Skeleton: local ZIP install only. No remote marketplace directory yet.".to_string(),
            ),
            _ => (false, String::new()),
        };
        out.push(IntegrationStatus {
            id: def.id,
            name: def.name,
            kind: def.kind,
            description: def.description,
            setup_hint: def.setup_hint,
            configured,
            configured_detail: detail,
            config,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{block_on, SqliteDb};
    use serde_json::json;

    #[test]
    fn lists_every_integration_with_live_configured_state() {
        let db = SqliteDb::new();
        let list = block_on(list_integrations(
            &db,
            &EnvSecrets::default(),
            &Bindings::default(),
        ))
        .unwrap();
        let ids: Vec<&str> = list.iter().map(|i| i.id).collect();
        assert_eq!(
            ids,
            vec![
                "email",
                "paystack",
                "flutterwave",
                "stripe",
                "analytics",
                "social",
                "cloudflare",
                "marketplace",
            ]
        );
        let email = list.iter().find(|i| i.id == "email").unwrap();
        assert!(!email.configured);
        assert!(
            email.configured_detail.contains("not delivered"),
            "{}",
            email.configured_detail
        );
        let social = list.iter().find(|i| i.id == "social").unwrap();
        assert!(social.configured);
        let cloudflare = list.iter().find(|i| i.id == "cloudflare").unwrap();
        assert!(cloudflare.configured);
        assert!(
            cloudflare
                .configured_detail
                .contains("Workers AI is not bound"),
            "{}",
            cloudflare.configured_detail
        );
    }

    #[test]
    fn email_and_payments_flip_when_secrets_and_config_are_set() {
        let db = SqliteDb::new();
        block_on(crate::config::set_integration_config(
            &db,
            "email",
            &json!({ "apiUrl": "https://api.example.com/send" })
                .as_object()
                .unwrap()
                .clone(),
        ))
        .unwrap();
        let secrets = EnvSecrets {
            email_api_key: Some("key_123".to_string()),
            paystack_secret_key: Some("sk_test".to_string()),
            ..Default::default()
        };
        let bindings = Bindings {
            ai: true,
            ..Default::default()
        };
        let list = block_on(list_integrations(&db, &secrets, &bindings)).unwrap();
        let email = list.iter().find(|i| i.id == "email").unwrap();
        assert!(email.configured);
        assert!(email
            .configured_detail
            .contains("https://api.example.com/send"));
        let paystack = list.iter().find(|i| i.id == "paystack").unwrap();
        assert!(paystack.configured);
        assert!(paystack.configured_detail.contains("HMAC-SHA512"));
        let stripe = list.iter().find(|i| i.id == "stripe").unwrap();
        assert!(!stripe.configured);
        let cloudflare = list.iter().find(|i| i.id == "cloudflare").unwrap();
        assert!(cloudflare
            .configured_detail
            .contains("All bindings present"));
    }
}
