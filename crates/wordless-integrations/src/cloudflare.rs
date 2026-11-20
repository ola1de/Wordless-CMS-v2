//! Cloudflare bindings status (port of v1 `cloudflare.ts`) —
//! read-only, real.
//!
//! Reports which Worker bindings actually exist in this environment
//! (D1, R2, KV, Static Assets, Workers AI). Nothing is invented: a
//! binding is listed only when the env object has it. This is the
//! honest answer to "is Cloudflare connected?" — the AI binding is
//! absent until the user deploys with their own account.

use serde::Serialize;

use crate::env::Bindings;

/// One binding's presence (v1 `BindingStatus`).
#[derive(Debug, Clone, Serialize)]
pub struct BindingStatus {
    pub binding: &'static str,
    pub present: bool,
    pub detail: &'static str,
}

/// The five binding statuses, in v1 order. Pure function over the
/// [`Bindings`] presence flags the worker entry builds from `Env`.
pub fn cloudflare_binding_status(bindings: &Bindings) -> Vec<BindingStatus> {
    vec![
        BindingStatus {
            binding: "DB (D1)",
            present: bindings.d1,
            detail: "Relational data (posts, orders, options).",
        },
        BindingStatus {
            binding: "MEDIA (R2)",
            present: bindings.r2,
            detail: "Media uploads, theme files, marketplace extension files.",
        },
        BindingStatus {
            binding: "CONFIG (KV)",
            present: bindings.kv,
            detail: "Reserved for fast config reads (not used yet).",
        },
        BindingStatus {
            binding: "ASSETS (Static Assets)",
            present: bindings.assets,
            detail: "Serves the React admin SPA at /admin.",
        },
        BindingStatus {
            binding: "AI (Workers AI)",
            present: bindings.ai,
            detail: if bindings.ai {
                "Workers AI is bound — the default AI provider can run."
            } else {
                "Not bound: uncomment the [ai] block in wrangler.toml when deploying to Cloudflare. Local dev uses the mock provider."
            },
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_the_bindings_that_actually_exist() {
        let all = cloudflare_binding_status(&Bindings {
            d1: true,
            r2: true,
            kv: true,
            assets: true,
            ai: true,
        });
        assert!(all.iter().all(|b| b.present));
        assert!(all
            .iter()
            .any(|b| b.binding == "AI (Workers AI)" && b.detail.contains("can run")));

        let none = cloudflare_binding_status(&Bindings::default());
        assert_eq!(none.len(), 5);
        assert!(none.iter().take(4).all(|b| !b.present));
        let ai = none
            .iter()
            .find(|b| b.binding == "AI (Workers AI)")
            .unwrap();
        assert!(!ai.present);
        assert!(ai.detail.contains("Not bound"), "{}", ai.detail);
    }
}
