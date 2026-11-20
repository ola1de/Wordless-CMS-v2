//! WORDLESS v2 AI operating system (P3).
//!
//! Port of the v1.1.0 TypeScript AI system (`src/ai/` +
//! `src/modules/ai/`, Milestone 6): provider abstraction, orchestrator
//! with three operating modes, 15 specialist agents, approvals in
//! `wl_ai_approvals` (migration 0006), website memory in `wl_ai_memory`
//! (migration 0006). Same SQL, same bound parameters, same response
//! shapes as v1; the route handlers that call these functions live in
//! `wordless-worker` and are wired by the P3 coordinator (this crate
//! owns no routes).
//!
//! # Providers
//!
//! Workers AI is the default and only required provider (user
//! decision, 2026-11-19). Which path this crate uses — **measured
//! 2026-11-19**: the native `worker::Ai` binding. The pinned `worker`
//! 0.9.0 crate exposes `worker::Ai` with `Ai::run(model, input)` and
//! `Env::ai("AI")`, so no REST fallback is implemented for Workers AI
//! (see `platform`). The OpenAI-compatible provider is an optional
//! fallback via `worker::Fetch`; the mock provider is deterministic
//! (same prompt → same response) for tests and local dev. An absent AI
//! binding is reported honestly as not-configured, never faked. Live
//! Workers AI calls are never observed in this VM.
//!
//! # Modes and safety (same as v1, non-negotiable)
//!
//! - `manual`: agents only draft; no approvals, no application.
//! - `supervised` (default): every proposal becomes a pending
//!   approval; nothing applies until an admin approves it.
//! - `autonomous`: low-risk proposals auto-apply; medium/high-risk
//!   proposals still wait for approval.
//! - `create_post` ALWAYS creates a draft; `publish_post` ALWAYS needs
//!   approval; there is NO delete action; external/AI content is data,
//!   never instructions.
//!
//! # Public service functions (all take `db: &dyn Db`, the
//! `wordless-db` trait, and return `Result<_, WordlessError>` unless
//! noted)
//!
//! Providers (`providers`):
//! - `providers::list_providers(env: &AiEnv) -> Vec<ProviderInfo>` —
//!   all three providers with their live configured state.
//! - `providers::get_provider(id: ProviderId) -> Rc<dyn Provider>` —
//!   look one provider up by id.
//! - `providers::get_selected_provider_id(db) -> Result<ProviderId>` —
//!   the selected provider (`wl_ai_provider` option, default
//!   `workers-ai`).
//! - `providers::select_provider(db, id: &str) -> Result<ProviderId>`
//!   — persist a provider selection; unknown ids are a 400.
//! - `providers::resolve_provider(db, env) -> Result<ResolvedProvider>`
//!   — the selected provider plus whether it can complete a request.
//! - `providers::test_provider(env, id: &str) -> Result<ProviderProbe>`
//!   — probe one provider by id; never throws for "not configured".
//!
//! Operating mode (`service`):
//! - `service::get_ai_mode(db) -> Result<AiMode>` — the site mode
//!   (`wl_ai_mode` option, default `supervised`).
//! - `service::set_ai_mode(db, mode: &str) -> Result<AiMode>` — set
//!   the mode; unknown modes are a 400.
//!
//! Agents (`agents`):
//! - `agents::list_agents(provider_configured: bool) -> Vec<AgentInfo>`
//!   — the 15 agents with availability for the command centre.
//! - `agents::get_agent(id: &str) -> Option<&'static dyn Agent>` —
//!   look one agent up by id.
//!
//! Tasks (`orchestrator`):
//! - `orchestrator::create_task(db, CreateAiTaskInput) -> Result<AiTaskView>`
//!   — create a task (`wl_jobs` kind `'ai_task'`, status `pending`).
//! - `orchestrator::get_task(db, id: &str) -> Result<AiTaskView>` —
//!   fetch one task; unknown ids are a 404.
//! - `orchestrator::list_tasks(db, page: i64, per_page: i64) -> Result<AiTaskList>`
//!   — list tasks, newest first.
//! - `orchestrator::run_task(db, job_id: &str, env: &AiEnv, user_id: i64) -> Result<AiTaskView>`
//!   — run one task to completion (done/failed) under the site mode.
//! - `orchestrator::process_proposals(db, mode, proposals, task_id, agent_id, user_id) -> Result<ProcessProposalsResult>`
//!   — mode enforcement, exported for tests.
//!
//! Approvals (`service`):
//! - `service::create_approval(db, task_id, agent, action, created_by) -> Result<ApprovalView>`
//!   — create a pending approval.
//! - `service::get_approval(db, id: &str) -> Result<ApprovalView>` —
//!   fetch one approval; unknown ids are a 404.
//! - `service::list_approvals(db, status: Option<&str>, page: i64, per_page: i64) -> Result<ApprovalList>`
//!   — list approvals, newest first.
//! - `service::approve_approval(db, id: &str, user_id: i64) -> Result<ApprovalView>`
//!   — apply the action, then mark approved (apply failure leaves
//!   the approval pending).
//! - `service::reject_approval(db, id: &str, user_id: i64, reason: Option<&str>) -> Result<ApprovalView>`
//!   — reject, recording the reason; nothing is applied.
//! - `service::decide_approval(db, id, approve, user_id, reason) -> Result<ApprovalView>`
//!   — the shared decide implementation behind both wrappers.
//!
//! Memory (`memory`):
//! - `memory::list_memory(db) -> Result<MemoryList>` — facts +
//!   notes.
//! - `memory::list_memory_notes(db) -> Result<Vec<MemoryEntry>>` —
//!   admin notes only.
//! - `memory::add_memory_note(db, text: &str) -> Result<MemoryEntry>`
//!   — add an admin note; refuses anything that looks like a secret.
//! - `memory::delete_memory_note(db, id: i64) -> Result<bool>` —
//!   delete an admin note; facts cannot be deleted (404 when missing).
//! - `memory::rebuild_memory(db) -> Result<MemoryRebuildResult>` —
//!   re-ingest facts from the site's own data (notes untouched).
//! - `memory::build_memory_context(db) -> Result<String>` — render
//!   memory as the context block injected into agent prompts.
//!
//! Applying (`apply`):
//! - `apply::apply_proposal(db, action: &ProposedAction, user_id: i64) -> Result<serde_json::Value>`
//!   — apply one proposed action through the existing services.
//!
//! # WASM size
//!
//! This crate adds no new external crates (only workspace deps
//! already in `Cargo.lock`) and no regex. Its new code is the
//! provider/agent/orchestrator logic plus one thin wasm32-only
//! platform module. Of its sibling-crate dependencies,
//! `wordless-cms`, `wordless-media`, `wordless-seo`,
//! `wordless-auth` and `wordless-themes` are already linked into the
//! worker binary; `wordless-commerce`, `wordless-backup`,
//! `wordless-conversion`, `wordless-security` and
//! `wordless-integrations` are P3 crates the worker does not depend
//! on yet, so they will be linked once when the P3 routes are wired
//! in — that cost is shared across all of P3, not specific to this
//! crate. The exact WASM delta cannot be measured until that wiring
//! lands (the worker does not depend on this crate yet); nothing
//! here claims a measured number.

pub mod agents;
pub mod apply;
pub mod env;
pub mod memory;
pub mod orchestrator;
pub mod platform;
pub mod providers;
pub mod service;
pub mod types;
pub mod util;

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests;

pub use agents::{get_agent, list_agents, Agent, AgentContext, AgentInfo, AGENTS};
pub use apply::apply_proposal;
pub use env::AiEnv;
pub use memory::{
    add_memory_note, build_memory_context, delete_memory_note, list_memory, list_memory_notes,
    rebuild_memory, MemoryEntry, MemoryList, MemoryRebuildResult,
};
pub use orchestrator::{
    create_task, get_task, list_tasks, process_proposals, run_task, ProcessProposalsResult,
};
pub use providers::{
    get_provider, get_selected_provider_id, list_providers, resolve_provider, select_provider,
    test_provider, CompleteOptions, CompleteResult, Provider, ProviderId, ProviderInfo,
    ProviderProbe, ProviderUsage, ResolvedProvider, AI_PROVIDER_OPTION, DEFAULT_PROVIDER_ID,
    PROVIDER_IDS,
};
pub use service::{
    approve_approval, create_approval, decide_approval, get_ai_mode, get_approval, list_approvals,
    reject_approval, set_ai_mode, AI_MODE_OPTION,
};
pub use types::{
    AgentResult, AiMode, AiTaskList, AiTaskResult, AiTaskStatus, AiTaskView, AppliedSummary,
    ApplyErrorSummary, ApprovalList, ApprovalStatus, ApprovalSummary, ApprovalView,
    CreateAiTaskInput, ProposalActionType, ProposalRisk, ProposedAction, AI_MODES, DEFAULT_AI_MODE,
};
