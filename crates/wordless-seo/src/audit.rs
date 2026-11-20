//! SEO audit runs (P10, doc §0.1 F + §3 P10).
//!
//! Table `wl_seo_audit_runs` (migration 0016) stores Lighthouse /
//! structured-data check results, timestamped. Scores are recorded
//! exactly as the tool measured them — the Worker never generates a
//! score, and a NULL score means "not measured", shown as such in
//! the admin audit screen. A run is append-only history: no update
//! endpoint exists.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wordless_db::row::row_text;
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, not_found, WordlessError};

/// One recorded audit run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditRun {
    pub id: i64,
    pub url: String,
    pub tool: String,
    pub seo_score: Option<i64>,
    pub accessibility_score: Option<i64>,
    pub performance_score: Option<i64>,
    pub best_practices_score: Option<i64>,
    pub lcp_ms: Option<i64>,
    pub inp_ms: Option<i64>,
    /// CLS stored ×1000 (0.05 → 50) so the DB holds an integer.
    pub cls_milli: Option<i64>,
    pub notes: String,
    pub created_at: String,
}

/// Input for recording a run. Scores are 0–100 or absent; vitals
/// are non-negative or absent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AuditRunInput {
    pub url: String,
    pub tool: Option<String>,
    pub seo_score: Option<i64>,
    pub accessibility_score: Option<i64>,
    pub performance_score: Option<i64>,
    pub best_practices_score: Option<i64>,
    pub lcp_ms: Option<i64>,
    pub inp_ms: Option<i64>,
    pub cls_milli: Option<i64>,
    pub notes: Option<String>,
}

fn opt_string(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn score_field(body: &Value, field: &str) -> Result<Option<i64>, WordlessError> {
    let Some(v) = body.get(field) else {
        return Ok(None);
    };
    if v.is_null() {
        return Ok(None);
    }
    let Some(n) = v.as_i64() else {
        return Err(bad_request(&format!(
            "Field \"{field}\" must be an integer 0-100."
        )));
    };
    if !(0..=100).contains(&n) {
        return Err(bad_request(&format!(
            "Field \"{field}\" must be between 0 and 100."
        )));
    }
    Ok(Some(n))
}

fn vital_field(body: &Value, field: &str) -> Result<Option<i64>, WordlessError> {
    let Some(v) = body.get(field) else {
        return Ok(None);
    };
    if v.is_null() {
        return Ok(None);
    }
    let Some(n) = v.as_i64() else {
        return Err(bad_request(&format!(
            "Field \"{field}\" must be a non-negative integer."
        )));
    };
    if n < 0 {
        return Err(bad_request(&format!(
            "Field \"{field}\" must be a non-negative integer."
        )));
    }
    Ok(Some(n))
}

/// Parse + validate a record-run body. At least one score or vital
/// must be present — an empty run is not history, it is a typo.
pub fn audit_run_input_from_json(body: &Value) -> Result<AuditRunInput, WordlessError> {
    let url = body
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if url.is_empty() || url.len() > 2000 {
        return Err(bad_request(
            "Field \"url\" must be a non-empty URL (max 2000 characters).",
        ));
    }
    if !(url.starts_with("http://") || url.starts_with("https://") || url.starts_with('/')) {
        return Err(bad_request(
            "Field \"url\" must be an absolute URL or a site path.",
        ));
    }
    let tool = opt_string(body.get("tool").unwrap_or(&Value::Null))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "lighthouse".to_string());
    if tool.len() > 100 {
        return Err(bad_request(
            "Field \"tool\" must be at most 100 characters.",
        ));
    }
    let notes = opt_string(body.get("notes").unwrap_or(&Value::Null)).unwrap_or_default();
    if notes.len() > 2000 {
        return Err(bad_request(
            "Field \"notes\" must be at most 2000 characters.",
        ));
    }
    let input = AuditRunInput {
        url,
        tool: Some(tool),
        seo_score: score_field(body, "seoScore")?,
        accessibility_score: score_field(body, "accessibilityScore")?,
        performance_score: score_field(body, "performanceScore")?,
        best_practices_score: score_field(body, "bestPracticesScore")?,
        lcp_ms: vital_field(body, "lcpMs")?,
        inp_ms: vital_field(body, "inpMs")?,
        cls_milli: vital_field(body, "clsMilli")?,
        notes: Some(notes),
    };
    let any = input.seo_score.is_some()
        || input.accessibility_score.is_some()
        || input.performance_score.is_some()
        || input.best_practices_score.is_some()
        || input.lcp_ms.is_some()
        || input.inp_ms.is_some()
        || input.cls_milli.is_some();
    if !any {
        return Err(bad_request(
            "An audit run must record at least one score or vital.",
        ));
    }
    Ok(input)
}

/// The column as an integer, or `None` when it is missing, null, or
/// non-numeric. NULL means "not measured" — never 0.
fn row_opt_i64(row: &Row, key: &str) -> Option<i64> {
    match row.get(key) {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Some(Value::String(s)) => s.parse::<i64>().ok(),
        _ => None,
    }
}

fn row_to_run(row: &Row) -> AuditRun {
    AuditRun {
        id: row_opt_i64(row, "id").unwrap_or(0),
        url: row_text(row, "url"),
        tool: row_text(row, "tool"),
        seo_score: row_opt_i64(row, "seo_score"),
        accessibility_score: row_opt_i64(row, "accessibility_score"),
        performance_score: row_opt_i64(row, "performance_score"),
        best_practices_score: row_opt_i64(row, "best_practices_score"),
        lcp_ms: row_opt_i64(row, "lcp_ms"),
        inp_ms: row_opt_i64(row, "inp_ms"),
        cls_milli: row_opt_i64(row, "cls_milli"),
        notes: row_text(row, "notes"),
        created_at: row_text(row, "created_at"),
    }
}

fn opt_param(v: Option<i64>) -> Param {
    match v {
        Some(n) => Param::Integer(n),
        None => Param::Null,
    }
}

pub async fn record_audit_run(
    db: &dyn Db,
    site_id: i64,
    input: AuditRunInput,
) -> Result<AuditRun, WordlessError> {
    let now = wordless_kernel::time::now_iso();
    let meta = db
        .exec(
            "INSERT INTO wl_seo_audit_runs (site_id, url, tool, seo_score, accessibility_score, performance_score, best_practices_score, lcp_ms, inp_ms, cls_milli, notes, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            &[
                Param::Integer(site_id),
                Param::Text(input.url.clone()),
                Param::Text(input.tool.clone().unwrap_or_else(|| "lighthouse".to_string())),
                opt_param(input.seo_score),
                opt_param(input.accessibility_score),
                opt_param(input.performance_score),
                opt_param(input.best_practices_score),
                opt_param(input.lcp_ms),
                opt_param(input.inp_ms),
                opt_param(input.cls_milli),
                Param::Text(input.notes.clone().unwrap_or_default()),
                Param::Text(now),
            ],
        )
        .await
        .map_err(WordlessError::from)?;
    let id = meta.last_row_id.unwrap_or(0);
    get_audit_run(db, site_id, id)
        .await?
        .ok_or_else(|| not_found("Audit run not found after insert."))
}

pub async fn get_audit_run(db: &dyn Db, site_id: i64, id: i64) -> Result<Option<AuditRun>, WordlessError> {
    let row = db
        .query_one(
            "SELECT * FROM wl_seo_audit_runs WHERE id = ? AND site_id = ? LIMIT 1",
            &[Param::Integer(id), Param::Integer(site_id)],
        )
        .await
        .map_err(WordlessError::from)?;
    Ok(row.as_ref().map(row_to_run))
}

pub async fn list_audit_runs(db: &dyn Db, site_id: i64, limit: i64) -> Result<Vec<AuditRun>, WordlessError> {
    let rows = db
        .query_all(
            "SELECT * FROM wl_seo_audit_runs WHERE site_id = ? ORDER BY created_at DESC, id DESC LIMIT ?",
            &[Param::Integer(site_id), Param::Integer(limit.clamp(1, 100))],
        )
        .await
        .map_err(WordlessError::from)?;
    Ok(rows.iter().map(row_to_run).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SqliteDb;
    use futures_executor::block_on;

    fn body(json: serde_json::Value) -> Value {
        json
    }

    #[test]
    fn records_a_run_and_lists_it_newest_first() {
        let db = SqliteDb::new();
        let input = audit_run_input_from_json(&body(serde_json::json!({
            "url": "https://example.test/hello",
            "seoScore": 100,
            "accessibilityScore": 97,
            "performanceScore": 92,
            "lcpMs": 1800,
            "clsMilli": 40
        })))
        .unwrap();
        let run = block_on(record_audit_run(&db, 1, input)).unwrap();
        assert_eq!(run.url, "https://example.test/hello");
        assert_eq!(run.tool, "lighthouse");
        assert_eq!(run.seo_score, Some(100));
        assert_eq!(run.cls_milli, Some(40));
        assert_eq!(run.inp_ms, None);
        let listed = block_on(list_audit_runs(&db, 1, 10)).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, run.id);
        assert_eq!(block_on(get_audit_run(&db, 1, 9999)).unwrap(), None);
    }

    #[test]
    fn rejects_an_empty_run_a_bad_score_and_a_bad_url() {
        let db = SqliteDb::new();
        assert!(audit_run_input_from_json(&body(
            serde_json::json!({"url": "https://example.test/"})
        ))
        .is_err());
        assert!(audit_run_input_from_json(&body(
            serde_json::json!({"url": "https://example.test/", "seoScore": 101})
        ))
        .is_err());
        assert!(audit_run_input_from_json(&body(
            serde_json::json!({"url": "ftp://example.test/", "seoScore": 90})
        ))
        .is_err());
        assert!(audit_run_input_from_json(&body(
            serde_json::json!({"url": "https://example.test/", "lcpMs": -5})
        ))
        .is_err());
        let _ = db;
    }
}
