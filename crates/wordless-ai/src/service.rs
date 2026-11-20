//! AI module service — port of v1 `src/modules/ai/service.ts`
//! (Milestone 6): operating mode + approvals.
//!
//! Mode is a site setting (`wl_ai_mode`, default `supervised`).
//! Approvals are the human gate: an agent's proposal becomes a row in
//! `wl_ai_approvals` and nothing it proposes is applied until an admin
//! approves it here. Approving applies the action through the applier
//! (existing services only); rejecting records the reason.

use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, not_found, WordlessError};

use crate::apply::apply_proposal;
use crate::types::{
    AiMode, ApprovalList, ApprovalStatus, ApprovalView, ProposalActionType, ProposalRisk,
    ProposedAction,
};
use crate::util::new_uuid_v4;

/// Option key holding the operating mode.
pub const AI_MODE_OPTION: &str = "wl_ai_mode";

/// Read the operating mode; defaults to `supervised` when never set
/// or when the stored value is not a known mode.
pub async fn get_ai_mode(db: &dyn Db) -> Result<AiMode, WordlessError> {
    let stored: String = wordless_cms::options::get_option(
        db,
        AI_MODE_OPTION,
        AiMode::Supervised.as_str().to_string(),
    )
    .await?;
    Ok(AiMode::from_str(&stored).unwrap_or(AiMode::Supervised))
}

/// Set the operating mode. Unknown modes are a 400.
pub async fn set_ai_mode(db: &dyn Db, mode: &str) -> Result<AiMode, WordlessError> {
    let Some(parsed) = AiMode::from_str(mode) else {
        return Err(bad_request(
            "Field \"mode\" must be one of: manual, supervised, autonomous.",
        ));
    };
    wordless_cms::options::set_option(db, AI_MODE_OPTION, &parsed.as_str().to_string()).await?;
    Ok(parsed)
}

fn row_to_approval(row: &Row) -> ApprovalView {
    let action: ProposedAction = serde_json::from_str(&row_text(row, "action"))
        .ok()
        .unwrap_or(ProposedAction {
            action_type: ProposalActionType::CreatePost,
            summary: "Unparseable proposed action.".to_string(),
            risk: ProposalRisk::High,
            payload: serde_json::json!({}),
        });
    ApprovalView {
        id: row_text(row, "id"),
        task_id: row_text(row, "task_id"),
        agent: row_text(row, "agent"),
        action,
        status: ApprovalStatus::from_str(&row_text(row, "status"))
            .unwrap_or(ApprovalStatus::Pending),
        created_by: row_int(row, "created_by"),
        decided_by: row.get("decided_by").and_then(|v| v.as_i64()),
        decided_at: row
            .get("decided_at")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        reason: row
            .get("reason")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

/// Create a pending approval for one proposed action.
pub async fn create_approval(
    db: &dyn Db,
    task_id: &str,
    agent: &str,
    action: &ProposedAction,
    created_by: i64,
) -> Result<ApprovalView, WordlessError> {
    let id = new_uuid_v4()?;
    let now = wordless_kernel::time::now_iso();
    let action_json = serde_json::to_string(action).map_err(|e| {
        WordlessError::new(
            "internal_error",
            500,
            &format!("Approval serialisation failed: {e}"),
        )
    })?;
    db.prepare(
        "INSERT INTO wl_ai_approvals (id, task_id, agent, action, status, created_by, created_at, updated_at)
         VALUES (?, ?, ?, ?, 'pending', ?, ?, ?)",
    )
    .bind(vec![
        Param::Text(id.clone()),
        Param::Text(task_id.to_string()),
        Param::Text(agent.to_string()),
        Param::Text(action_json),
        Param::Integer(created_by),
        Param::Text(now.clone()),
        Param::Text(now),
    ])
    .run()
    .await?;
    get_approval(db, &id).await
}

/// Fetch one approval by id. Unknown ids are a 404.
pub async fn get_approval(db: &dyn Db, id: &str) -> Result<ApprovalView, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT * FROM wl_ai_approvals WHERE id = ? LIMIT 1")
        .bind(vec![Param::Text(id.to_string())])
        .first::<Row>()
        .await?;
    row.map(|r| row_to_approval(&r))
        .ok_or_else(|| not_found("Approval not found."))
}

/// List approvals, optionally filtered by status, newest first.
pub async fn list_approvals(
    db: &dyn Db,
    status: Option<&str>,
    page: i64,
    per_page: i64,
) -> Result<ApprovalList, WordlessError> {
    let page = page.max(1);
    let per_page = per_page.clamp(1, 100);
    let offset = (page - 1) * per_page;
    let (where_sql, mut params): (String, Vec<Param>) = match status {
        Some(s) => (
            "WHERE status = ?".to_string(),
            vec![Param::Text(s.to_string())],
        ),
        None => (String::new(), Vec::new()),
    };
    let total: i64 = db
        .prepare(&format!(
            "SELECT COUNT(*) AS n FROM wl_ai_approvals {where_sql}"
        ))
        .bind(params.clone())
        .first::<Row>()
        .await?
        .map(|r| row_int(&r, "n"))
        .unwrap_or(0);
    params.push(Param::Integer(per_page));
    params.push(Param::Integer(offset));
    let rows: Vec<Row> = db
        .prepare(&format!(
            "SELECT * FROM wl_ai_approvals {where_sql} ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?"
        ))
        .bind(params)
        .all::<Row>()
        .await?;
    let total_pages = (total + per_page - 1) / per_page;
    Ok(ApprovalList {
        approvals: rows.iter().map(row_to_approval).collect(),
        total,
        page,
        per_page,
        total_pages: total_pages.max(1),
    })
}

/// Decide an approval. Approving applies the proposed action through
/// the applier; if applying fails the approval STAYS pending and the
/// error is reported, so a half-applied action can never be marked
/// approved. Rejecting records the reason. Deciding twice is an error.
pub async fn decide_approval(
    db: &dyn Db,
    site_id: i64,
    id: &str,
    approve: bool,
    user_id: i64,
    reason: Option<&str>,
) -> Result<ApprovalView, WordlessError> {
    let view = get_approval(db, id).await?;
    if view.status != ApprovalStatus::Pending {
        return Err(bad_request(&format!(
            "Approval has already been {}.",
            view.status.as_str()
        )));
    }
    let now = wordless_kernel::time::now_iso();
    if approve {
        // Apply first; only mark approved when the action actually applied.
        apply_proposal(db, site_id, &view.action, user_id).await?;
        db.prepare(
            "UPDATE wl_ai_approvals SET status = 'approved', decided_by = ?, decided_at = ?, updated_at = ? WHERE id = ?",
        )
        .bind(vec![
            Param::Integer(user_id),
            Param::Text(now.clone()),
            Param::Text(now),
            Param::Text(id.to_string()),
        ])
        .run()
        .await?;
    } else {
        let reason = reason
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(str::to_string);
        db.prepare(
            "UPDATE wl_ai_approvals SET status = 'rejected', decided_by = ?, decided_at = ?, reason = ?, updated_at = ? WHERE id = ?",
        )
        .bind(vec![
            Param::Integer(user_id),
            Param::Text(now.clone()),
            match &reason {
                Some(r) => Param::Text(r.clone()),
                None => Param::Null,
            },
            Param::Text(now),
            Param::Text(id.to_string()),
        ])
        .run()
        .await?;
    }
    get_approval(db, id).await
}

/// Approve an approval: apply its action, then mark it approved.
/// (Wrapper over [`decide_approval`] for the command centre.)
pub async fn approve_approval(
    db: &dyn Db,
    site_id: i64,
    id: &str,
    user_id: i64,
) -> Result<ApprovalView, WordlessError> {
    decide_approval(db, site_id, id, true, user_id, None).await
}

/// Reject an approval, recording the reason. Nothing is applied.
/// (Wrapper over [`decide_approval`] for the command centre.)
pub async fn reject_approval(
    db: &dyn Db,
    site_id: i64,
    id: &str,
    user_id: i64,
    reason: Option<&str>,
) -> Result<ApprovalView, WordlessError> {
    decide_approval(db, site_id, id, false, user_id, reason).await
}
