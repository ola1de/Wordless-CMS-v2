//! Provider registry — port of v1 `src/ai/providers/index.ts`
//! (Milestone 6).
//!
//! The site's selected provider is stored in the `wl_ai_provider`
//! option. The default is Workers AI (user decision, 2026-11-19) —
//! the only required provider; OpenAI-compatible is an optional
//! fallback, mock is for local development and tests.

pub mod backend;
pub mod mock;
pub mod openai;
pub mod types;
pub mod workers_ai;

pub use types::{
    ai_not_configured, ai_provider_error, CompleteOptions, CompleteResult, Provider, ProviderId,
    ProviderInfo, ProviderProbe, ProviderUsage, DEFAULT_PROVIDER_ID, PROVIDER_IDS,
};

use std::rc::Rc;

use wordless_db::Db;
use wordless_kernel::error::{bad_request, WordlessError};

use crate::env::AiEnv;
use crate::providers::mock::MockProvider;
use crate::providers::openai::OpenAiProvider;
use crate::providers::workers_ai::WorkersAiProvider;

/// Option key holding the selected provider id.
pub const AI_PROVIDER_OPTION: &str = "wl_ai_provider";

/// All providers, in v1 registry order: Workers AI, OpenAI-compatible,
/// mock. Each is a unit struct; the environment is passed per call.
pub fn providers() -> [Rc<dyn Provider>; 3] {
    [
        Rc::new(WorkersAiProvider),
        Rc::new(OpenAiProvider),
        Rc::new(MockProvider),
    ]
}

/// Look up a provider by id. Unknown ids are a 400, mirroring v1's
/// `getProvider`.
pub fn get_provider(id: ProviderId) -> Rc<dyn Provider> {
    match id {
        ProviderId::WorkersAi => Rc::new(WorkersAiProvider),
        ProviderId::Openai => Rc::new(OpenAiProvider),
        ProviderId::Mock => Rc::new(MockProvider),
    }
}

/// All providers with their live configured state for this environment.
pub fn list_providers(env: &AiEnv) -> Vec<ProviderInfo> {
    providers()
        .iter()
        .map(|p| ProviderInfo {
            id: p.id(),
            name: p.name().to_string(),
            description: p.description().to_string(),
            configured: p.is_configured(env),
            configured_detail: p.configured_detail(env),
            recommended: p.recommended(),
            requires_key: p.requires_key(),
        })
        .collect()
}

/// Selected provider id; defaults to Workers AI when never set or when
/// the stored value is not a known provider id.
pub async fn get_selected_provider_id(db: &dyn Db) -> Result<ProviderId, WordlessError> {
    let stored: String = wordless_cms::options::get_option(
        db,
        AI_PROVIDER_OPTION,
        DEFAULT_PROVIDER_ID.as_str().to_string(),
    )
    .await?;
    Ok(ProviderId::from_str(&stored).unwrap_or(DEFAULT_PROVIDER_ID))
}

/// Persist a provider selection. Unknown ids are a 400.
pub async fn select_provider(db: &dyn Db, id: &str) -> Result<ProviderId, WordlessError> {
    let Some(parsed) = ProviderId::from_str(id) else {
        return Err(bad_request(
            "Field \"provider\" must be one of: workers-ai, openai, mock.",
        ));
    };
    wordless_cms::options::set_option(db, AI_PROVIDER_OPTION, &parsed.as_str().to_string()).await?;
    Ok(parsed)
}

/// The selected provider plus whether it can actually complete a
/// request right now.
pub struct ResolvedProvider {
    pub provider: Rc<dyn Provider>,
    pub configured: bool,
}

pub async fn resolve_provider(db: &dyn Db, env: &AiEnv) -> Result<ResolvedProvider, WordlessError> {
    let id = get_selected_provider_id(db).await?;
    let provider = get_provider(id);
    let configured = provider.is_configured(env);
    Ok(ResolvedProvider {
        provider,
        configured,
    })
}

/// Probe one provider by id: mock always passes; real providers report
/// their real state. Unknown ids are a 400.
pub async fn test_provider(env: &AiEnv, id: &str) -> Result<ProviderProbe, WordlessError> {
    let Some(parsed) = ProviderId::from_str(id) else {
        return Err(bad_request(&format!("Unknown AI provider \"{id}\".")));
    };
    Ok(get_provider(parsed).test(env).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SqliteDb;
    use futures_executor::block_on;

    #[test]
    fn registry_lists_all_three_workers_ai_recommended() {
        let env = AiEnv::empty();
        let list = list_providers(&env);
        assert_eq!(
            list.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec!["workers-ai", "openai", "mock"]
        );
        let workers = list.iter().find(|p| p.id == ProviderId::WorkersAi).unwrap();
        assert!(workers.recommended);
        assert!(!workers.configured);
        assert!(
            list.iter()
                .find(|p| p.id == ProviderId::Mock)
                .unwrap()
                .configured
        );
    }

    #[test]
    fn selected_provider_defaults_to_workers_ai_and_persists() {
        block_on(async {
            let db = SqliteDb::new();
            assert_eq!(
                get_selected_provider_id(&db).await.unwrap(),
                ProviderId::WorkersAi
            );
            assert_eq!(
                select_provider(&db, "mock").await.unwrap(),
                ProviderId::Mock
            );
            assert_eq!(
                get_selected_provider_id(&db).await.unwrap(),
                ProviderId::Mock
            );
            assert!(select_provider(&db, "anthropic").await.is_err());
        });
    }
}
