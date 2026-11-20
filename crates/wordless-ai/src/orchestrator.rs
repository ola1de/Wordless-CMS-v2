//! AI orchestrator — port of v1 `src/ai/orchestrator.ts`
//! (Milestone 6).
//!
//! An AI task is a row in `wl_jobs` (kind = `'ai_task'`). The flow:
//! create → run (synchronously in the request in local dev; structured
//! so the same `run_task` can be invoked by a Queue/Workflow consumer
//! in production) → the agent runs → its proposals are handled
//! according to the operating mode → the result JSON is stored on the
//! job.
//!
//! Mode enforcement lives HERE, in [`process_proposals`], and nowhere
//! else:
//! - manual: proposals are recorded in the result only. No approval
//!   rows, no application.
//! - supervised: every proposal becomes a pending approval row.
//!   Nothing is applied.
//! - autonomous: LOW-risk proposals apply immediately through the
//!   applier; medium/high-risk proposals become approval rows. A
//!   failed auto-apply is recorded, never hidden.
//!
//! Prompt-injection rule (architecture §8): external content (uploads,
//! pages, plugins) is DATA, never instructions. Agents only ever
//! receive the admin's input plus website memory; tool calls never
//! accept instruction text from untrusted sources.

use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, not_found, WordlessError};

use crate::agents::{get_agent, AgentContext};
use crate::apply::apply_proposal;
use crate::env::AiEnv;
use crate::memory::build_memory_context;
use crate::providers::{ai_not_configured, resolve_provider};
use crate::service::{create_approval, get_ai_mode};
use crate::types::{
    AiMode, AiTaskList, AiTaskResult, AiTaskStatus, AiTaskView, AppliedSummary, ApplyErrorSummary,
    ApprovalSummary, ApprovalView, CreateAiTaskInput, ProposedAction,
};
use crate::util::{new_uuid_v4, require_str};

/// What [`process_proposals`] did with an agent's proposals.
#[derive(Debug, Clone)]
pub struct ProcessProposalsResult {
    pub approvals: Vec<ApprovalView>,
    pub applied: Vec<AppliedSummary>,
    pub apply_errors: Vec<ApplyErrorSummary>,
}

struct JobRow {
    id: String,
    status: String,
    payload: String,
    result: Option<String>,
    created_at: String,
    updated_at: String,
}

fn row_to_job(row: &Row) -> JobRow {
    JobRow {
        id: row_text(row, "id"),
        status: row_text(row, "status"),
        payload: row_text(row, "payload"),
        result: row
            .get("result")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

fn parse_payload(payload: &str) -> (String, String, AiMode) {
    let parsed: serde_json::Value = serde_json::from_str(payload).unwrap_or(serde_json::json!({}));
    let agent = parsed
        .get("agent")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let input = parsed
        .get("input")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mode = parsed
        .get("mode")
        .and_then(|v| v.as_str())
        .and_then(AiMode::from_str)
        .unwrap_or(AiMode::Supervised);
    (agent, input, mode)
}

fn row_to_view(row: &JobRow) -> AiTaskView {
    let (agent, input, mode) = parse_payload(&row.payload);
    let status = AiTaskStatus::from_str(&row.status).unwrap_or(AiTaskStatus::Pending);
    let progress = match status {
        AiTaskStatus::Done => 100,
        AiTaskStatus::Pending | AiTaskStatus::Failed => 0,
        AiTaskStatus::Running => 50,
    };
    let mut view = AiTaskView {
        id: row.id.clone(),
        agent,
        input,
        mode,
        status,
        progress,
        created_at: row.created_at.clone(),
        updated_at: row.updated_at.clone(),
        result: None,
        error: None,
    };
    if let Some(result) = &row.result {
        if !result.is_empty() {
            if status == AiTaskStatus::Done {
                if let Ok(parsed) = serde_json::from_str::<AiTaskResult>(result) {
                    view.result = Some(parsed);
                }
            } else if status == AiTaskStatus::Failed {
                let parsed: serde_json::Value =
                    serde_json::from_str(result).unwrap_or(serde_json::json!({}));
                view.error = parsed
                    .get("error")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .or_else(|| Some(result.clone()));
            }
        }
    }
    view
}

/// Create an AI task (`wl_jobs` row, kind `'ai_task'`, status
/// `pending`). Unknown agents and unavailable agents are 400s; the
/// mode defaults to the site mode when not given.
pub async fn create_task(
    db: &dyn Db,
    input: CreateAiTaskInput,
) -> Result<AiTaskView, WordlessError> {
    let agent_id = require_str(&input.agent, "agent", 100)?;
    let text = require_str(&input.input, "input", 10000)?;
    let Some(agent) = get_agent(&agent_id) else {
        return Err(bad_request(&format!("Unknown AI agent \"{agent_id}\".")));
    };
    if let Some(reason) = agent.unavailable_reason() {
        return Err(bad_request(&format!(
            "Agent \"{}\" is unavailable: {reason}",
            agent.name()
        )));
    }
    let mode = match input.mode {
        Some(m) => m,
        None => get_ai_mode(db).await?,
    };
    let id = new_uuid_v4()?;
    let now = wordless_kernel::time::now_iso();
    db.prepare(
        "INSERT INTO wl_jobs (id, kind, status, payload, attempts, created_at, updated_at)
         VALUES (?, 'ai_task', 'pending', ?, 0, ?, ?)",
    )
    .bind(vec![
        Param::Text(id.clone()),
        Param::Text(
            serde_json::to_string(&serde_json::json!({
                "agent": agent_id,
                "input": text,
                "mode": mode.as_str(),
            }))
            .unwrap_or_else(|_| "{}".to_string()),
        ),
        Param::Text(now.clone()),
        Param::Text(now),
    ])
    .run()
    .await?;
    get_task(db, &id).await
}

/// Fetch one AI task by id. Unknown ids are a 404.
pub async fn get_task(db: &dyn Db, id: &str) -> Result<AiTaskView, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_jobs WHERE id = ? AND kind = 'ai_task' LIMIT 1")
        .bind(vec![Param::Text(id.to_string())])
        .first::<Row>()
        .await?;
    row.map(|r| row_to_view(&row_to_job(&r)))
        .ok_or_else(|| not_found("AI task not found."))
}

/// List AI tasks, newest first.
pub async fn list_tasks(
    db: &dyn Db,
    page: i64,
    per_page: i64,
) -> Result<AiTaskList, WordlessError> {
    let page = page.max(1);
    let per_page = per_page.clamp(1, 100);
    let offset = (page - 1) * per_page;
    let total: i64 = db
        .prepare("SELECT COUNT(*) AS n FROM wl_jobs WHERE kind = 'ai_task'")
        .first::<Row>()
        .await?
        .map(|r| row_int(&r, "n"))
        .unwrap_or(0);
    let rows: Vec<Row> = db
        .prepare(
            "SELECT * FROM wl_jobs WHERE kind = 'ai_task' ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?",
        )
        .bind(vec![Param::Integer(per_page), Param::Integer(offset)])
        .all::<Row>()
        .await?;
    let total_pages = (total + per_page - 1) / per_page;
    Ok(AiTaskList {
        tasks: rows.iter().map(|r| row_to_view(&row_to_job(r))).collect(),
        total,
        page,
        per_page,
        total_pages: total_pages.max(1),
    })
}

/// Handle an agent's proposals under the operating mode. Exported for
/// tests: mode enforcement must be provable without running a whole task.
pub async fn process_proposals(
    db: &dyn Db,
    site_id: i64,
    mode: AiMode,
    proposals: &[ProposedAction],
    task_id: &str,
    agent_id: &str,
    user_id: i64,
) -> Result<ProcessProposalsResult, WordlessError> {
    let mut approvals = Vec::new();
    let mut applied = Vec::new();
    let mut apply_errors = Vec::new();
    if mode == AiMode::Manual {
        return Ok(ProcessProposalsResult {
            approvals,
            applied,
            apply_errors,
        });
    }
    for proposal in proposals {
        if mode == AiMode::Autonomous && proposal.risk == crate::types::ProposalRisk::Low {
            match apply_proposal(db, site_id, proposal, user_id).await {
                Ok(result) => applied.push(AppliedSummary {
                    summary: proposal.summary.clone(),
                    result,
                }),
                Err(e) => apply_errors.push(ApplyErrorSummary {
                    summary: proposal.summary.clone(),
                    error: e.message.clone(),
                }),
            }
        } else {
            let approval = create_approval(db, task_id, agent_id, proposal, user_id).await?;
            approvals.push(approval);
        }
    }
    Ok(ProcessProposalsResult {
        approvals,
        applied,
        apply_errors,
    })
}

/// Run one AI task to completion. Safe to call from a request handler
/// (local dev) or from a Queue/Workflow consumer (production): it only
/// touches the job row, the provider, the agent and the approvals
/// table. A failed run marks the job `failed` and records the error;
/// it does not throw, except when the task row itself is missing or
/// the agent is unknown/unavailable (mirroring v1, where those checks
/// throw before the job is marked running).
pub async fn run_task(
    db: &dyn Db,
    site_id: i64,
    job_id: &str,
    env: &AiEnv,
    user_id: i64,
) -> Result<AiTaskView, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_jobs WHERE id = ? AND kind = 'ai_task' LIMIT 1")
        .bind(vec![Param::Text(job_id.to_string())])
        .first::<Row>()
        .await?;
    let Some(row) = row else {
        return Err(not_found("AI task not found."));
    };
    let job = row_to_job(&row);
    let (agent_id, input, mode) = parse_payload(&job.payload);
    let Some(agent) = get_agent(&agent_id) else {
        return Err(bad_request(&format!("Unknown AI agent \"{agent_id}\".")));
    };
    if let Some(reason) = agent.unavailable_reason() {
        return Err(bad_request(&format!(
            "Agent \"{}\" is unavailable: {reason}",
            agent.name()
        )));
    }
    let now = wordless_kernel::time::now_iso();
    db.prepare(
        "UPDATE wl_jobs SET status = 'running', attempts = attempts + 1, result = ?, updated_at = ? WHERE id = ?",
    )
    .bind(vec![
        Param::Text(serde_json::to_string(&serde_json::json!({"progress": 50, "phase": "running"})).unwrap()),
        Param::Text(now),
        Param::Text(job_id.to_string()),
    ])
    .run()
    .await?;

    let outcome: Result<AiTaskResult, WordlessError> = async {
        let resolved = resolve_provider(db, env).await?;
        if agent.requires_provider() && !resolved.configured {
            return Err(ai_not_configured(&format!(
                "No AI provider is configured for agent \"{}\". Workers AI (the default) needs the [ai] binding in wrangler.toml; or select the mock provider for local development.",
                agent.name()
            )));
        }
        let memory_text = build_memory_context(db).await?;
        let agent_ctx = AgentContext {
            db,
            env,
            provider: resolved.provider,
            provider_configured: resolved.configured,
            input,
            memory_text,
            user_id,
            site_id,
            object_count: None,
        };
        let result = agent.run(&agent_ctx).await?;
        let proposals = result.proposals.clone().unwrap_or_default();
        let handled = process_proposals(
            db,
            site_id,
            mode,
            &proposals,
            job_id,
            agent.id(),
            user_id,
        )
        .await?;
        Ok(AiTaskResult {
            agent: agent.id().to_string(),
            mode,
            summary: result.summary,
            output: result.output,
            data: result.data,
            usage: result.usage,
            proposals,
            approvals: handled
                .approvals
                .iter()
                .map(|a| ApprovalSummary {
                    id: a.id.clone(),
                    summary: a.action.summary.clone(),
                    risk: a.action.risk,
                })
                .collect(),
            applied: handled.applied,
            apply_errors: if handled.apply_errors.is_empty() {
                None
            } else {
                Some(handled.apply_errors)
            },
        })
    }
    .await;

    match outcome {
        Ok(task_result) => {
            let result_json =
                serde_json::to_string(&task_result).unwrap_or_else(|_| "{}".to_string());
            db.prepare(
                "UPDATE wl_jobs SET status = 'done', result = ?, updated_at = ? WHERE id = ?",
            )
            .bind(vec![
                Param::Text(result_json),
                Param::Text(wordless_kernel::time::now_iso()),
                Param::Text(job_id.to_string()),
            ])
            .run()
            .await?;
        }
        Err(e) => {
            let message = e.message.clone();
            let result_json =
                serde_json::to_string(&serde_json::json!({"progress": 0, "error": message}))
                    .unwrap_or_else(|_| "{}".to_string());
            db.prepare(
                "UPDATE wl_jobs SET status = 'failed', result = ?, updated_at = ? WHERE id = ?",
            )
            .bind(vec![
                Param::Text(result_json),
                Param::Text(wordless_kernel::time::now_iso()),
                Param::Text(job_id.to_string()),
            ])
            .run()
            .await?;
        }
    }
    get_task(db, job_id).await
}
