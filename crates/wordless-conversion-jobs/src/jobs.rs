//! Conversion jobs — moved from `wordless-conversion` in the
//! 2026-11-20 split (see the crate docs). The job *records* stay in
//! the main Worker; the pipeline runs in the `wordless-converter`
//! Worker and is reached through the kernel's `ConverterClient`.
//! This file links no conversion code.
//!
//! A conversion job is a row in `wl_jobs` (kind = `'conversion'`).
//! Processing runs synchronously inside the request in local dev —
//! the pipeline is CPU work, but a single plugin file converts in
//! milliseconds. The module is structured so the same
//! `run_conversion_job` can be invoked by a Queue/Workflow consumer
//! later; nothing here assumes it runs in a request.
//!
//! The pipeline NEVER executes the uploaded PHP. Blocked constructs
//! fail the job with a clear error; the job is never left half-done.
//!
//! One deliberate change vs the pre-split version: the stored report
//! is a [`serde_json::Value`] (the converter's report JSON, stored
//! verbatim) instead of the pipeline's `ConversionReport` struct, so
//! this crate never links the pipeline's types. The API response is
//! byte-identical.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use wordless_db::{exec_run, query_all, query_one, Db, DbError, Param, Row};
use wordless_kernel::converter::ConverterClient;
use wordless_kernel::time::now_iso;
use wordless_kernel::{bad_request, not_found, WordlessError};

/// Maximum PHP upload size (1 MB, same as v1 `MAX_PHP_BYTES`).
pub const MAX_PHP_BYTES: usize = 1024 * 1024;

/// Known fixtures (v1 `CONVERSION_FIXTURES`). Only `"hello-dolly"` in M4.
pub const CONVERSION_FIXTURES: &[(&str, &str, &str)] =
    &[("hello-dolly", "hello.php", "hello-dolly")];

/// Job status (v1 `ConversionJobStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConversionJobStatus {
    Pending,
    Running,
    Done,
    Failed,
}

impl ConversionJobStatus {
    fn as_str(self) -> &'static str {
        match self {
            ConversionJobStatus::Pending => "pending",
            ConversionJobStatus::Running => "running",
            ConversionJobStatus::Done => "done",
            ConversionJobStatus::Failed => "failed",
        }
    }

    fn from_str(s: &str) -> Self {
        match s {
            "running" => ConversionJobStatus::Running,
            "done" => ConversionJobStatus::Done,
            "failed" => ConversionJobStatus::Failed,
            _ => ConversionJobStatus::Pending,
        }
    }
}

/// One conversion job as returned by the API (v1 `ConversionJobView`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionJobView {
    pub id: String,
    pub status: ConversionJobStatus,
    /// Original uploaded filename (or the fixture's file, e.g. hello.php).
    pub filename: String,
    /// Fixture name when the job was started from a fixture.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixture: Option<String>,
    /// 0–100. Local processing is synchronous, so progress jumps 10 → 100.
    pub progress: u8,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    /// Present when status is `"done"`: the converter's report JSON,
    /// stored verbatim (see the crate docs — the report is a Value so
    /// this crate never links the pipeline's types).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<Value>,
    /// Present when status is `"failed"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A page of conversion jobs (v1 `ConversionJobList`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionJobList {
    pub jobs: Vec<ConversionJobView>,
    pub total: usize,
    pub page: usize,
    #[serde(rename = "perPage")]
    pub per_page: usize,
    #[serde(rename = "totalPages")]
    pub total_pages: usize,
}

/// Input for [`create_conversion_job`] (v1 `CreateConversionJobInput`).
#[derive(Debug, Clone, Default)]
pub struct CreateConversionJobInput {
    pub filename: String,
    /// Raw PHP source (upload path).
    pub source: Option<String>,
    /// Fixture name (fixture path). Only `"hello-dolly"` in M4.
    pub fixture: Option<String>,
}

struct JobPayload {
    filename: String,
    source: Option<String>,
    fixture: Option<String>,
}

fn parse_payload(raw: &str) -> JobPayload {
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(v) => JobPayload {
            filename: v
                .get("filename")
                .and_then(|x| x.as_str())
                .unwrap_or("unknown.php")
                .to_string(),
            source: v.get("source").and_then(|x| x.as_str()).map(str::to_string),
            fixture: v
                .get("fixture")
                .and_then(|x| x.as_str())
                .map(str::to_string),
        },
        Err(_) => JobPayload {
            filename: "unknown.php".to_string(),
            source: None,
            fixture: None,
        },
    }
}

fn row_to_view(row: &Row) -> ConversionJobView {
    use wordless_db::row::{row_opt_text, row_text};
    let status = ConversionJobStatus::from_str(&row_text(row, "status"));
    let payload = parse_payload(&row_text(row, "payload"));
    let result = row_opt_text(row, "result");
    let mut view = ConversionJobView {
        id: row_text(row, "id"),
        status,
        filename: payload.filename,
        fixture: payload.fixture,
        progress: match status {
            ConversionJobStatus::Done => 100,
            ConversionJobStatus::Running => 10,
            _ => 0,
        },
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
        report: None,
        error: None,
    };
    if let Some(result) = result {
        if !result.is_empty() {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&result) {
                match status {
                    ConversionJobStatus::Done => {
                        if parsed.get("output").is_some() {
                            // The stored report is the converter's
                            // JSON, kept verbatim.
                            view.report = Some(parsed.clone());
                            view.progress = 100;
                        }
                    }
                    ConversionJobStatus::Running => {
                        if let Some(p) = parsed.get("progress").and_then(|x| x.as_u64()) {
                            view.progress = p.min(100) as u8;
                        }
                    }
                    ConversionJobStatus::Failed => {
                        view.error = parsed
                            .get("error")
                            .and_then(|x| x.as_str())
                            .map(str::to_string)
                            .or_else(|| Some("Conversion failed.".to_string()));
                    }
                    ConversionJobStatus::Pending => {}
                }
            } else if status == ConversionJobStatus::Failed {
                view.error = Some(result);
            }
        }
    }
    view
}

async fn update_job(
    db: &dyn Db,
    id: &str,
    status: ConversionJobStatus,
    result: Option<&str>,
) -> Result<(), DbError> {
    exec_run(
        db,
        "UPDATE wl_jobs SET status = ?, result = ?, updated_at = ? WHERE id = ?",
        &[
            Param::Text(status.as_str().to_string()),
            match result {
                Some(r) => Param::Text(r.to_string()),
                None => Param::Null,
            },
            Param::Text(now_iso()),
            Param::Text(id.to_string()),
        ],
    )
    .await?;
    Ok(())
}

async fn fetch_job_row(db: &dyn Db, id: &str) -> Result<Option<Row>, DbError> {
    query_one(
        db,
        "SELECT * FROM wl_jobs WHERE id = ? AND kind = 'conversion' LIMIT 1",
        &[Param::Text(id.to_string())],
    )
    .await
}

/// Generate a random UUID (v4) without a uuid crate: 16 bytes from the
/// platform RNG, formatted per RFC 4122.
fn new_job_id() -> Result<String, WordlessError> {
    let bytes = wordless_kernel::crypto::random_bytes(16)
        .map_err(|e| wordless_kernel::internal_error(&format!("random ID failed: {e}")))?;
    let mut b = [0u8; 16];
    b.copy_from_slice(&bytes);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex = |n: u8| format!("{n:02x}");
    Ok(format!(
        "{}{}{}{}-{}{}-{}{}-{}{}-{}{}{}{}{}{}",
        hex(b[0]),
        hex(b[1]),
        hex(b[2]),
        hex(b[3]),
        hex(b[4]),
        hex(b[5]),
        hex(b[6]),
        hex(b[7]),
        hex(b[8]),
        hex(b[9]),
        hex(b[10]),
        hex(b[11]),
        hex(b[12]),
        hex(b[13]),
        hex(b[14]),
        hex(b[15]),
    ))
}

/// Create a conversion job (status `pending`). Validates the input the
/// same way v1 does: unknown fixture, missing source, non-`.php`
/// filename, and over-1 MB uploads are all rejected.
pub async fn create_conversion_job(
    db: &dyn Db,
    input: CreateConversionJobInput,
) -> Result<ConversionJobView, WordlessError> {
    let mut filename = input.filename;
    let mut source = input.source;
    let fixture = input.fixture;

    if let Some(fixture) = &fixture {
        let known = CONVERSION_FIXTURES
            .iter()
            .find(|(name, _, _)| name == fixture);
        let Some((_, known_filename, _)) = known else {
            return Err(bad_request(&format!(
                "Unknown conversion fixture \"{fixture}\". Only \"hello-dolly\" is available in M4."
            )));
        };
        filename = (*known_filename).to_string();
        source = None;
    } else {
        match &source {
            Some(s) if !s.is_empty() => {}
            _ => {
                return Err(bad_request(
                    "A conversion job needs either a PHP file upload or a fixture name.",
                ));
            }
        }
        if !filename.to_lowercase().ends_with(".php") {
            return Err(bad_request("Uploaded file must have a .php extension."));
        }
        let bytes = source.as_ref().map(|s| s.len()).unwrap_or(0);
        if bytes > MAX_PHP_BYTES {
            return Err(bad_request(&format!(
                "PHP file is {bytes} bytes; the conversion limit is {MAX_PHP_BYTES} bytes (1 MB)."
            )));
        }
    }

    let id = new_job_id()?;
    let now = now_iso();
    let payload = serde_json::json!({
        "filename": filename,
        "source": source,
        "fixture": fixture,
    });
    exec_run(
        db,
        "INSERT INTO wl_jobs (id, kind, status, payload, attempts, created_at, updated_at) VALUES (?, 'conversion', 'pending', ?, 0, ?, ?)",
        &[
            Param::Text(id.clone()),
            Param::Text(payload.to_string()),
            Param::Text(now.clone()),
            Param::Text(now),
        ],
    )
    .await
    ?;
    let row = fetch_job_row(db, &id).await?;
    let Some(row) = row else {
        return Err(wordless_kernel::internal_error(
            "Conversion job was not created.",
        ));
    };
    Ok(row_to_view(&row))
}

/// Fetch one conversion job by id.
pub async fn get_conversion_job(db: &dyn Db, id: &str) -> Result<ConversionJobView, WordlessError> {
    let row = fetch_job_row(db, id).await?;
    let Some(row) = row else {
        return Err(not_found("Conversion job not found."));
    };
    Ok(row_to_view(&row))
}

/// List conversion jobs, newest first.
pub async fn list_conversion_jobs(
    db: &dyn Db,
    page: usize,
    per_page: usize,
) -> Result<ConversionJobList, WordlessError> {
    let page = page.max(1);
    let per_page = per_page.clamp(1, 100);
    let offset = (page - 1) * per_page;
    let total_row: Option<Row> = query_one(
        db,
        "SELECT COUNT(*) AS n FROM wl_jobs WHERE kind = 'conversion'",
        &[],
    )
    .await?;
    let total = total_row
        .as_ref()
        .map(|r| wordless_db::row::row_int(r, "n"))
        .unwrap_or(0)
        .max(0) as usize;
    let rows: Vec<Row> = query_all(
        db,
        "SELECT * FROM wl_jobs WHERE kind = 'conversion' ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?",
        &[
            Param::Integer(per_page as i64),
            Param::Integer(offset as i64),
        ],
    )
    .await
    ?;
    Ok(ConversionJobList {
        jobs: rows.iter().map(row_to_view).collect(),
        total,
        page,
        per_page,
        total_pages: ((total + per_page - 1) / per_page.max(1)).max(1),
    })
}

/// Run one conversion job to completion. Safe to call from a request
/// handler (local dev) or from a Queue/Workflow consumer (production):
/// it only touches the job row and the converter service.
///
/// The conversion itself runs in the `wordless-converter` Worker,
/// reached through `converter`. When no converter is configured the
/// job fails with a clear error (fail closed) instead of pretending
/// the conversion happened.
///
/// Fixture jobs need their PHP source supplied: pass
/// `fixture_source` as `(source, filename)` — the route layer resolves
/// the fixture (fetched from upstream) and hands it in. A fixture job
/// run without it fails with a clear error, exactly as v1 does when no
/// resolver is provided.
pub async fn run_conversion_job(
    db: &dyn Db,
    job_id: &str,
    fixture_source: Option<(&str, &str)>,
    converter: Option<&dyn ConverterClient>,
) -> Result<ConversionJobView, WordlessError> {
    let row = fetch_job_row(db, job_id).await?;
    let Some(row) = row else {
        return Err(not_found("Conversion job not found."));
    };
    let payload = parse_payload(&wordless_db::row::row_text(&row, "payload"));
    let now = now_iso();
    exec_run(
        db,
        "UPDATE wl_jobs SET status = 'running', attempts = attempts + 1, result = ?, updated_at = ? WHERE id = ?",
        &[
            Param::Text(serde_json::json!({"progress": 10, "phase": "converting"}).to_string()),
            Param::Text(now),
            Param::Text(job_id.to_string()),
        ],
    )
    .await
    ?;

    let result = run_conversion_job_inner(&payload, fixture_source, converter).await;
    match result {
        Ok(report) => {
            update_job(
                db,
                job_id,
                ConversionJobStatus::Done,
                Some(&serde_json::to_string(&report).unwrap_or_default()),
            )
            .await?;
        }
        Err((message, blocked)) => {
            let mut failed = serde_json::json!({ "progress": 0, "error": message });
            if let Some(blocked) = blocked {
                failed["blocked"] = blocked;
            }
            update_job(
                db,
                job_id,
                ConversionJobStatus::Failed,
                Some(&failed.to_string()),
            )
            .await?;
        }
    }
    get_conversion_job(db, job_id).await
}

async fn run_conversion_job_inner(
    payload: &JobPayload,
    fixture_source: Option<(&str, &str)>,
    converter: Option<&dyn ConverterClient>,
) -> Result<Value, (String, Option<Value>)> {
    let mut source = payload.source.clone();
    let mut filename = payload.filename.clone();
    if let Some(fixture) = &payload.fixture {
        let Some((src, fname)) = fixture_source else {
            return Err((
                format!("No fixture resolver was provided for fixture \"{fixture}\"."),
                None,
            ));
        };
        source = Some(src.to_string());
        filename = fname.to_string();
    }
    let Some(source) = source.filter(|s| !s.is_empty()) else {
        return Err(("Conversion job has no PHP source.".to_string(), None));
    };
    let fixture_slug = payload.fixture.as_ref().and_then(|f| {
        CONVERSION_FIXTURES
            .iter()
            .find(|(name, _, _)| name == f)
            .map(|(_, _, slug)| (*slug).to_string())
    });
    let slug = fixture_slug.unwrap_or_else(|| slugify_filename(&filename));
    let Some(converter) = converter else {
        return Err((
            "Conversion service is not configured. Start the wordless-converter worker (local dev: WORDLESS_CONVERTER_URL) or bind it as a service (production: CONVERTER binding).".to_string(),
            None,
        ));
    };
    match converter.convert_php(&filename, &source, &slug).await {
        Ok(report) => Ok(report),
        Err(e) => Err((e.message, e.blocked)),
    }
}

/// Slugify a PHP filename the way the pipeline does (v1
/// `slugifyFilename`): strip a trailing `.php`, lowercase, collapse
/// runs of non-alphanumerics to one `-`. Duplicated here from the
/// pipeline (small pure helper) so this crate never links it.
fn slugify_filename(filename: &str) -> String {
    let suffix = filename
        .get(filename.len().saturating_sub(4)..)
        .unwrap_or("");
    let base = if suffix.eq_ignore_ascii_case(".php") {
        filename.get(..filename.len() - 4).unwrap_or(filename)
    } else {
        filename
    };
    let base = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let base = base.trim_matches('-').to_string();
    let mut collapsed = String::with_capacity(base.len());
    let mut last_dash = false;
    for c in base.chars() {
        if c == '-' {
            if last_dash {
                continue;
            }
            last_dash = true;
        } else {
            last_dash = false;
        }
        collapsed.push(c);
    }
    collapsed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SqliteDb;
    use async_trait::async_trait;
    use futures_executor::block_on;
    use serde_json::json;
    use wordless_kernel::converter::{ConverterClient, ConverterError};

    /// Stub converter: returns a canned Hello Dolly report for any
    /// `convert_php` call whose source contains `hello_dolly`, a
    /// sandbox-blocked error for sources containing `eval(`, and a
    /// generic converted report otherwise. The REAL conversion proof
    /// (Hello Dolly 3/3, TT25 23/76/0) lives in the converter Worker's
    /// own tests — this crate only proves the job lifecycle around
    /// whatever the converter returns.
    struct StubConverter;

    #[async_trait(?Send)]
    impl ConverterClient for StubConverter {
        async fn convert_php(
            &self,
            filename: &str,
            source: &str,
            slug: &str,
        ) -> Result<Value, ConverterError> {
            if source.contains("eval(") {
                return Err(ConverterError::blocked(
                    "Blocked by the conversion sandbox: eval.",
                    json!([{ "construct": "eval", "reason": "eval() is blocked.", "line": 1 }]),
                ));
            }
            let is_hello = source.contains("hello_dolly");
            Ok(json!({
                "source": { "filename": filename, "bytes": source.len(), "lines": source.lines().count() },
                "filesProcessed": 1,
                "ast": { "nodes": 10, "functions": if is_hello { 3 } else { 1 }, "classes": 0, "hooks": if is_hello { 2 } else { 0 } },
                "converted": {
                    "functions": if is_hello { vec!["hello_dolly_get_lyric", "hello_dolly", "dolly_css"] } else { vec!["f"] },
                    "hooks": if is_hello { vec![json!({ "kind": "action", "tag": "admin_notices" }), json!({ "kind": "action", "tag": "admin_head" })] } else { vec![] },
                },
                "unconverted": [],
                "warnings": [],
                "transformed": [],
                "output": { "code": format!("// converted {filename}\nexport function stub() {{ return 1; }}" ), "language": "typescript" },
                "extension": { "slug": slug, "name": if is_hello { "Hello Dolly" } else { slug } },
                "durationMs": 1,
            }))
        }

        async fn analyze_files(
            &self,
            _files: &[(String, String)],
        ) -> Result<Value, ConverterError> {
            Ok(json!({ "analyses": [] }))
        }

        async fn convert_files(
            &self,
            _files: &[(String, String)],
            _slug: &str,
            _kind: &str,
        ) -> Result<Value, ConverterError> {
            Ok(json!({ "generated": [], "report": null, "defaults": [] }))
        }

        async fn theme_settings(
            &self,
            _theme_json: &Value,
            _unsupported: &[String],
        ) -> Result<Value, ConverterError> {
            Ok(json!({ "carriedOver": [], "skipped": [] }))
        }

        async fn render_elementor(&self, _data: &Value) -> Result<String, ConverterError> {
            Ok(String::new())
        }
    }

    const HELLO_PHP: &str = "<?php\n/**\n * Plugin Name: Hello Dolly\n */\nfunction hello_dolly_get_lyric() { return 'Hello, Dolly'; }\nfunction hello_dolly() { echo hello_dolly_get_lyric(); }\nfunction dolly_css() { echo '<style></style>'; }\nadd_action('admin_notices', 'hello_dolly');\nadd_action('admin_head', 'dolly_css');\n";

    #[test]
    fn runs_upload_from_pending_to_done_with_report() {
        let db = SqliteDb::new();
        let created = block_on(create_conversion_job(
            &db,
            CreateConversionJobInput {
                filename: "hello.php".to_string(),
                source: Some(HELLO_PHP.to_string()),
                fixture: None,
            },
        ))
        .expect("create");
        assert_eq!(created.status, ConversionJobStatus::Pending);
        assert_eq!(created.progress, 0);
        assert_eq!(created.filename, "hello.php");

        let done = block_on(run_conversion_job(
            &db,
            &created.id,
            None,
            Some(&StubConverter),
        ))
        .expect("run");
        assert_eq!(done.status, ConversionJobStatus::Done);
        assert_eq!(done.progress, 100);
        let report = done.report.expect("report");
        assert_eq!(
            report["converted"]["functions"],
            json!(["hello_dolly_get_lyric", "hello_dolly", "dolly_css"])
        );
        assert_eq!(report["unconverted"], json!([]));
        // NOTE — divergence from the v1 test's expectation, not from v1's
        // code: v1 `service.ts` slugs an *upload* job with
        // `slugifyFilename("hello.php")`, i.e. "hello" — the fixture slug
        // "hello-dolly" applies only to fixture jobs.
        assert_eq!(report["extension"]["slug"], "hello");

        let fetched = block_on(get_conversion_job(&db, &created.id)).expect("get");
        assert_eq!(fetched.status, ConversionJobStatus::Done);
        assert_eq!(
            fetched.report.expect("report")["extension"]["name"],
            "Hello Dolly"
        );

        let list = block_on(list_conversion_jobs(&db, 1, 20)).expect("list");
        assert_eq!(list.total, 1);
        assert_eq!(list.jobs[0].id, created.id);
    }

    #[test]
    fn fails_job_on_blocked_construct_with_clear_error() {
        let db = SqliteDb::new();
        let created = block_on(create_conversion_job(
            &db,
            CreateConversionJobInput {
                filename: "bad.php".to_string(),
                source: Some("<?php function f($x) { return eval($x); }".to_string()),
                fixture: None,
            },
        ))
        .expect("create");
        let failed = block_on(run_conversion_job(
            &db,
            &created.id,
            None,
            Some(&StubConverter),
        ))
        .expect("run");
        assert_eq!(failed.status, ConversionJobStatus::Failed);
        let error = failed.error.expect("error");
        assert!(error.to_lowercase().contains("eval"), "{error}");
        assert!(failed.report.is_none());
        assert_eq!(failed.progress, 0);
    }

    #[test]
    fn fails_job_with_clear_error_when_no_converter_is_configured() {
        let db = SqliteDb::new();
        let created = block_on(create_conversion_job(
            &db,
            CreateConversionJobInput {
                filename: "hello.php".to_string(),
                source: Some(HELLO_PHP.to_string()),
                fixture: None,
            },
        ))
        .expect("create");
        let failed = block_on(run_conversion_job(&db, &created.id, None, None)).expect("run");
        assert_eq!(failed.status, ConversionJobStatus::Failed);
        assert!(
            failed.error.expect("error").contains("not configured"),
            "a missing converter must fail the job clearly, not hang or panic"
        );
    }

    #[test]
    fn rejects_non_php_upload_unknown_fixture_and_missing_source() {
        let db = SqliteDb::new();
        let err = block_on(create_conversion_job(
            &db,
            CreateConversionJobInput {
                filename: "notes.txt".to_string(),
                source: Some("hello".to_string()),
                fixture: None,
            },
        ))
        .expect_err("non-php rejected");
        assert!(err.to_string().contains(".php"), "{err}");
        let err = block_on(create_conversion_job(
            &db,
            CreateConversionJobInput {
                filename: "hello.php".to_string(),
                source: None,
                fixture: Some("not-a-fixture".to_string()),
            },
        ))
        .expect_err("unknown fixture rejected");
        assert!(
            err.to_string().contains("Unknown conversion fixture"),
            "{err}"
        );
        let err = block_on(create_conversion_job(
            &db,
            CreateConversionJobInput {
                filename: "hello.php".to_string(),
                source: None,
                fixture: None,
            },
        ))
        .expect_err("missing source rejected");
        assert!(
            err.to_string()
                .contains("either a PHP file upload or a fixture name"),
            "{err}"
        );
    }

    #[test]
    fn runs_fixture_job_through_supplied_source() {
        let db = SqliteDb::new();
        let created = block_on(create_conversion_job(
            &db,
            CreateConversionJobInput {
                filename: "hello.php".to_string(),
                source: None,
                fixture: Some("hello-dolly".to_string()),
            },
        ))
        .expect("create");
        assert_eq!(created.status, ConversionJobStatus::Pending);
        assert_eq!(created.fixture.as_deref(), Some("hello-dolly"));
        let done = block_on(run_conversion_job(
            &db,
            &created.id,
            Some((HELLO_PHP, "hello.php")),
            Some(&StubConverter),
        ))
        .expect("run");
        assert_eq!(done.status, ConversionJobStatus::Done);
        assert_eq!(
            done.report.expect("report")["extension"]["slug"],
            "hello-dolly"
        );
    }

    #[test]
    fn fails_fixture_job_when_no_source_is_supplied() {
        let db = SqliteDb::new();
        let created = block_on(create_conversion_job(
            &db,
            CreateConversionJobInput {
                filename: "hello.php".to_string(),
                source: None,
                fixture: Some("hello-dolly".to_string()),
            },
        ))
        .expect("create");
        let failed = block_on(run_conversion_job(
            &db,
            &created.id,
            None,
            Some(&StubConverter),
        ))
        .expect("run");
        assert_eq!(failed.status, ConversionJobStatus::Failed);
        assert!(
            failed.error.expect("error").contains("No fixture resolver"),
            "fixture job without a source must fail clearly"
        );
    }

    #[test]
    fn get_unknown_job_is_not_found() {
        let db = SqliteDb::new();
        let err = block_on(get_conversion_job(&db, "no-such-id")).expect_err("missing job");
        assert!(err.to_string().contains("not found"), "{err}");
    }

    #[test]
    fn slugify_matches_the_pipeline_rules() {
        assert_eq!(slugify_filename("hello.php"), "hello");
        assert_eq!(slugify_filename("My Plugin.PHP"), "my-plugin");
        assert_eq!(slugify_filename("hello-dolly.php"), "hello-dolly");
        assert_eq!(slugify_filename("a__b.php"), "a-b");
    }
}
