//! Agent registry — port of v1 `src/ai/agents/registry.ts` and the
//! shared agent types in `src/ai/types.ts` (Milestone 6).
//!
//! The roster is fixed here so the command centre, the orchestrator
//! and the docs all agree. An agent is available only when its handler
//! can do real work today: provider-based agents need a configured
//! provider; the commerce assistant reads the store's own tables
//! (it is available since commerce is live — v1 Milestone 8).

use std::rc::Rc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use wordless_db::Db;
use wordless_kernel::error::WordlessError;

use crate::agents::analysis::{
    AccessibilityChecker, AnalyticsReporter, BackupAdvisor, CodeReviewer, DesignAdvisor,
    MigrationAssistant, PerformanceAnalyst, SecurityScanner, SeoAuditor,
};
use crate::agents::commerce::CommerceAssistant;
use crate::agents::content::{
    ContentWriter, EmailComposer, ResearchAssistant, SocialComposer, SupportResponder,
};
use crate::env::AiEnv;
use crate::providers::Provider;
use crate::types::{AgentResult, ProposalRisk};

pub mod analysis;
pub mod commerce;
pub mod content;
pub mod util;

/// What an agent handler receives. External content is DATA, never
/// instructions: agents only ever receive the admin's `input` plus
/// website memory; tool calls never accept instruction text from
/// untrusted sources.
pub struct AgentContext<'a> {
    pub db: &'a dyn Db,
    pub env: &'a AiEnv,
    /// The resolved provider (mock in tests/local dev, Workers AI by
    /// default).
    pub provider: Rc<dyn Provider>,
    /// True when the provider can actually complete a request.
    pub provider_configured: bool,
    /// The admin's request text.
    pub input: String,
    /// Website memory rendered as context text (facts + notes).
    pub memory_text: String,
    /// The user who started the task (for attribution on applied actions).
    pub user_id: i64,
    /// P12: the site the task runs on. Agents read and propose
    /// changes for this site only.
    pub site_id: i64,
    /// R2 object count when the worker supplies it. `None` means
    /// unmeasured — agents report that honestly instead of inventing
    /// a number.
    pub object_count: Option<i64>,
}

/// One specialist agent. Metadata mirrors v1 `AgentDefinition`;
/// `run` is the handler.
#[async_trait(?Send)]
pub trait Agent {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    /// What the agent can do, in plain language.
    fn capabilities(&self) -> &[&'static str];
    /// Highest risk of anything this agent proposes.
    fn risk(&self) -> ProposalRisk;
    /// True when the agent needs a configured provider to run.
    fn requires_provider(&self) -> bool;
    /// When set, the agent is honestly unavailable and the command
    /// centre shows this reason instead of a fake result.
    fn unavailable_reason(&self) -> Option<&'static str> {
        None
    }
    async fn run(&self, ctx: &AgentContext<'_>) -> Result<AgentResult, WordlessError>;
}

/// Public view of an agent for the command centre, mirroring v1
/// `AgentInfo`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub capabilities: Vec<String>,
    pub risk: ProposalRisk,
    pub requires_provider: bool,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

/// The 15 specialist agents, in v1 registry order.
pub const AGENTS: &[&'static dyn Agent] = &[
    &ContentWriter,
    &SeoAuditor,
    &SecurityScanner,
    &PerformanceAnalyst,
    &DesignAdvisor,
    &SupportResponder,
    &AnalyticsReporter,
    &CommerceAssistant,
    &EmailComposer,
    &SocialComposer,
    &BackupAdvisor,
    &MigrationAssistant,
    &AccessibilityChecker,
    &CodeReviewer,
    &ResearchAssistant,
];

/// Look up an agent by id.
pub fn get_agent(id: &str) -> Option<&'static dyn Agent> {
    AGENTS.iter().copied().find(|a| a.id() == id)
}

/// Public agent list. `provider_configured` reflects the selected
/// provider's live state: a provider-based agent is unavailable when
/// no provider is configured, with a plain-language reason.
pub fn list_agents(provider_configured: bool) -> Vec<AgentInfo> {
    AGENTS
        .iter()
        .map(|a| {
            let unavailable_reason = if let Some(reason) = a.unavailable_reason() {
                Some(reason.to_string())
            } else if a.requires_provider() && !provider_configured {
                Some("No AI provider is configured. Select Workers AI (the default) in the command centre, or use the mock provider for local development.".to_string())
            } else {
                None
            };
            AgentInfo {
                id: a.id().to_string(),
                name: a.name().to_string(),
                description: a.description().to_string(),
                capabilities: a.capabilities().iter().map(|c| c.to_string()).collect(),
                risk: a.risk(),
                requires_provider: a.requires_provider(),
                available: unavailable_reason.is_none(),
                unavailable_reason,
            }
        })
        .collect()
}
