//! Shared AI types — port of v1 `src/ai/types.ts` and
//! `src/modules/ai/types.ts` (Milestone 6).
//!
//! Operating modes:
//! - `manual`: agents only draft. Nothing is applied, no approval is
//!   created. The result is text and data the admin can read.
//! - `supervised` (default): agents propose changes; every proposal
//!   becomes an approval record and NOTHING is applied until an admin
//!   approves it in the command centre.
//! - `autonomous`: LOW-risk proposals apply automatically; medium- and
//!   high-risk proposals still become approval records and wait for a
//!   human. Destructive and publishing actions are always high-risk and
//!   never auto-apply.
//!
//! Safety rules (same as v1, non-negotiable): `create_post` ALWAYS
//! creates a draft; `publish_post` ALWAYS needs approval; there is NO
//! delete action; external/AI content is data, never instructions.

use serde::{Deserialize, Serialize};

/// Operating mode of the AI system. Stored in the `wl_ai_mode` option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AiMode {
    Manual,
    Supervised,
    Autonomous,
}

impl AiMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            AiMode::Manual => "manual",
            AiMode::Supervised => "supervised",
            AiMode::Autonomous => "autonomous",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<AiMode> {
        match s {
            "manual" => Some(AiMode::Manual),
            "supervised" => Some(AiMode::Supervised),
            "autonomous" => Some(AiMode::Autonomous),
            _ => None,
        }
    }

    /// Plain-language description shown in the command centre.
    pub fn description(&self) -> &'static str {
        match self {
            AiMode::Manual => "Agents only draft. Nothing is applied and no approval is created — results are text and data for the admin to read.",
            AiMode::Supervised => "Agents propose changes; every proposal waits in the approvals queue until an admin approves or rejects it. Nothing applies on its own.",
            AiMode::Autonomous => "Low-risk proposals (e.g. creating a draft post) apply automatically. Medium- and high-risk proposals — anything destructive or publishing — still wait for approval.",
        }
    }
}

/// Safe default: nothing applies without a human saying so.
pub const DEFAULT_AI_MODE: AiMode = AiMode::Supervised;

/// All operating modes, in v1 order.
pub const AI_MODES: [AiMode; 3] = [AiMode::Manual, AiMode::Supervised, AiMode::Autonomous];

pub fn is_ai_mode(value: &str) -> bool {
    AiMode::from_str(value).is_some()
}

/// Risk of one proposed change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalRisk {
    Low,
    Medium,
    High,
}

impl ProposalRisk {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProposalRisk::Low => "low",
            ProposalRisk::Medium => "medium",
            ProposalRisk::High => "high",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<ProposalRisk> {
        match s {
            "low" => Some(ProposalRisk::Low),
            "medium" => Some(ProposalRisk::Medium),
            "high" => Some(ProposalRisk::High),
            _ => None,
        }
    }
}

/// The action types an agent may propose. There is deliberately NO
/// delete action in this list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalActionType {
    CreatePost,
    UpdatePost,
    PublishPost,
    UpdateOption,
    CreateTerm,
    UpdateThemeVariation,
}

impl ProposalActionType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProposalActionType::CreatePost => "create_post",
            ProposalActionType::UpdatePost => "update_post",
            ProposalActionType::PublishPost => "publish_post",
            ProposalActionType::UpdateOption => "update_option",
            ProposalActionType::CreateTerm => "create_term",
            ProposalActionType::UpdateThemeVariation => "update_theme_variation",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<ProposalActionType> {
        match s {
            "create_post" => Some(ProposalActionType::CreatePost),
            "update_post" => Some(ProposalActionType::UpdatePost),
            "publish_post" => Some(ProposalActionType::PublishPost),
            "update_option" => Some(ProposalActionType::UpdateOption),
            "create_term" => Some(ProposalActionType::CreateTerm),
            "update_theme_variation" => Some(ProposalActionType::UpdateThemeVariation),
            _ => None,
        }
    }
}

/// A change an agent proposes. Applying goes through the applier
/// (`crate::apply`), which calls the existing services — never raw SQL
/// writes to content tables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposedAction {
    #[serde(rename = "type")]
    pub action_type: ProposalActionType,
    /// Human-readable one-liner shown in the approvals queue.
    pub summary: String,
    pub risk: ProposalRisk,
    /// Free-form payload; the applier validates every field it reads.
    #[serde(default)]
    pub payload: serde_json::Value,
}

impl ProposedAction {
    pub fn new(
        action_type: ProposalActionType,
        summary: impl Into<String>,
        risk: ProposalRisk,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            action_type,
            summary: summary.into(),
            risk,
            payload,
        }
    }
}

/// What an agent returns. `output` is labelled as AI output in the UI;
/// `data` holds only measured data, never invented numbers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResult {
    /// One-paragraph summary for the task list.
    pub summary: String,
    /// Main text output (draft, report, reply).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Structured findings (counts, lists, checks).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// Changes the agent proposes, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposals: Option<Vec<ProposedAction>>,
    /// Token usage from the provider, when a provider was used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<crate::providers::ProviderUsage>,
}

/// Status of an AI task (`wl_jobs` row with `kind = 'ai_task'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AiTaskStatus {
    Pending,
    Running,
    Done,
    Failed,
}

impl AiTaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            AiTaskStatus::Pending => "pending",
            AiTaskStatus::Running => "running",
            AiTaskStatus::Done => "done",
            AiTaskStatus::Failed => "failed",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<AiTaskStatus> {
        match s {
            "pending" => Some(AiTaskStatus::Pending),
            "running" => Some(AiTaskStatus::Running),
            "done" => Some(AiTaskStatus::Done),
            "failed" => Some(AiTaskStatus::Failed),
            _ => None,
        }
    }
}

/// The stored result of a finished AI task (the `result` JSON on the
/// `wl_jobs` row), mirroring v1 `AiTaskResult`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiTaskResult {
    pub agent: String,
    pub mode: AiMode,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<crate::providers::ProviderUsage>,
    #[serde(default)]
    pub proposals: Vec<ProposedAction>,
    #[serde(default)]
    pub approvals: Vec<ApprovalSummary>,
    #[serde(default)]
    pub applied: Vec<AppliedSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apply_errors: Option<Vec<ApplyErrorSummary>>,
}

/// One approval created by a task, as recorded in its result JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalSummary {
    pub id: String,
    pub summary: String,
    pub risk: ProposalRisk,
}

/// One auto-applied proposal, as recorded in a task's result JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedSummary {
    pub summary: String,
    pub result: serde_json::Value,
}

/// One failed auto-apply, as recorded in a task's result JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyErrorSummary {
    pub summary: String,
    pub error: String,
}

/// Public view of an AI task, mirroring v1 `AiTaskView`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiTaskView {
    pub id: String,
    pub agent: String,
    pub input: String,
    pub mode: AiMode,
    pub status: AiTaskStatus,
    /// 100 when done, 0 when pending/failed, 50 while running.
    pub progress: u8,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<AiTaskResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Paginated task list, mirroring v1 `AiTaskList`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiTaskList {
    pub tasks: Vec<AiTaskView>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

/// Input for creating an AI task. `mode` overrides the site mode for
/// this one task; it defaults to the site mode.
#[derive(Debug, Clone)]
pub struct CreateAiTaskInput {
    pub agent: String,
    pub input: String,
    pub mode: Option<AiMode>,
}

/// Status of an approval (`wl_ai_approvals` row).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Rejected,
}

impl ApprovalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ApprovalStatus::Pending => "pending",
            ApprovalStatus::Approved => "approved",
            ApprovalStatus::Rejected => "rejected",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<ApprovalStatus> {
        match s {
            "pending" => Some(ApprovalStatus::Pending),
            "approved" => Some(ApprovalStatus::Approved),
            "rejected" => Some(ApprovalStatus::Rejected),
            _ => None,
        }
    }
}

/// Public view of an approval, mirroring v1 `ApprovalView`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalView {
    pub id: String,
    pub task_id: String,
    pub agent: String,
    pub action: ProposedAction,
    pub status: ApprovalStatus,
    pub created_by: i64,
    pub decided_by: Option<i64>,
    pub decided_at: Option<String>,
    pub reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Paginated approval list, mirroring v1 `ApprovalList`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalList {
    pub approvals: Vec<ApprovalView>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}
