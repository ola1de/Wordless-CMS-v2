//! Hook dispatch — invoke one hook across all loaded extensions,
//! with the kill switch re-checked on every invocation.
//!
//! The registry is re-read per extension immediately before dispatch:
//! disabling an extension stops its hooks within one request. Every
//! invocation — including a skip — is recorded in
//! `wl_extension_runs`, so the admin can see exactly what ran, what
//! was denied, and what timed out.
//!
//! The executor and the host token for a slug come from the caller
//! (`resolve`): the worker crate owns binding resolution (service
//! binding / dispatch namespace / HTTP) and token derivation (the
//! deployment secret never leaves the worker crate).

use serde_json::Value;
use wordless_db::Db;
use wordless_kernel::error::WordlessError;

use crate::executor::{ExtensionExecutor, InvokeRequest, DEFAULT_INVOKE_TIMEOUT_MS};
use crate::registry::{get_instance, list_instances, record_run, InstanceStatus};

/// The outcome of invoking one hook across all loaded extensions.
#[derive(Debug, Clone)]
pub struct HookOutcome {
    pub hook: String,
    pub kind: String,
    /// For filters: the final value after every extension filtered it.
    /// For actions: `Null` (the output is in `rendered`).
    pub value: Value,
    /// Strings the extensions' action callbacks rendered, in registry
    /// order.
    pub rendered: Vec<String>,
    pub per_extension: Vec<ExtensionHookResult>,
}

/// One extension's part in a hook invocation.
#[derive(Debug, Clone)]
pub struct ExtensionHookResult {
    pub slug: String,
    /// `ok` | `error` | `timeout` | `skipped_disabled`.
    pub status: String,
    pub rendered: Vec<String>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

/// Invoke one hook across every loaded extension.
///
/// * `kind` is `"action"` or `"filter"`. For a filter, `args[0]` is
///   the value and each extension's filtered value feeds the next.
/// * `resolve` builds `(executor, host_token)` for a slug, or `None`
///   when the extension cannot be reached (recorded as an error, and
///   the other extensions still run).
/// * `timeout_ms` is 0 for the default ([`DEFAULT_INVOKE_TIMEOUT_MS`]).
pub async fn invoke_hook<F, E>(
    db: &dyn Db,
    hook: &str,
    kind: &str,
    args: Vec<Value>,
    host_url: &str,
    actor_user_id: i64,
    locale: &str,
    timeout_ms: u64,
    resolve: F,
) -> Result<HookOutcome, WordlessError>
where
    F: Fn(&str) -> Option<(E, String)>,
    E: ExtensionExecutor,
{
    let instances = list_instances(db).await?;
    let timeout_ms = if timeout_ms == 0 {
        DEFAULT_INVOKE_TIMEOUT_MS
    } else {
        timeout_ms
    };
    let mut value = if kind == "filter" {
        args.first().cloned().unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let mut rendered: Vec<String> = Vec::new();
    let mut per_extension: Vec<ExtensionHookResult> = Vec::new();

    for inst in instances {
        // Kill switch: re-read this extension's status right before
        // dispatch. A disable that landed after list_instances must
        // still stop this invocation.
        let fresh = get_instance(db, &inst.slug).await?;
        let status = fresh
            .as_ref()
            .map(|i| i.status.clone())
            .unwrap_or_else(|| InstanceStatus::Disabled.as_str().to_string());
        if status != InstanceStatus::Loaded.as_str() {
            record_run(
                db,
                &inst.slug,
                hook,
                "skipped_disabled",
                0,
                None,
                Some("extension is disabled"),
            )
            .await?;
            per_extension.push(ExtensionHookResult {
                slug: inst.slug,
                status: "skipped_disabled".to_string(),
                rendered: vec![],
                error: Some("extension is disabled".to_string()),
                duration_ms: 0,
            });
            continue;
        }
        let Some((executor, host_token)) = resolve(&inst.slug) else {
            record_run(
                db,
                &inst.slug,
                hook,
                "error",
                0,
                None,
                Some("no executor could be built for this extension"),
            )
            .await?;
            per_extension.push(ExtensionHookResult {
                slug: inst.slug,
                status: "error".to_string(),
                rendered: vec![],
                error: Some("no executor could be built for this extension".to_string()),
                duration_ms: 0,
            });
            continue;
        };
        let mut call_args = args.clone();
        if kind == "filter" {
            // The current value feeds the next extension.
            if call_args.is_empty() {
                call_args.push(value.clone());
            } else {
                call_args[0] = value.clone();
            }
        }
        let req = InvokeRequest {
            slug: inst.slug.clone(),
            hook: hook.to_string(),
            kind: kind.to_string(),
            args: call_args,
            host_url: host_url.to_string(),
            host_token,
            actor_user_id,
            locale: locale.to_string(),
            timeout_ms,
        };
        let outcome = executor.invoke(&req).await;
        match outcome {
            Ok(result) => {
                let run_status = if result.timed_out {
                    "timeout"
                } else if result.ok {
                    "ok"
                } else {
                    "error"
                };
                let summary = if result.rendered.is_empty() {
                    None
                } else {
                    Some(format!("{} rendered string(s)", result.rendered.len()))
                };
                record_run(
                    db,
                    &inst.slug,
                    hook,
                    run_status,
                    result.duration_ms as i64,
                    summary.as_deref(),
                    result.error.as_deref(),
                )
                .await?;
                if kind == "filter" && result.ok {
                    value = result.result.clone();
                }
                rendered.extend(result.rendered.iter().cloned());
                per_extension.push(ExtensionHookResult {
                    slug: inst.slug,
                    status: run_status.to_string(),
                    rendered: result.rendered,
                    error: result.error,
                    duration_ms: result.duration_ms,
                });
            }
            Err(e) => {
                record_run(db, &inst.slug, hook, "error", 0, None, Some(&e)).await?;
                per_extension.push(ExtensionHookResult {
                    slug: inst.slug,
                    status: "error".to_string(),
                    rendered: vec![],
                    error: Some(e),
                    duration_ms: 0,
                });
            }
        }
    }

    Ok(HookOutcome {
        hook: hook.to_string(),
        kind: kind.to_string(),
        value,
        rendered,
        per_extension,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::test_support_transport::FakeTransport;
    use crate::executor::HttpExecutor;
    use crate::registry::{record_instance, set_instance_status};
    use crate::test_support::{block_on, test_db, SCHEMA_P6};
    use crate::Capability;

    fn db() -> impl Db {
        let db = test_db();
        block_on(async {
            db.exec(SCHEMA_P6, &[]).await.unwrap();
        });
        db
    }

    fn resolve_ok(
        transport: FakeTransport,
    ) -> impl Fn(&str) -> Option<(HttpExecutor<FakeTransport>, String)> {
        move |slug: &str| {
            if slug == "unreachable" {
                None
            } else {
                Some((
                    HttpExecutor::new(transport.clone(), "https://ext-wordless.internal"),
                    "tok".to_string(),
                ))
            }
        }
    }

    #[test]
    fn action_hook_runs_loaded_and_skips_disabled() {
        let db = db();
        block_on(async {
            record_instance(
                &db,
                "hello-dolly",
                "1.7.2",
                &[Capability::HooksRegister],
                "k",
                "h",
                None,
            )
            .await
            .unwrap();
            record_instance(
                &db,
                "sleepy",
                "1.0",
                &[Capability::HooksRegister],
                "k",
                "h",
                None,
            )
            .await
            .unwrap();
            set_instance_status(&db, "sleepy", InstanceStatus::Disabled)
                .await
                .unwrap();

            let transport = FakeTransport::new();
            transport.respond(
                "__wordless/invoke",
                200,
                r#"{"ok":true,"hook":"admin_notices","kind":"action","result":[null],"rendered":["<p>hi</p>"]}"#,
            );
            let outcome = invoke_hook(
                &db,
                "admin_notices",
                "action",
                vec![],
                "http://127.0.0.1:8799",
                1,
                "en_US",
                0,
                resolve_ok(transport.clone()),
            )
            .await
            .unwrap();
            assert_eq!(outcome.rendered, vec!["<p>hi</p>".to_string()]);
            assert_eq!(outcome.per_extension.len(), 2);
            assert_eq!(outcome.per_extension[0].status, "ok");
            assert_eq!(outcome.per_extension[1].status, "skipped_disabled");
            // The skip is recorded, not silent.
            let runs = crate::registry::list_runs(&db, "sleepy", 10).await.unwrap();
            assert_eq!(runs.len(), 1);
            assert_eq!(runs[0].status, "skipped_disabled");
        });
    }

    #[test]
    fn filter_threads_value_through_extensions() {
        let db = db();
        block_on(async {
            record_instance(
                &db,
                "a",
                "1.0",
                &[Capability::HooksRegister],
                "k",
                "h",
                None,
            )
            .await
            .unwrap();
            let transport = FakeTransport::new();
            transport.respond(
                "__wordless/invoke",
                200,
                r#"{"ok":true,"hook":"the_content","kind":"filter","result":"FILTERED","rendered":[]}"#,
            );
            let outcome = invoke_hook(
                &db,
                "the_content",
                "filter",
                vec![Value::String("orig".to_string())],
                "http://127.0.0.1:8799",
                0,
                "en_US",
                0,
                resolve_ok(transport.clone()),
            )
            .await
            .unwrap();
            assert_eq!(outcome.value, Value::String("FILTERED".to_string()));
        });
    }

    #[test]
    fn timeout_is_recorded_not_fatal() {
        let db = db();
        block_on(async {
            record_instance(
                &db,
                "slow",
                "1.0",
                &[Capability::HooksRegister],
                "k",
                "h",
                None,
            )
            .await
            .unwrap();
            let transport = FakeTransport::new();
            transport.respond(
                "__wordless/invoke",
                200,
                r#"{"ok":false,"hook":"x","kind":"action","error":"extension slow timed out after 5000 ms (hook x)"}"#,
            );
            let outcome = invoke_hook(
                &db,
                "x",
                "action",
                vec![],
                "http://127.0.0.1:8799",
                0,
                "en_US",
                DEFAULT_INVOKE_TIMEOUT_MS,
                resolve_ok(transport.clone()),
            )
            .await
            .unwrap();
            assert_eq!(outcome.per_extension[0].status, "error");
            assert!(outcome.per_extension[0]
                .error
                .as_deref()
                .unwrap()
                .contains("timed out"));
        });
    }
}
