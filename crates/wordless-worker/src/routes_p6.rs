//! P6 route table — the extension runtime (v2-only routes, no v1
//! counterpart): load/unload, permissions, logs, script, the
//! token-authenticated host endpoint, and hook invocation.
//!
//! Registered from `routes::build_router` after the P3 table.
//!
//! Security model (doc §3 P6, §9.4 decide-now #4): an extension gets
//! only declared capabilities, nothing else. The load gate refuses
//! plugins whose conversion report has blocked files, whose code
//! calls a host method that does not exist, uses `eval`/`Function`/
//! dynamic `import`, or fetches the network without declared hosts.
//! The host endpoint checks the per-load token first, then the
//! granted capability, then runs the real implementation — and logs
//! every denial.

use std::collections::HashMap;
use std::rc::Rc;

use serde_json::{json, Value};
use wordless_cms::menus::{get_menu, list_menu_items};
use wordless_cms::posts::{
    create_post, delete_post, get_post_by_id, list_posts, update_post, CreatePostInput,
    ListPostsOptions, PostStatus, UpdatePostInput,
};
use wordless_cms::revisions::{get_revision, list_revisions, restore_revision};
use wordless_cms::terms::{get_post_terms, list_terms, ListTermsOptions};
use wordless_db::options::{get_option as db_get_option, set_option as db_set_option};
use wordless_db::row::{row_opt_text, row_text};
use wordless_db::{Db, Param, Row};
use wordless_extensions::{
    capability_for_wp_call, derive_host_token, evaluate_load_gate, generate_extension_project,
    get_instance, invoke_hook, list_instances, list_logs, list_runs, record_instance, record_log,
    set_instance_permissions, set_instance_status, token_hash, token_matches, Capability,
    ConvertedFile, ExtensionInstance, ExtensionProjectInput, HostCall, HostResult, InstanceStatus,
    LoadDecision, ALL_CAPABILITIES,
};
use wordless_extensions::{delete_instance, ExtensionExecutor};
use wordless_integrations::extensions::{get_extension, list_extensions};
use wordless_kernel::envelope::RouteResponse;
use wordless_kernel::error::{bad_request, internal_error, not_found, WordlessError};
use wordless_kernel::router::{read_json_body, RequestContext, RouteOptions, Router};
use wordless_kernel::storage::ObjectStore;
use wordless_media::service::get_media_record;

use crate::context::Ctx;
use crate::routes::{actor, ok, ok_json, require_store};

/// Register every P6 route on the router built by `routes.rs`.
pub fn register_p6_routes(router: &mut Router<Ctx>) {
    // ------------------------------------------------------- extensions
    // NOTE: GET /api/extensions stays the P3 route (an array of
    // ExtensionRecord, capability install_plugins) — the router returns
    // the FIRST matching registration, so a second GET /api/extensions
    // here would be dead code AND would break the Conversion page and
    // the parity corpus step that expect the P3 array. The runtime
    // list lives at /api/extensions/runtime instead.
    router.get(
        "/api/extensions/runtime",
        |ctx| {
            Box::pin(async move {
                let instances = list_instances(ctx.db.as_ref()).await?;
                let installed = list_extensions(ctx.db.as_ref()).await?;
                let loaded: HashMap<String, bool> =
                    instances.iter().map(|i| (i.slug.clone(), true)).collect();
                let items: Vec<Value> = instances
                    .iter()
                    .map(|i| {
                        let name = installed
                            .iter()
                            .find(|e| e.slug == i.slug)
                            .map(|e| e.name.clone())
                            .unwrap_or_else(|| i.slug.clone());
                        json!({
                            "slug": i.slug, "name": name, "version": i.version,
                            "status": i.status, "permissions": i.permissions,
                            "scriptKey": i.script_key, "devUrl": i.dev_url,
                            "createdAt": i.created_at, "updatedAt": i.updated_at,
                        })
                    })
                    .collect();
                let not_loaded: Vec<Value> = installed
                    .iter()
                    .filter(|e| !loaded.contains_key(&e.slug))
                    .map(|e| {
                        json!({
                            "slug": e.slug, "name": e.name, "kind": e.kind,
                            "version": e.version, "status": e.status,
                            "loaded": false, "report": e.report,
                        })
                    })
                    .collect();
                ok(json!({ "loaded": items, "installed": not_loaded }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/extensions/:slug",
        |ctx| {
            Box::pin(async move {
                let slug = slug_param(ctx);
                let inst = require_instance(ctx.db.as_ref(), &slug).await?;
                let ext = get_extension(ctx.db.as_ref(), &slug).await.ok();
                ok(
                    json!({
                        "slug": inst.slug, "version": inst.version, "status": inst.status,
                        "permissions": inst.permissions, "scriptKey": inst.script_key,
                        "devUrl": inst.dev_url, "createdAt": inst.created_at,
                        "updatedAt": inst.updated_at, "tokenEpoch": inst.token_epoch,
                        "name": ext.as_ref().map(|e| e.name.clone()),
                        "report": ext.as_ref().and_then(|e| e.report.clone()),
                    }),
                    200,
                )
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.post(
        "/api/extensions/:slug/load",
        |ctx| Box::pin(async move { load_extension(ctx).await }),
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.post(
        "/api/extensions/:slug/unload",
        |ctx| {
            Box::pin(async move {
                let slug = slug_param(ctx);
                let removed = delete_instance(ctx.db.as_ref(), &slug).await?;
                if !removed {
                    return Err(not_found("Extension instance not found."));
                }
                ok(json!({ "slug": slug, "loaded": false }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.post(
        "/api/extensions/:slug/disable",
        |ctx| {
            Box::pin(async move {
                let slug = slug_param(ctx);
                let inst =
                    set_instance_status(ctx.db.as_ref(), &slug, InstanceStatus::Disabled).await?;
                ok(json!({ "slug": inst.slug, "status": inst.status }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.post(
        "/api/extensions/:slug/enable",
        |ctx| {
            Box::pin(async move {
                let slug = slug_param(ctx);
                let inst =
                    set_instance_status(ctx.db.as_ref(), &slug, InstanceStatus::Loaded).await?;
                ok(json!({ "slug": inst.slug, "status": inst.status }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/extensions/:slug/permissions",
        |ctx| {
            Box::pin(async move {
                let slug = slug_param(ctx);
                let inst = require_instance(ctx.db.as_ref(), &slug).await?;
                let available: Vec<Value> = ALL_CAPABILITIES
                    .iter()
                    .map(|c| {
                        json!({
                            "id": c.id(), "description": c.description(),
                            "grantedByDefault": c.granted_by_default(),
                            "granted": inst.permissions.contains(&c.id().to_string()),
                        })
                    })
                    .collect();
                ok(json!({ "slug": inst.slug, "permissions": inst.permissions, "available": available }), 200)
            })
        },
        RouteOptions { require_auth: false, capability: Some("manage_options") },
    );

    router.post(
        "/api/extensions/:slug/permissions",
        |ctx| {
            Box::pin(async move {
                let slug = slug_param(ctx);
                let body = read_json_body(ctx)?;
                let ids = body
                    .get("permissions")
                    .and_then(Value::as_array)
                    .ok_or_else(|| bad_request("Body must have a \"permissions\" array."))?;
                let mut caps: Vec<Capability> = Vec::new();
                for id in ids {
                    let Some(s) = id.as_str() else {
                        return Err(bad_request("Permissions must be strings."));
                    };
                    let Some(cap) = Capability::from_id(s) else {
                        return Err(bad_request(&format!("Unknown capability: {s}")));
                    };
                    if !caps.contains(&cap) {
                        caps.push(cap);
                    }
                }
                let inst = set_instance_permissions(ctx.db.as_ref(), &slug, &caps).await?;
                ok(
                    json!({ "slug": inst.slug, "permissions": inst.permissions }),
                    200,
                )
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/extensions/:slug/logs",
        |ctx| {
            Box::pin(async move {
                let slug = slug_param(ctx);
                let limit = ctx
                    .query("limit")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(50)
                    .clamp(1, 200);
                let logs = list_logs(ctx.db.as_ref(), &slug, limit).await?;
                ok(json!({ "slug": slug, "logs": logs }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/extensions/:slug/runs",
        |ctx| {
            Box::pin(async move {
                let slug = slug_param(ctx);
                let limit = ctx
                    .query("limit")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(50)
                    .clamp(1, 200);
                let runs = list_runs(ctx.db.as_ref(), &slug, limit).await?;
                ok(json!({ "slug": slug, "runs": runs }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/extensions/:slug/script",
        |ctx| {
            Box::pin(async move {
                let slug = slug_param(ctx);
                let source = load_source_from_query(ctx);
                let inst = require_instance(ctx.db.as_ref(), &slug).await?;
                let store = require_store(ctx)?;
                let files = load_converted_files(store.as_ref(), &slug, source).await?;
                let project = generate_extension_project(&ExtensionProjectInput {
                    slug: &slug,
                    version: &inst.version,
                    files: &files,
                    declared_hosts: &declared_hosts(store.as_ref(), &slug, source).await?,
                    locale: "en_US",
                });
                ok(
                    json!({
                        "slug": slug, "entry": project.entry, "shim": project.shim,
                        "wordpressDts": project.wordpress_dts, "wranglerToml": project.wrangler_toml,
                        "convertedPaths": project.converted_paths,
                    }),
                    200,
                )
            })
        },
        RouteOptions { require_auth: false, capability: Some("manage_options") },
    );

    router.post(
        "/api/extensions/:slug/host",
        |ctx| Box::pin(async move { host_endpoint(ctx).await }),
        // No session auth: the per-load bearer token IS the auth.
        RouteOptions::default(),
    );

    router.post(
        "/api/extensions/:slug/invoke",
        |ctx| {
            Box::pin(async move {
                let slug = slug_param(ctx);
                let body = read_json_body(ctx)?;
                let hook = body
                    .get("hook")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if hook.is_empty() {
                    return Err(bad_request("Body must have a \"hook\" string."));
                }
                let kind = body
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("action")
                    .to_string();
                let args = body
                    .get("args")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let outcome = invoke_one(ctx, &slug, &hook, &kind, args).await?;
                ok_json(
                    json!({
                        "slug": slug, "hook": outcome.hook, "kind": outcome.kind,
                        "value": outcome.value, "rendered": outcome.rendered,
                        "perExtension": outcome.per_extension.iter().map(|p| json!({
                            "slug": p.slug, "status": p.status, "rendered": p.rendered,
                            "error": p.error, "durationMs": p.duration_ms,
                        })).collect::<Vec<_>>(),
                    }),
                    200,
                )
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.post(
        "/api/extensions/invoke-all",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let hook = body
                    .get("hook")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if hook.is_empty() {
                    return Err(bad_request("Body must have a \"hook\" string."));
                }
                let kind = body
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("action")
                    .to_string();
                let args = body
                    .get("args")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let outcome = invoke_all(ctx, &hook, &kind, args).await?;
                ok_json(
                    json!({
                        "hook": outcome.hook, "kind": outcome.kind,
                        "value": outcome.value, "rendered": outcome.rendered,
                        "perExtension": outcome.per_extension.iter().map(|p| json!({
                            "slug": p.slug, "status": p.status, "rendered": p.rendered,
                            "error": p.error, "durationMs": p.duration_ms,
                        })).collect::<Vec<_>>(),
                    }),
                    200,
                )
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );
}

fn slug_param(ctx: &Ctx) -> String {
    ctx.param("slug").unwrap_or("").to_string()
}

async fn require_instance(db: &dyn Db, slug: &str) -> Result<ExtensionInstance, WordlessError> {
    get_instance(db, slug)
        .await?
        .ok_or_else(|| not_found("Extension instance not found. Load it first."))
}

/// Load (or reload) an extension: run the gate, generate the Worker
/// project, store it in R2, record the instance with a fresh token.
///
/// `source` (body, default `"extension"`): `"extension"` loads a
/// marketplace plugin from `extensions/<slug>/` with the report from
/// `wl_extensions`; `"theme"` loads a theme's converted `functions.php`
/// from `themes/<slug>/` with the conversion report persisted in
/// `wl_themes.report_json` (the import report's `conversion` field).
/// Either way the generated Worker project is stored under
/// `extensions/<slug>/worker/` and the instance is recorded in
/// `wl_extension_instances` — a loaded theme is an extension instance
/// whose slug is the theme slug.
async fn load_extension(ctx: &mut Ctx) -> Result<RouteResponse, WordlessError> {
    let slug = slug_param(ctx);
    let secret = ctx.ext_secret.clone().ok_or_else(|| {
        internal_error(
            "The extension host secret is not configured (WORDLESS_EXT_HOST_SECRET). \
             Set it in .dev.vars for local dev or as a Worker secret, then load again.",
        )
    })?;
    let body = if ctx.body.is_empty() {
        Value::Null
    } else {
        read_json_body(ctx)?
    };
    let dev_url = body
        .get("devUrl")
        .and_then(Value::as_str)
        .map(|s| s.to_string());
    let source = load_source_from_body(&body);
    let store = require_store(ctx)?;
    let (report, version) = load_source_meta(ctx.db.as_ref(), &slug, source).await?;
    let files = load_converted_files(store.as_ref(), &slug, source).await?;
    let hosts = declared_hosts(store.as_ref(), &slug, source).await?;

    let decision = evaluate_load_gate(report.as_ref(), &files, &hosts);
    let (required, warnings) = match decision {
        LoadDecision::Allowed { required, warnings } => (required, warnings),
        LoadDecision::Refused {
            reasons,
            blocked_files,
        } => {
            return ok(
                json!({
                    "slug": slug, "loaded": false, "reasons": reasons,
                    "blockedFiles": blocked_files,
                }),
                200,
            );
        }
    };

    // Generate + store the extension Worker project. The converted
    // files already live in R2 under extensions/<slug>/converted/ (or
    // themes/<slug>/converted/ for a theme source); the project (entry
    // + shim + type declarations) goes under extensions/<slug>/worker/.
    let project = generate_extension_project(&ExtensionProjectInput {
        slug: &slug,
        version: &version,
        files: &files,
        declared_hosts: &hosts,
        locale: "en_US",
    });
    let prefix = format!("extensions/{slug}/worker/");
    store
        .put_object(
            &format!("{prefix}worker-entry.ts"),
            project.entry.into_bytes(),
            "text/typescript",
        )
        .await?;
    store
        .put_object(
            &format!("{prefix}shim.ts"),
            project.shim.into_bytes(),
            "text/typescript",
        )
        .await?;
    for (path, content) in &project.wordpress_dts {
        store
            .put_object(
                &format!("{prefix}{path}"),
                content.clone().into_bytes(),
                "text/typescript",
            )
            .await?;
    }
    store
        .put_object(
            &format!("{prefix}wrangler.toml"),
            project.wrangler_toml.into_bytes(),
            "text/plain",
        )
        .await?;
    let script_key = format!("{prefix}worker-entry.ts");

    // The reload epoch: the instance row increments it on conflict,
    // so derive the token from the epoch the row WILL have.
    let epoch = get_instance(ctx.db.as_ref(), &slug)
        .await?
        .map(|i| i.token_epoch + 1)
        .unwrap_or(0);
    let token = derive_host_token(&secret, &slug, epoch);
    let inst = record_instance(
        ctx.db.as_ref(),
        &slug,
        &version,
        &required,
        &script_key,
        &token_hash(&token),
        dev_url.as_deref(),
    )
    .await?;
    // The stored epoch must match the one the token was derived from;
    // a mismatch means host calls would fail closed — refuse loudly
    // instead of loading an extension that cannot call home.
    if inst.token_epoch != epoch {
        return Err(internal_error(
            "Token epoch mismatch after recording the instance — the extension was not loaded.",
        ));
    }
    let _ = record_log(
        ctx.db.as_ref(),
        &slug,
        "info",
        &format!(
            "Extension loaded ({} required capabilities).",
            required.len()
        ),
    )
    .await;
    ok(
        json!({
            "slug": slug, "loaded": true, "version": inst.version,
            "status": inst.status, "permissions": inst.permissions,
            "requiredCapabilities": required.iter().map(|c| c.id()).collect::<Vec<_>>(),
            "warnings": warnings, "scriptKey": script_key, "devUrl": inst.dev_url,
            "tokenEpoch": inst.token_epoch,
            "dispatchConfigured": ctx.ext_dispatcher_present || dev_url.is_some(),
        }),
        200,
    )
}

/// Where a loadable extension's converted code and report live.
/// `Extension` is a marketplace plugin (`extensions/<slug>/`,
/// report in `wl_extensions`); `Theme` is an imported theme
/// (`themes/<slug>/`, conversion report in `wl_themes.report_json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoadSource {
    Extension,
    Theme,
}

impl LoadSource {
    fn converted_prefix(self, slug: &str) -> String {
        match self {
            LoadSource::Extension => format!("extensions/{slug}/converted/"),
            LoadSource::Theme => format!("themes/{slug}/converted/"),
        }
    }
    fn manifest_key(self, slug: &str) -> String {
        match self {
            LoadSource::Extension => format!("extensions/{slug}/wordless.json"),
            LoadSource::Theme => format!("themes/{slug}/wordless.json"),
        }
    }
}

fn load_source_from_body(body: &Value) -> LoadSource {
    match body.get("source").and_then(Value::as_str) {
        Some("theme") => LoadSource::Theme,
        _ => LoadSource::Extension,
    }
}

fn load_source_from_query(ctx: &Ctx) -> LoadSource {
    match ctx.query("source").as_deref() {
        Some("theme") => LoadSource::Theme,
        _ => LoadSource::Extension,
    }
}

/// The load gate's inputs for one source: (report, version).
/// Extension: the marketplace row's `report_json` + version.
/// Theme: the import report's `conversion` field (persisted in
/// `wl_themes.report_json`) + theme version. A theme with no
/// conversion report (imported before reports were persisted, or
/// never auto-converted) cannot load — re-import it.
async fn load_source_meta(
    db: &dyn Db,
    slug: &str,
    source: LoadSource,
) -> Result<(Option<Value>, String), WordlessError> {
    match source {
        LoadSource::Extension => {
            let ext = get_extension(db, slug).await?;
            Ok((ext.report, ext.version))
        }
        LoadSource::Theme => {
            let row: Option<Row> = db
                .query_one(
                    "SELECT version, report_json FROM wl_themes WHERE slug = ? LIMIT 1",
                    &[Param::Text(slug.to_string())],
                )
                .await?;
            let Some(row) = row else {
                return Err(not_found(&format!(
                    "Theme \"{slug}\" not found. Import it first."
                )));
            };
            let version = row_text(&row, "version");
            let report_json = row_opt_text(&row, "report_json").unwrap_or_default();
            let report: Value = serde_json::from_str(&report_json).unwrap_or(Value::Null);
            let conversion = report.get("conversion").cloned();
            match conversion {
                Some(Value::Object(_)) => Ok((conversion, version)),
                _ => Err(bad_request(&format!(
                    "Theme \"{slug}\" has no conversion report — re-import it to generate one."
                ))),
            }
        }
    }
}

/// Read every converted file for an extension from R2, as
/// [`ConvertedFile`]s (PHP path + generated R2 key + code).
async fn load_converted_files(
    store: &dyn ObjectStore,
    slug: &str,
    source: LoadSource,
) -> Result<Vec<ConvertedFile>, WordlessError> {
    let prefix = source.converted_prefix(slug);
    let mut files: Vec<ConvertedFile> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = store.list_objects(&prefix, 1000, cursor.as_deref()).await?;
        for obj in &page.objects {
            if !obj.key.ends_with(".ts") {
                continue;
            }
            let Some((bytes, _)) = store.get_object(&obj.key).await? else {
                continue;
            };
            let code = String::from_utf8(bytes).map_err(|_| {
                internal_error(&format!("Converted file {} is not valid UTF-8.", obj.key))
            })?;
            let rel = obj.key.strip_prefix(&prefix).unwrap_or(&obj.key);
            let php_path = rel
                .strip_suffix(".ts")
                .map(|s| format!("{s}.php"))
                .unwrap_or_else(|| rel.to_string());
            files.push(ConvertedFile {
                path: php_path,
                generated_path: obj.key.clone(),
                code,
            });
        }
        if !page.truncated {
            break;
        }
        cursor = page.cursor;
        if cursor.is_none() {
            break;
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Network hosts declared in the extension manifest
/// (`extensions/<slug>/wordless.json` or `themes/<slug>/wordless.json`,
/// optional `networkHosts` array). Empty by default — an extension
/// that fetches the network without declaring hosts is refused by
/// the load gate.
async fn declared_hosts(
    store: &dyn ObjectStore,
    slug: &str,
    source: LoadSource,
) -> Result<Vec<String>, WordlessError> {
    let key = source.manifest_key(slug);
    let Some((bytes, _)) = store.get_object(&key).await? else {
        return Ok(vec![]);
    };
    let parsed: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let mut hosts: Vec<String> = Vec::new();
    if let Some(arr) = parsed.get("networkHosts").and_then(Value::as_array) {
        for h in arr {
            if let Some(s) = h.as_str() {
                let s = s.trim().to_ascii_lowercase();
                if !s.is_empty() && !hosts.contains(&s) {
                    hosts.push(s);
                }
            }
        }
    }
    Ok(hosts)
}

/// Option keys an extension may write: its own `<slug>_*` keys, plus
/// the keys its PHP declared via `get_option` defaults (the analyser
/// records them as `optionKeys` in the report).
fn allowed_option_keys(slug: &str, report: Option<&Value>) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    if let Some(report) = report {
        if let Some(files) = report.get("files").and_then(Value::as_array) {
            for f in files {
                if let Some(arr) = f.get("optionKeys").and_then(Value::as_array) {
                    for k in arr {
                        if let Some(s) = k.as_str() {
                            if !keys.contains(&s.to_string()) {
                                keys.push(s.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    let _ = slug;
    keys
}

fn option_key_allowed(slug: &str, report: Option<&Value>, key: &str) -> bool {
    if key.starts_with(&format!("{slug}_")) {
        return true;
    }
    allowed_option_keys(slug, report).contains(&key.to_string())
}

/// Build the resolver adapter for [`invoke_hook`]: slug → executor +
/// token, from the request's `ext_resolver` plus the instance list.
fn hook_resolver<'a>(
    ctx: &'a Ctx,
    instances: &'a [ExtensionInstance],
) -> impl Fn(&str) -> Option<(Rc<dyn ExtensionExecutor>, String)> + 'a {
    let resolver = ctx.ext_resolver.clone();
    let instances: Vec<ExtensionInstance> = instances.to_vec();
    move |slug: &str| {
        let inst = instances.iter().find(|i| i.slug == slug)?;
        resolver.as_ref().and_then(|r| r(inst))
    }
}

/// Invoke one hook on one loaded extension (single-extension route).
async fn invoke_one(
    ctx: &mut Ctx,
    slug: &str,
    hook: &str,
    kind: &str,
    args: Vec<Value>,
) -> Result<wordless_extensions::HookOutcome, WordlessError> {
    use wordless_extensions::{record_run, ExtensionHookResult, HookOutcome};
    let inst = require_instance(ctx.db.as_ref(), slug).await?;
    if inst.status != InstanceStatus::Loaded.as_str() {
        record_run(
            ctx.db.as_ref(),
            slug,
            hook,
            "skipped_disabled",
            0,
            None,
            Some("extension is disabled"),
        )
        .await?;
        return Ok(HookOutcome {
            hook: hook.to_string(),
            kind: kind.to_string(),
            value: Value::Null,
            rendered: vec![],
            per_extension: vec![ExtensionHookResult {
                slug: slug.to_string(),
                status: "skipped_disabled".to_string(),
                rendered: vec![],
                error: Some("extension is disabled".to_string()),
                duration_ms: 0,
            }],
        });
    }
    let resolver = ctx.ext_resolver.clone();
    let Some((executor, token)) = resolver.as_ref().and_then(|r| r(&inst)) else {
        let reason = if ctx.ext_secret.is_none() {
            "extension host secret is not configured (WORDLESS_EXT_HOST_SECRET)"
        } else if inst.dev_url.is_none() && !ctx.ext_dispatcher_present {
            "dispatch namespace is not configured — publish the extension first (scripts/publish-extensions.mjs)"
        } else {
            "no executor could be built for this extension"
        };
        record_run(ctx.db.as_ref(), slug, hook, "error", 0, None, Some(reason)).await?;
        return Err(internal_error(reason));
    };
    let req = wordless_extensions::InvokeRequest {
        slug: slug.to_string(),
        hook: hook.to_string(),
        kind: kind.to_string(),
        args,
        host_url: ctx.origin.clone(),
        host_token: token,
        actor_user_id: actor(ctx),
        locale: "en_US".to_string(),
        timeout_ms: 0,
    };
    let result = executor
        .invoke(&req)
        .await
        .map_err(|e| internal_error(&e))?;
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
        ctx.db.as_ref(),
        slug,
        hook,
        run_status,
        result.duration_ms as i64,
        summary.as_deref(),
        result.error.as_deref(),
    )
    .await?;
    let value = if kind == "filter" && result.ok {
        result.result.clone()
    } else {
        Value::Null
    };
    Ok(HookOutcome {
        hook: hook.to_string(),
        kind: kind.to_string(),
        value,
        rendered: result.rendered.clone(),
        per_extension: vec![ExtensionHookResult {
            slug: slug.to_string(),
            status: run_status.to_string(),
            rendered: result.rendered,
            error: result.error,
            duration_ms: result.duration_ms,
        }],
    })
}

/// Invoke one hook across every loaded extension.
async fn invoke_all(
    ctx: &mut Ctx,
    hook: &str,
    kind: &str,
    args: Vec<Value>,
) -> Result<wordless_extensions::HookOutcome, WordlessError> {
    let instances = list_instances(ctx.db.as_ref()).await?;
    let resolver = hook_resolver(ctx, &instances);
    invoke_hook(
        ctx.db.as_ref(),
        hook,
        kind,
        args,
        &ctx.origin,
        actor(ctx),
        "en_US",
        0,
        resolver,
    )
    .await
}

/// The token-authenticated host endpoint: `POST
/// /api/extensions/:slug/host` with header `x-wordless-ext-token`
/// and body `{call, args, actorUserId}`. No session auth — the
/// per-load token IS the auth. Checks, in order: the instance is
/// loaded, the token matches the stored hash, the call exists in the
/// P6 host API, the instance was granted the capability. Every denial
/// is logged (level `denied`) and returns a `capability_denied`-style
/// error the extension can see — never the data, never the token.
async fn host_endpoint(ctx: &mut Ctx) -> Result<RouteResponse, WordlessError> {
    let slug = slug_param(ctx);
    let presented = ctx.header("x-wordless-ext-token").unwrap_or("").to_string();
    let inst = match get_instance(ctx.db.as_ref(), &slug).await? {
        Some(i) => i,
        None => return Ok(host_unauthorized()),
    };
    if inst.status != InstanceStatus::Loaded.as_str() {
        return Ok(host_unauthorized());
    }
    if presented.is_empty() || !token_matches(&presented, &inst.host_token_hash) {
        let _ = record_log(
            ctx.db.as_ref(),
            &slug,
            "denied",
            "Host call with a missing or invalid token.",
        )
        .await;
        return Ok(host_unauthorized());
    }
    let body = read_json_body(ctx)?;
    let call: HostCall = serde_json::from_value(body.clone())
        .map_err(|_| bad_request("Body must be {call, args, actorUserId}."))?;
    let actor_user_id = body.get("actorUserId").and_then(Value::as_i64).unwrap_or(0);

    // The capability gate: unknown call → not implemented; local-only
    // call arriving here → not implemented (the shim runs those
    // locally; a host call for one is forged or a bug); ungranted
    // capability → denied. All three are logged.
    let Some((capability, kind)) = capability_for_wp_call(&call.call) else {
        let _ = record_log(
            ctx.db.as_ref(),
            &slug,
            "denied",
            &format!("Unknown host call: wp_{}()", call.call),
        )
        .await;
        return ok(HostResult::not_implemented(&call.call), 200);
    };
    if !matches!(kind, wordless_extensions::CallKind::Host) {
        let _ = record_log(
            ctx.db.as_ref(),
            &slug,
            "denied",
            &format!("Local-only call arrived as a host call: wp_{}()", call.call),
        )
        .await;
        return ok(HostResult::not_implemented(&call.call), 200);
    }
    if !inst.permissions.contains(&capability.id().to_string()) {
        let _ = record_log(
            ctx.db.as_ref(),
            &slug,
            "denied",
            &format!(
                "wp_{}() needs the {} capability, which was not granted",
                call.call,
                capability.id()
            ),
        )
        .await;
        return ok(HostResult::denied(capability.id(), &call.call), 200);
    }

    let result = run_host_call(ctx, &inst, &call, actor_user_id).await;
    ok(result, 200)
}

fn host_unauthorized() -> RouteResponse {
    RouteResponse::json_error(
        "unauthorized",
        401,
        "Extension host token is missing or invalid — reload the extension.",
    )
}

/// Run one capability-checked host call against the real services.
/// Draft-only writes: an extension may create drafts and edit/delete
/// drafts, never touch published content. User meta is actor-scoped:
/// only the user the hook runs for, never another user.
async fn run_host_call(
    ctx: &mut Ctx,
    inst: &ExtensionInstance,
    call: &HostCall,
    actor_user_id: i64,
) -> HostResult {
    let db = ctx.db.as_ref();
    let args = &call.args;
    let s = |i: usize| {
        args.get(i)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let n = |i: usize| args.get(i).and_then(Value::as_i64).unwrap_or(0);
    let v = |i: usize| args.get(i).cloned().unwrap_or(Value::Null);
    match call.call.as_str() {
        "log" => {
            let level = s(0);
            let message = s(1);
            let level = match level.as_str() {
                "info" | "warn" | "error" | "denied" => level,
                _ => "info".to_string(),
            };
            match record_log(db, &inst.slug, &level, &message).await {
                Ok(()) => HostResult::ok(Value::Bool(true)),
                Err(e) => HostResult::bad_call(e.to_string()),
            }
        }
        // Theme asset enqueues are recorded only: the theme engine
        // serves the actual assets, so an enqueue can never inject an
        // arbitrary URL into a page. The call always succeeds when the
        // capability is granted (as in WordPress, wp_enqueue_* never
        // fails the page); a missing grant is denied by the endpoint
        // before this arm runs, and logged there.
        "wp_enqueue_style" | "wp_enqueue_script" => {
            let kind = if call.call == "wp_enqueue_style" {
                "style"
            } else {
                "script"
            };
            let handle = s(0);
            let src = s(1);
            let message = if src.is_empty() {
                format!("enqueue {kind} {handle}")
            } else {
                format!("enqueue {kind} {handle} ({src})")
            };
            match record_log(db, &inst.slug, "info", &message).await {
                Ok(()) => HostResult::ok(Value::Bool(true)),
                Err(e) => HostResult::bad_call(e.to_string()),
            }
        }
        "get_option" => {
            let name = s(0);
            let def = v(1);
            match db_get_option::<Value>(db, &name, def).await {
                Ok(value) => HostResult::ok(value),
                Err(e) => HostResult::bad_call(e.to_string()),
            }
        }
        "update_option" | "delete_option" | "add_option" => {
            let name = s(0);
            let report = match get_extension(db, &inst.slug).await {
                Ok(e) => e.report,
                Err(_) => None,
            };
            if !option_key_allowed(&inst.slug, report.as_ref(), &name) {
                let _ = record_log(
                    db,
                    &inst.slug,
                    "denied",
                    &format!("Option write to \"{name}\" is outside this extension's keys (own <slug>_* keys and declared get_option defaults only)."),
                )
                .await;
                return HostResult::denied("options.write", &call.call);
            }
            if call.call == "delete_option" {
                return match db
                    .exec(
                        "DELETE FROM wp_options WHERE option_name = ?",
                        &[Param::Text(name)],
                    )
                    .await
                {
                    Ok(meta) => HostResult::ok(Value::Bool(meta.changes > 0)),
                    Err(e) => HostResult::bad_call(e.to_string()),
                };
            }
            if call.call == "add_option" {
                // add_option fails (false) when the key already exists —
                // check the row directly (get_option's default would be
                // ambiguous when the stored value is null).
                let row = db
                    .query_one(
                        "SELECT option_name FROM wp_options WHERE option_name = ? LIMIT 1",
                        &[Param::Text(name.clone())],
                    )
                    .await;
                return match row {
                    Ok(Some(_)) => HostResult::ok(Value::Bool(false)),
                    Ok(None) => match db_set_option(db, &name, &v(1)).await {
                        Ok(()) => HostResult::ok(Value::Bool(true)),
                        Err(e) => HostResult::bad_call(e.to_string()),
                    },
                    Err(e) => HostResult::bad_call(e.to_string()),
                };
            }
            match db_set_option(db, &name, &v(1)).await {
                Ok(()) => HostResult::ok(Value::Null),
                Err(e) => HostResult::bad_call(e.to_string()),
            }
        }
        "get_post" => match get_post_by_id(db, ctx.site_id, n(0)).await {
            Ok(Some(post)) => HostResult::ok(json_value(&post)),
            Ok(None) => HostResult::ok(Value::Null),
            Err(e) => HostResult::bad_call(e.to_string()),
        },
        "get_posts" => {
            let a = v(0);
            let opts = ListPostsOptions {
                status: a
                    .get("status")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string()),
                post_type: a.get("type").and_then(Value::as_str).map(|s| s.to_string()),
                author_id: a.get("authorId").and_then(Value::as_i64),
                page: a.get("page").and_then(Value::as_i64),
                per_page: a.get("perPage").and_then(Value::as_i64),
                include_trash: false,
            };
            match list_posts(db, ctx.site_id, opts).await {
                Ok(result) => HostResult::ok(json_value(&result)),
                Err(e) => HostResult::bad_call(e.to_string()),
            }
        }
        "wp_insert_post" => {
            let input = v(0);
            let post_input = CreatePostInput {
                title: input
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                content: input
                    .get("content")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string()),
                excerpt: None,
                slug: None,
                // Draft-only: the status the extension asked for is
                // ignored — an extension never publishes directly.
                status: Some("draft".to_string()),
                post_type: input
                    .get("type")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string()),
                author_id: actor_user_id,
                parent_id: None,
                mime_type: None,
                featured_image_id: None,
            };
            match create_post(db, ctx.site_id, post_input).await {
                Ok(post) => HostResult::ok(json_value(&post)),
                Err(e) => HostResult::bad_call(e.to_string()),
            }
        }
        "wp_update_post" => {
            let id = n(0);
            let patch = v(1);
            match get_post_by_id(db, ctx.site_id, id).await {
                Ok(Some(post)) => {
                    if post.status != PostStatus::Draft {
                        return HostResult::bad_call(
                            "Extensions may only edit draft posts — this post is not a draft.",
                        );
                    }
                    let input = UpdatePostInput {
                        title: patch
                            .get("title")
                            .and_then(Value::as_str)
                            .map(|s| s.to_string()),
                        content: patch
                            .get("content")
                            .and_then(Value::as_str)
                            .map(|s| s.to_string()),
                        excerpt: None,
                        slug: None,
                        // Stays a draft whatever the patch asked for.
                        status: Some("draft".to_string()),
                        post_type: None,
                        parent_id: None,
                        featured_image_id: None,
                    };
                    match update_post(db, ctx.site_id, id, input).await {
                        Ok(post) => HostResult::ok(json_value(&post)),
                        Err(e) => HostResult::bad_call(e.to_string()),
                    }
                }
                Ok(None) => HostResult::bad_call(format!("Post {id} not found.")),
                Err(e) => HostResult::bad_call(e.to_string()),
            }
        }
        "wp_delete_post" => {
            let id = n(0);
            match get_post_by_id(db, ctx.site_id, id).await {
                Ok(Some(post)) => {
                    if post.status != PostStatus::Draft {
                        return HostResult::bad_call(
                            "Extensions may only delete draft posts — this post is not a draft.",
                        );
                    }
                    match delete_post(db, ctx.site_id, id).await {
                        Ok(deleted) => HostResult::ok(Value::Bool(deleted)),
                        Err(e) => HostResult::bad_call(e.to_string()),
                    }
                }
                Ok(None) => HostResult::ok(Value::Bool(false)),
                Err(e) => HostResult::bad_call(e.to_string()),
            }
        }
        "get_user_meta" | "update_user_meta" => {
            let user_id = n(0);
            let key = s(1);
            if actor_user_id == 0 || user_id != actor_user_id {
                let _ = record_log(
                    db,
                    &inst.slug,
                    "denied",
                    "User meta is actor-scoped: an extension may only touch the user the hook runs for.",
                )
                .await;
                return HostResult::denied("users.write", &call.call);
            }
            if call.call == "get_user_meta" {
                match wordless_auth::users::get_user_meta::<Value>(db, user_id, &key, v(2)).await {
                    Ok(value) => HostResult::ok(value),
                    Err(e) => HostResult::bad_call(e.to_string()),
                }
            } else {
                match wordless_auth::users::update_user_meta(db, user_id, &key, &v(2)).await {
                    Ok(()) => HostResult::ok(Value::Null),
                    Err(e) => HostResult::bad_call(e.to_string()),
                }
            }
        }
        "wp_get_post_terms" => {
            let taxonomy = s(1);
            let taxonomy = if taxonomy.is_empty() {
                None
            } else {
                Some(taxonomy.as_str())
            };
            match get_post_terms(db, ctx.site_id, n(0), taxonomy).await {
                Ok(terms) => HostResult::ok(json_value(&terms)),
                Err(e) => HostResult::bad_call(e.to_string()),
            }
        }
        "get_the_terms" => match get_post_terms(db, ctx.site_id, n(0), Some(s(1).as_str())).await {
            Ok(terms) => HostResult::ok(json_value(&terms)),
            Err(e) => HostResult::bad_call(e.to_string()),
        },
        "wp_list_categories" => {
            let a = v(0);
            let taxonomy = a
                .get("taxonomy")
                .and_then(Value::as_str)
                .unwrap_or("category")
                .to_string();
            match list_terms(
                db,
                ctx.site_id,
                &taxonomy,
                ListTermsOptions {
                    parent_id: None,
                    page: Some(1),
                    per_page: Some(100),
                },
            )
            .await
            {
                Ok(result) => {
                    let items: Vec<String> = result
                        .terms
                        .iter()
                        .map(|t| format!("<li>{}</li>", esc_html(&t.name)))
                        .collect();
                    HostResult::ok(Value::String(format!("<ul>{}</ul>", items.join(""))))
                }
                Err(e) => HostResult::bad_call(e.to_string()),
            }
        }
        "wp_get_post_revisions" => match list_revisions(db, ctx.site_id, n(0)).await {
            Ok(revisions) => HostResult::ok(json_value(&revisions)),
            Err(e) => HostResult::bad_call(e.to_string()),
        },
        "wp_restore_post_revision" => {
            let revision_id = n(0);
            match get_revision(db, ctx.site_id, revision_id).await {
                Ok(revision) => match get_post_by_id(db, ctx.site_id, revision.post_id).await {
                    Ok(Some(post)) => {
                        if post.status != PostStatus::Draft {
                            return HostResult::bad_call(
                                "Extensions may only restore revisions of draft posts.",
                            );
                        }
                        match restore_revision(db, ctx.site_id, revision_id).await {
                            Ok(post) => HostResult::ok(json_value(&post)),
                            Err(e) => HostResult::bad_call(e.to_string()),
                        }
                    }
                    Ok(None) => {
                        HostResult::bad_call(format!("Post {} not found.", revision.post_id))
                    }
                    Err(e) => HostResult::bad_call(e.to_string()),
                },
                Err(e) => HostResult::bad_call(e.to_string()),
            }
        }
        "wp_nav_menu" => {
            let a = v(0);
            let menu_id = a.get("menu").and_then(Value::as_i64).unwrap_or(0);
            if menu_id < 1 {
                return HostResult::bad_call("wp_nav_menu needs a menu id.");
            }
            match get_menu(db, ctx.site_id, menu_id).await {
                Ok(_) => match list_menu_items(db, ctx.site_id, menu_id).await {
                    Ok(items) => {
                        let lis: Vec<String> = items
                            .iter()
                            .map(|i| {
                                format!(
                                    "<li><a href=\"{}\">{}</a></li>",
                                    esc_html(&i.url),
                                    esc_html(&i.title)
                                )
                            })
                            .collect();
                        HostResult::ok(Value::String(format!("<ul>{}</ul>", lis.join(""))))
                    }
                    Err(e) => HostResult::bad_call(e.to_string()),
                },
                Err(e) => HostResult::bad_call(e.to_string()),
            }
        }
        "get_media_items" => {
            let a = v(0);
            match wordless_media::service::list_media(
                db,
                ctx.site_id,
                wordless_media::service::ListMediaOpts {
                    page: a.get("page").and_then(Value::as_i64),
                    per_page: a.get("perPage").and_then(Value::as_i64),
                    mime_type: a
                        .get("mimeType")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string()),
                },
            )
            .await
            {
                Ok(result) => {
                    HostResult::ok(json!({ "items": result.media, "total": result.total }))
                }
                Err(e) => HostResult::bad_call(e.to_string()),
            }
        }
        "wp_get_attachment_url" => match get_media_record(db, ctx.site_id, n(0)).await {
            Ok(Some(record)) => HostResult::ok(Value::String(record.url)),
            Ok(None) => HostResult::ok(Value::String(String::new())),
            Err(e) => HostResult::bad_call(e.to_string()),
        },
        _ => HostResult::not_implemented(&call.call),
    }
}

fn json_value<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

fn esc_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#039;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use wordless_db::{DbError, RunMeta};
    use wordless_kernel::storage::{ListPage, ObjectMeta, ObjectStore};

    /// In-memory SQLite `Db` for the source-meta tests — the same
    /// pattern as `wordless-extensions/src/test_support.rs` (that
    /// module is `#[cfg(test)]` inside its own crate, so it is not
    /// importable here).
    struct SqliteDb {
        conn: Mutex<rusqlite::Connection>,
    }

    fn param_to_rusqlite(p: &Param) -> rusqlite::types::Value {
        match p {
            Param::Null => rusqlite::types::Value::Null,
            Param::Integer(n) => rusqlite::types::Value::Integer(*n),
            Param::Real(f) => rusqlite::types::Value::Real(*f),
            Param::Text(s) => rusqlite::types::Value::Text(s.clone()),
            Param::Blob(b) => rusqlite::types::Value::Blob(b.clone()),
        }
    }

    fn rusqlite_to_json(v: rusqlite::types::Value) -> Value {
        match v {
            rusqlite::types::Value::Null => Value::Null,
            rusqlite::types::Value::Integer(n) => Value::from(n),
            rusqlite::types::Value::Real(f) => serde_json::Number::from_f64(f)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            rusqlite::types::Value::Text(s) => Value::String(s),
            rusqlite::types::Value::Blob(_) => Value::Null,
        }
    }

    fn test_db() -> SqliteDb {
        SqliteDb {
            conn: Mutex::new(rusqlite::Connection::open_in_memory().expect("in-memory sqlite")),
        }
    }

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        futures_executor::block_on(f)
    }

    #[async_trait::async_trait(?Send)]
    impl Db for SqliteDb {
        fn prepare(&self, sql: &str) -> wordless_db::Statement<'_> {
            wordless_db::Statement::new(self as &dyn Db, sql.to_string())
        }
        async fn query_all(&self, sql: &str, params: &[Param]) -> Result<Vec<Row>, DbError> {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare(sql).map_err(|e| DbError::D1(e.to_string()))?;
            let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
            let rows = stmt
                .query_map(
                    rusqlite::params_from_iter(params.iter().map(param_to_rusqlite)),
                    |row| {
                        let mut obj = serde_json::Map::new();
                        for (idx, col) in cols.iter().enumerate() {
                            let v: rusqlite::types::Value = row.get(idx)?;
                            obj.insert(col.clone(), rusqlite_to_json(v));
                        }
                        Ok(Value::Object(obj))
                    },
                )
                .map_err(|e| DbError::D1(e.to_string()))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(|e| DbError::D1(e.to_string()))?);
            }
            Ok(out)
        }
        async fn query_one(&self, sql: &str, params: &[Param]) -> Result<Option<Row>, DbError> {
            let rows = self.query_all(sql, params).await?;
            Ok(rows.into_iter().next())
        }
        async fn exec(&self, sql: &str, params: &[Param]) -> Result<RunMeta, DbError> {
            let conn = self.conn.lock().unwrap();
            if params.is_empty() {
                conn.execute_batch(sql)
                    .map_err(|e| DbError::D1(e.to_string()))?;
                return Ok(RunMeta {
                    last_row_id: Some(conn.last_insert_rowid()),
                    changes: 0,
                });
            }
            let changes = conn
                .execute(
                    sql,
                    rusqlite::params_from_iter(params.iter().map(param_to_rusqlite)),
                )
                .map_err(|e| DbError::D1(e.to_string()))?;
            Ok(RunMeta {
                last_row_id: Some(conn.last_insert_rowid()),
                changes: changes as u64,
            })
        }
        async fn batch(&self, stmts: &[(String, Vec<Param>)]) -> Result<(), DbError> {
            for (sql, params) in stmts {
                self.exec(sql, params).await?;
            }
            Ok(())
        }
    }

    /// Minimal in-memory `ObjectStore` for the source-prefix tests.
    struct MemStore {
        objects: RefCell<HashMap<String, Vec<u8>>>,
    }

    impl MemStore {
        fn new() -> Self {
            Self {
                objects: RefCell::new(HashMap::new()),
            }
        }
        fn put(&self, key: &str, bytes: &[u8]) {
            self.objects
                .borrow_mut()
                .insert(key.to_string(), bytes.to_vec());
        }
    }

    #[async_trait::async_trait(?Send)]
    impl ObjectStore for MemStore {
        async fn count_objects(&self, limit: usize) -> Result<(usize, bool), WordlessError> {
            let total = self.objects.borrow().len();
            Ok((total.min(limit), total > limit))
        }
        async fn get_object(
            &self,
            key: &str,
        ) -> Result<Option<(Vec<u8>, Option<String>)>, WordlessError> {
            Ok(self
                .objects
                .borrow()
                .get(key)
                .map(|b| (b.clone(), Some("text/plain".to_string()))))
        }
        async fn put_object(
            &self,
            key: &str,
            bytes: Vec<u8>,
            content_type: &str,
        ) -> Result<(), WordlessError> {
            let _ = content_type;
            self.objects.borrow_mut().insert(key.to_string(), bytes);
            Ok(())
        }
        async fn delete_object(&self, key: &str) -> Result<(), WordlessError> {
            self.objects.borrow_mut().remove(key);
            Ok(())
        }
        async fn head_object(&self, key: &str) -> Result<Option<ObjectMeta>, WordlessError> {
            Ok(self.objects.borrow().get(key).map(|b| ObjectMeta {
                key: key.to_string(),
                size: b.len() as u64,
                content_type: Some("text/plain".to_string()),
                uploaded_at: None,
            }))
        }
        async fn list_objects(
            &self,
            prefix: &str,
            limit: usize,
            cursor: Option<&str>,
        ) -> Result<ListPage, WordlessError> {
            let mut keys: Vec<String> = self
                .objects
                .borrow()
                .keys()
                .filter(|k| k.starts_with(prefix))
                .cloned()
                .collect();
            keys.sort();
            if let Some(c) = cursor {
                keys.retain(|k| k.as_str() > c);
            }
            let truncated = keys.len() > limit;
            let page_keys: Vec<String> = keys.into_iter().take(limit).collect();
            let next = if truncated {
                page_keys.last().cloned()
            } else {
                None
            };
            Ok(ListPage {
                objects: page_keys
                    .iter()
                    .map(|k| ObjectMeta {
                        key: k.clone(),
                        size: self
                            .objects
                            .borrow()
                            .get(k)
                            .map(|b| b.len() as u64)
                            .unwrap_or(0),
                        content_type: Some("text/plain".to_string()),
                        uploaded_at: None,
                    })
                    .collect(),
                truncated,
                cursor: next,
            })
        }
    }

    fn analyser_report() -> Value {
        json!({
            "verdict": "converted", "phpFiles": 1,
            "files": [{ "file": "hello.php", "verdict": "converted", "reasons": [] }],
        })
    }

    fn theme_row_with_conversion() -> Value {
        json!({
            "kind": "block-theme-import",
            "conversion": {
                "phpFiles": 1, "converted": 0, "partial": 1, "blocked": 0,
                "files": [{ "file": "functions.php", "status": "partial",
                            "reason": "1 item could not be converted." }],
            },
        })
    }

    #[test]
    fn source_from_body_defaults_to_extension() {
        assert_eq!(load_source_from_body(&json!({})), LoadSource::Extension);
        assert_eq!(
            load_source_from_body(&json!({"source": "extension"})),
            LoadSource::Extension
        );
        assert_eq!(
            load_source_from_body(&json!({"source": "theme"})),
            LoadSource::Theme
        );
        assert_eq!(
            load_source_from_body(&json!({"source": "bogus"})),
            LoadSource::Extension
        );
    }

    #[test]
    fn source_meta_extension_reads_marketplace_report() {
        block_on(async {
            let db = test_db();
            db.exec(
                "CREATE TABLE wl_extensions (slug TEXT PRIMARY KEY, name TEXT, kind TEXT, version TEXT, status TEXT, source TEXT, report_json TEXT, created_at TEXT, updated_at TEXT)",
                &[],
            ).await.unwrap();
            db.exec(
                "INSERT INTO wl_extensions (slug, name, kind, version, status, source, report_json, created_at, updated_at) VALUES ('hello-dolly', 'Hello Dolly', 'plugin', '1.7.2', 'installed', NULL, ?, '2026-11-19', '2026-11-19')",
                &[Param::Text(serde_json::to_string(&analyser_report()).unwrap())],
            ).await.unwrap();
            let (report, version) = load_source_meta(&db, "hello-dolly", LoadSource::Extension)
                .await
                .unwrap();
            assert_eq!(version, "1.7.2");
            assert_eq!(report.as_ref().unwrap()["verdict"], "converted");
            assert!(load_source_meta(&db, "missing", LoadSource::Extension)
                .await
                .is_err());
        });
    }

    #[test]
    fn source_meta_theme_reads_conversion_from_theme_report() {
        block_on(async {
            let db = test_db();
            db.exec(
                "CREATE TABLE wl_themes (slug TEXT PRIMARY KEY, version TEXT, report_json TEXT)",
                &[],
            )
            .await
            .unwrap();
            db.exec(
                "INSERT INTO wl_themes (slug, version, report_json) VALUES ('twentytwentyfive', '1.0', ?)",
                &[Param::Text(serde_json::to_string(&theme_row_with_conversion()).unwrap())],
            ).await.unwrap();
            let (report, version) = load_source_meta(&db, "twentytwentyfive", LoadSource::Theme)
                .await
                .unwrap();
            assert_eq!(version, "1.0");
            let report = report.unwrap();
            assert_eq!(report["phpFiles"], 1);
            assert_eq!(report["files"][0]["status"], "partial");
            // Theme with no conversion report cannot load.
            db.exec(
                "INSERT INTO wl_themes (slug, version, report_json) VALUES ('no-report', '1.0', ?)",
                &[Param::Text(
                    "{\"kind\":\"block-theme-import\",\"conversion\":null}".to_string(),
                )],
            )
            .await
            .unwrap();
            assert!(load_source_meta(&db, "no-report", LoadSource::Theme)
                .await
                .is_err());
            // Unknown theme: not found.
            assert!(load_source_meta(&db, "missing", LoadSource::Theme)
                .await
                .is_err());
        });
    }

    #[test]
    fn converted_files_read_theme_prefix() {
        block_on(async {
            let store = MemStore::new();
            store.put(
                "themes/twentytwentyfive/converted/functions.ts",
                b"export function activate(wp) {}",
            );
            store.put(
                "themes/twentytwentyfive/converted/inc/foo.ts",
                b"export const x = 1;",
            );
            store.put(
                "extensions/hello-dolly/converted/hello.ts",
                b"export function activate(wp) {}",
            );
            let theme_files = load_converted_files(&store, "twentytwentyfive", LoadSource::Theme)
                .await
                .unwrap();
            assert_eq!(theme_files.len(), 2);
            assert_eq!(theme_files[0].path, "functions.php");
            assert_eq!(theme_files[1].path, "inc/foo.php");
            let ext_files = load_converted_files(&store, "hello-dolly", LoadSource::Extension)
                .await
                .unwrap();
            assert_eq!(ext_files.len(), 1);
            assert_eq!(ext_files[0].path, "hello.php");
        });
    }

    #[test]
    fn declared_hosts_read_theme_manifest() {
        block_on(async {
            let store = MemStore::new();
            store.put(
                "themes/twentytwentyfive/wordless.json",
                br#"{"networkHosts": ["Example.COM"]}"#,
            );
            let hosts = declared_hosts(&store, "twentytwentyfive", LoadSource::Theme)
                .await
                .unwrap();
            assert_eq!(hosts, vec!["example.com".to_string()]);
            let none = declared_hosts(&store, "hello-dolly", LoadSource::Extension)
                .await
                .unwrap();
            assert!(none.is_empty());
        });
    }

    struct StubHttpPost;

    #[async_trait::async_trait(?Send)]
    impl wordless_webhooks::HttpPost for StubHttpPost {
        async fn post(
            &self,
            _url: &str,
            _headers: &[(String, String)],
            _body: &[u8],
        ) -> Result<u16, String> {
            Ok(200)
        }
    }

    /// A `Ctx` wired to the given in-memory `Db` (same shape as the
    /// `test_ctx` in routes.rs, minus the stub D1).
    fn test_ctx_with_db(db: Rc<dyn Db>) -> Ctx {
        Ctx {
            method: "POST".to_string(),
            site_id: 1,
            site_slug: String::new(),
            site_archived: false,
            path: "/api/extensions/x/host".to_string(),
            query: HashMap::new(),
            headers: HashMap::new(),
            body: Vec::new(),
            params: HashMap::new(),
            user: None,
            db,
            media: None,
            bindings: crate::context::Bindings {
                db: true,
                media: true,
                config: true,
            },
            origin: "http://localhost:8787".to_string(),
            purge_urls: Vec::new(),
            ai_env: wordless_ai::AiEnv::empty(),
            payment_secrets: wordless_commerce::payments::PaymentSecrets::default(),
            int_secrets: wordless_integrations::EnvSecrets::default(),
            int_bindings: wordless_integrations::Bindings {
                d1: true,
                r2: true,
                kv: true,
                assets: true,
                ai: false,
            },
            http_post: Rc::new(StubHttpPost),
            ext_secret: None,
            ext_dispatcher_present: false,
            ext_resolver: None,
            converter: None,
            query_string: String::new(),
            social: None,
            woo: None,
            school: None,
            seo: None,
            woo_client: None,
        }
    }

    #[test]
    fn enqueue_host_call_records_the_enqueue_in_the_log() {
        // The TT25 proof end to end: wp_enqueue_style is a Host call
        // that the main Worker records in wl_extension_logs (the theme
        // engine serves the actual assets, so nothing from here ever
        // reaches a page). The TT25 smoke invoke cannot prove this —
        // its callback throws at the unmapped get_stylesheet_uri()
        // argument before the enqueue call — so the unit test is the
        // proof for the host arm itself.
        block_on(async {
            let db = test_db();
            db.exec(
                "CREATE TABLE wl_extension_logs (id INTEGER PRIMARY KEY AUTOINCREMENT, slug TEXT NOT NULL, level TEXT NOT NULL DEFAULT 'info', message TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL);",
                &[],
            )
            .await
            .unwrap();
            let mut ctx = test_ctx_with_db(Rc::new(db));
            let inst = ExtensionInstance {
                slug: "twentytwentyfive".to_string(),
                version: "1.0.0".to_string(),
                status: "loaded".to_string(),
                permissions: vec!["theme.enqueue".to_string()],
                script_key: "extensions/twentytwentyfive/worker/worker-entry.ts".to_string(),
                host_token_hash: "deadbeef".to_string(),
                token_epoch: 0,
                dev_url: None,
                created_at: "2026-11-19T00:00:00Z".to_string(),
                updated_at: "2026-11-19T00:00:00Z".to_string(),
            };
            let call = HostCall {
                call: "wp_enqueue_style".to_string(),
                args: vec![
                    Value::String("twentytwentyfive-style".to_string()),
                    Value::String("https://example.test/style.css".to_string()),
                    Value::Array(vec![]),
                    Value::String("1.0".to_string()),
                    Value::String("all".to_string()),
                ],
            };
            let result = run_host_call(&mut ctx, &inst, &call, 0).await;
            assert!(result.ok, "{result:?}");
            assert_eq!(result.data, Some(Value::Bool(true)));
            let logs = list_logs(ctx.db.as_ref(), "twentytwentyfive", 10)
                .await
                .unwrap();
            assert_eq!(logs.len(), 1);
            assert_eq!(logs[0].level, "info");
            assert!(
                logs[0]
                    .message
                    .contains("enqueue style twentytwentyfive-style"),
                "{}",
                logs[0].message
            );
            // And the script twin records the same way.
            let call = HostCall {
                call: "wp_enqueue_script".to_string(),
                args: vec![
                    Value::String("tt25-js".to_string()),
                    Value::String("".to_string()),
                ],
            };
            let result = run_host_call(&mut ctx, &inst, &call, 0).await;
            assert!(result.ok, "{result:?}");
            let logs = list_logs(ctx.db.as_ref(), "twentytwentyfive", 10)
                .await
                .unwrap();
            assert_eq!(logs.len(), 2);
            // list_logs is newest-first: the script entry is logs[0].
            assert!(
                logs[0].message.contains("enqueue script tt25-js"),
                "{}",
                logs[0].message
            );
            assert!(
                logs[1]
                    .message
                    .contains("enqueue style twentytwentyfive-style"),
                "{}",
                logs[1].message
            );
        });
    }
}
