//! P3 route table — conversion, AI, commerce, integrations,
//! marketplace, security, backup, importer, GraphQL, outbound
//! webhooks, and the public shop (v1 routes 105–173 + the two new
//! v1.3 groups). Registered from `routes::build_router` after the
//! P2 table; the helpers it needs from `routes.rs` are `pub(crate)`
//! there. Handlers mirror the v1 `src/index.ts` shapes (same paths,
//! methods, capabilities, status codes) unless a comment says
//! otherwise.
//!
//! Webhook dispatch (`dispatch_webhook`) is best-effort everywhere:
//! a delivery failure must never fail the post/comment/order call
//! that triggered it. Order-confirmation mail is best-effort the
//! same way. Both are wired here so the P2 handlers in `routes.rs`
//! can call them too.

use std::collections::BTreeMap;
use std::rc::Rc;

use serde_json::{json, Value};
use wordless_ai::{
    add_memory_note, approve_approval, create_task, delete_memory_note, get_ai_mode,
    get_selected_provider_id, get_task, list_agents, list_approvals, list_memory, list_providers,
    list_tasks, rebuild_memory, reject_approval, resolve_provider, run_task, select_provider,
    set_ai_mode, test_provider, CreateAiTaskInput,
};
use wordless_backup::{
    get_backup_download, list_backups, restore_backup, run_backup_job, verify_backup_report,
};
use wordless_cms::settings::get_settings;
use wordless_commerce::cart::{
    add_to_cart, add_to_cart_variation, cart_cookie_header, clear_cart, get_cart, new_cart_token,
    parse_cart_cookie, set_cart_quantity, set_cart_quantity_variation,
};
use wordless_commerce::orders::{get_order, get_order_by_reference, list_orders, transition_order};
use wordless_commerce::payments::{
    get_payment_mode, get_selected_provider_id as get_selected_payment_provider_id,
    hmac_hex_sha256, list_payment_providers, resolve_payment_provider, set_payment_mode,
    set_selected_provider_id, start_checkout, CheckoutGateway, MockCheckoutGateway,
    PAYMENT_PROVIDER_MOCK,
};
use wordless_commerce::products::{
    create_product, delete_product, get_product, get_product_by_slug, list_products, update_product,
};
use wordless_commerce::shop::get_shop_product;
use wordless_commerce::types::ProductInput;
use wordless_commerce::webhooks::{handle_payment_webhook, list_webhook_log, WebhookOrderOutcome};
use wordless_conversion_jobs::{
    create_conversion_job, get_conversion_job, list_conversion_jobs, migrate_option_defaults,
    run_conversion_job, ConversionJobStatus, CreateConversionJobInput, FileAnalysisJson,
    CONVERSION_FIXTURES,
};
use wordless_db::{Db, Param, Row};
use wordless_graphql::execute as graphql_execute;
use wordless_importer::{import_sql, import_wxr_with_media};
use wordless_integrations::analytics::get_analytics_status;
use wordless_integrations::cloudflare::cloudflare_binding_status;
use wordless_integrations::config::{get_integration_config, set_integration_config};
use wordless_integrations::email::{list_mail_log, send_mail, SendMailInput};
use wordless_integrations::extensions::{
    get_plugin_settings, install_extension, list_extensions, list_plugin_summaries,
    put_plugin_settings, ExtensionStatus, InstallExtensionInput,
};
use wordless_integrations::marketplace::{
    get_marketplace_extension_report, install_extension_from_zip, list_marketplace_extensions,
    set_marketplace_extension_status,
};
use wordless_integrations::registry::list_integrations;
use wordless_integrations::WorkerHttpClient;
use wordless_kernel::envelope::RouteResponse;
use wordless_kernel::error::{bad_request, internal_error, not_found, WordlessError};
use wordless_kernel::router::{read_json_body, RequestContext, RouteOptions, Router};
use wordless_kernel::time::{now_iso, now_ms};
use wordless_kernel::woo_client::WooClient;
use wordless_media::multipart::parse_multipart;
use wordless_media::service::get_media_record;
use wordless_render::blocks::plain_text_excerpt;
use wordless_render::feeds::site_url;
use wordless_render::shop::{
    render_cart_page, render_checkout_result, render_product_page, render_shop,
    render_test_checkout,
};
use wordless_security::security_overview;
use wordless_seo_core::head::{Breadcrumb, PageKind, ProductSeo};
use wordless_webhooks::{
    create_subscription, delete_subscription, dispatch_event, list_deliveries, list_subscriptions,
    retry_due, retry_one,
};

use crate::context::Ctx;
use crate::routes::{
    actor, body_str, html_page, log_action, ok, ok_json, opt_str, page_param, parse_id,
    per_page_param, render_not_found_page, render_opts, require_store, see_other_response,
    with_seo, VERSION,
};
use wordless_cms::posts::Post as CmsPost;
use wordless_gateways::{fetch_text, gateway_for, url_encode, WorkerAttachmentFetcher};

/// The v1 fixture source (pinned WordPress 6.9.1 upstream URL, same
/// as v1 `FIXTURE_SOURCE_URLS`). Fetched by the worker when a
/// conversion job asks for the `hello-dolly` fixture.
const HELLO_DOLLY_URL: &str =
    "https://raw.githubusercontent.com/WordPress/WordPress/6.9.1/wp-content/plugins/hello.php";

/// Register every P3 route on the router built by `routes.rs`.
pub fn register_p3_routes(router: &mut Router<Ctx>) {
    // ------------------------------------------------------- conversion
    router.post(
        "/api/conversion/jobs",
        |ctx| {
            Box::pin(async move {
                let content_type = ctx.header("content-type").unwrap_or("").to_string();
                let (filename, source, fixture) = if content_type.contains("multipart/form-data") {
                    let form = parse_multipart(ctx.body(), &content_type)?;
                    let Some(file) = form.file("file") else {
                        return Err(bad_request(
                            "Conversion requires a multipart \"file\" field.",
                        ));
                    };
                    let filename = file
                        .filename
                        .clone()
                        .unwrap_or_else(|| "upload.php".to_string());
                    let source = String::from_utf8_lossy(&file.bytes).into_owned();
                    (filename, Some(source), None)
                } else {
                    let body = read_json_body(ctx)?;
                    (
                        body_str(&body, "filename"),
                        opt_str(&body, "source"),
                        opt_str(&body, "fixture"),
                    )
                };
                // Fixture jobs fetch their source from the pinned
                // upstream URL (v1 behaviour); upstream failure is a
                // 502, never a half-created job.
                let fixture_source = if let Some(fixture) = &fixture {
                    let known = CONVERSION_FIXTURES
                        .iter()
                        .find(|(name, _, _)| name == fixture)
                        .ok_or_else(|| {
                            bad_request(&format!("Unknown conversion fixture \"{fixture}\"."))
                        })?;
                    let source = fetch_text(HELLO_DOLLY_URL).await.map_err(|e| {
                        WordlessError::new(
                            "upstream_error",
                            502,
                            &format!("Failed to fetch fixture \"{fixture}\": {e}"),
                        )
                    })?;
                    // The job runner's contract (see
                    // `run_conversion_job`): `fixture_source` is
                    // `(source, filename)` — the fetched PHP text
                    // first, the fixture filename second.
                    Some((source, known.1))
                } else {
                    None
                };
                let job = create_conversion_job(
                    ctx.db.as_ref(),
                    CreateConversionJobInput {
                        filename,
                        source,
                        fixture,
                    },
                )
                .await?;
                // Conversion runs synchronously in the request (same
                // as v1 local dev; a Queue consumer would call the
                // same runner in production).
                let fixture_ref = fixture_source
                    .as_ref()
                    .map(|(source, filename)| (source.as_str(), *filename));
                // The conversion itself runs in the wordless-converter
                // Worker (split, 2026-11-20); without it the job fails
                // with a clear error instead of pretending it ran.
                let finished = run_conversion_job(
                    ctx.db.as_ref(),
                    &job.id,
                    fixture_ref,
                    ctx.converter.as_deref(),
                )
                .await?;
                // A successful bundled-extension conversion (only
                // hello-dolly in M4) installs the converted
                // extension, exactly like v1's bundled install —
                // recorded, never executed.
                if finished.status == ConversionJobStatus::Done {
                    if let Some(fixture) = &finished.fixture {
                        if fixture == "hello-dolly" {
                            let _ = install_extension(
                                ctx.db.as_ref(),
                                &InstallExtensionInput {
                                    slug: "hello-dolly".to_string(),
                                    name: "Hello Dolly".to_string(),
                                    version: None,
                                    kind: Some("plugin".to_string()),
                                    source: Some("bundled".to_string()),
                                    status: Some(ExtensionStatus::Active),
                                    report: None,
                                },
                            )
                            .await;
                        }
                    }
                }
                log_action(ctx, "conversion.job", "job", 0).await;
                ok(finished, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("install_plugins"),
        },
    );

    router.get(
        "/api/conversion/jobs",
        |ctx| {
            Box::pin(async move {
                let page = page_param(ctx)? as usize;
                let per_page = per_page_param(ctx)? as usize;
                ok(
                    list_conversion_jobs(ctx.db.as_ref(), page, per_page).await?,
                    200,
                )
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("install_plugins"),
        },
    );

    router.get(
        "/api/conversion/jobs/:id",
        |ctx| {
            Box::pin(async move {
                let id = ctx.param("id").unwrap_or("").to_string();
                ok(get_conversion_job(ctx.db.as_ref(), &id).await?, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("install_plugins"),
        },
    );

    // ------------------------------------------------------- extensions
    router.get(
        "/api/extensions",
        |ctx| Box::pin(async move { ok(list_extensions(ctx.db.as_ref()).await?, 200) }),
        RouteOptions {
            require_auth: false,
            capability: Some("install_plugins"),
        },
    );

    // ---------------------------------------------------------- plugins (P4)
    // §0.1 A: the dedicated Plugins page list. A thin read-only view
    // over `wl_extensions` + the stored compat reports (installed
    // list, active status, verdict summary) — the full per-file
    // report stays on `GET /api/marketplace/:slug/report`. The
    // activate/deactivate routes are thin aliases of the marketplace
    // ones, so the page can live entirely under `/api/plugins`.
    // Per-plugin settings live in `wp_options` under
    // `wl_plugin_settings_<slug>` (see `extensions::get_plugin_settings`):
    // GET merges stored overrides over the report's carried-over
    // `optionDefaults`; PUT merges a patch into the stored object
    // (`null` deletes an override).
    router.get(
        "/api/plugins",
        |ctx| Box::pin(async move { ok(list_plugin_summaries(ctx.db.as_ref()).await?, 200) }),
        RouteOptions {
            require_auth: false,
            capability: Some("install_plugins"),
        },
    );

    router.get(
        "/api/plugins/:slug/settings",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                ok(get_plugin_settings(ctx.db.as_ref(), &slug).await?, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("install_plugins"),
        },
    );

    router.put(
        "/api/plugins/:slug/settings",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                let body = read_json_body(ctx)?;
                let settings = put_plugin_settings(ctx.db.as_ref(), &slug, &body).await?;
                log_action(ctx, "plugin.settings", "extension", 0).await;
                ok(settings, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("install_plugins"),
        },
    );

    router.post(
        "/api/plugins/:slug/activate",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                let record = set_marketplace_extension_status(
                    ctx.db.as_ref(),
                    &slug,
                    ExtensionStatus::Active,
                )
                .await?;
                log_action(ctx, "marketplace.activate", "extension", 0).await;
                ok(record, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("install_plugins"),
        },
    );

    router.post(
        "/api/plugins/:slug/deactivate",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                let record = set_marketplace_extension_status(
                    ctx.db.as_ref(),
                    &slug,
                    ExtensionStatus::Inactive,
                )
                .await?;
                log_action(ctx, "marketplace.deactivate", "extension", 0).await;
                ok(record, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("install_plugins"),
        },
    );

    // ------------------------------------------------------------------ ai
    router.get(
        "/api/ai/providers",
        |ctx| {
            Box::pin(async move {
                let selected = get_selected_provider_id(ctx.db.as_ref()).await?;
                let providers = list_providers(&ctx.ai_env);
                ok(
                    json!({ "selected": selected.as_str(), "providers": providers }),
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
        "/api/ai/providers/select",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let provider = body_str(&body, "provider");
                let selected = select_provider(ctx.db.as_ref(), &provider).await?;
                ok(json!({ "selected": selected.as_str() }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.post(
        "/api/ai/providers/:id/test",
        |ctx| {
            Box::pin(async move {
                let id = ctx.param("id").unwrap_or("").to_string();
                ok(test_provider(&ctx.ai_env, &id).await?, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/ai/mode",
        |ctx| {
            Box::pin(async move {
                let mode = get_ai_mode(ctx.db.as_ref()).await?;
                ok(
                    json!({ "mode": mode.as_str(), "description": mode.description() }),
                    200,
                )
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.put(
        "/api/ai/mode",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let mode = set_ai_mode(ctx.db.as_ref(), &body_str(&body, "mode")).await?;
                ok(
                    json!({ "mode": mode.as_str(), "description": mode.description() }),
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
        "/api/ai/agents",
        |ctx| {
            Box::pin(async move {
                let resolved = resolve_provider(ctx.db.as_ref(), &ctx.ai_env).await?;
                ok(list_agents(resolved.configured), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.post(
        "/api/ai/tasks",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let created = create_task(
                    ctx.db.as_ref(),
                    CreateAiTaskInput {
                        agent: body_str(&body, "agent"),
                        input: body_str(&body, "input"),
                        mode: None,
                    },
                )
                .await?;
                // Tasks run synchronously inside the request in local
                // dev (v1 pattern); in production a Queue consumer
                // calls the same runner.
                let finished =
                    run_task(ctx.db.as_ref(), ctx.site_id, &created.id, &ctx.ai_env, actor(ctx)).await?;
                log_action(ctx, "ai.task", "job", 0).await;
                ok(finished, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/ai/tasks",
        |ctx| {
            Box::pin(async move {
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                ok(list_tasks(ctx.db.as_ref(), page, per_page).await?, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/ai/tasks/:id",
        |ctx| {
            Box::pin(async move {
                let id = ctx.param("id").unwrap_or("").to_string();
                ok(get_task(ctx.db.as_ref(), &id).await?, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/ai/approvals",
        |ctx| {
            Box::pin(async move {
                let status = ctx.query("status").map(|s| s.to_string());
                let status = match status.as_deref() {
                    Some("pending") | Some("approved") | Some("rejected") => status.as_deref(),
                    _ => None,
                };
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                ok(
                    list_approvals(ctx.db.as_ref(), status, page, per_page).await?,
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
        "/api/ai/approvals/:id/approve",
        |ctx| {
            Box::pin(async move {
                let id = ctx.param("id").unwrap_or("").to_string();
                let view = approve_approval(ctx.db.as_ref(), ctx.site_id, &id, actor(ctx)).await?;
                log_action(ctx, "ai.approval_approve", "approval", 0).await;
                ok(view, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.post(
        "/api/ai/approvals/:id/reject",
        |ctx| {
            Box::pin(async move {
                let id = ctx.param("id").unwrap_or("").to_string();
                let body = read_json_body_lenient(ctx);
                let view = reject_approval(
                    ctx.db.as_ref(), ctx.site_id,
                    &id,
                    actor(ctx),
                    opt_str(&body, "reason").as_deref(),
                )
                .await?;
                log_action(ctx, "ai.approval_reject", "approval", 0).await;
                ok(view, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/ai/memory",
        |ctx| Box::pin(async move { ok(list_memory(ctx.db.as_ref()).await?, 200) }),
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.post(
        "/api/ai/memory/notes",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let note = add_memory_note(ctx.db.as_ref(), &body_str(&body, "text")).await?;
                ok(note, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.delete(
        "/api/ai/memory/notes/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "memory note id")?;
                let deleted = delete_memory_note(ctx.db.as_ref(), id).await?;
                ok_json(json!({ "deleted": deleted }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.post(
        "/api/ai/memory/rebuild",
        |ctx| Box::pin(async move { ok(rebuild_memory(ctx.db.as_ref(), ctx.site_id).await?, 200) }),
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    // -------------------------------------------------------- integrations
    router.get(
        "/api/integrations",
        |ctx| {
            Box::pin(async move {
                ok(
                    list_integrations(ctx.db.as_ref(), &ctx.int_secrets, &ctx.int_bindings).await?,
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
        "/api/integrations/:id/config",
        |ctx| {
            Box::pin(async move {
                let id = ctx.param("id").unwrap_or("").to_string();
                ok(get_integration_config(ctx.db.as_ref(), &id).await?, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.put(
        "/api/integrations/:id/config",
        |ctx| {
            Box::pin(async move {
                let id = ctx.param("id").unwrap_or("").to_string();
                let body = read_json_body(ctx)?;
                let patch = body.as_object().cloned().unwrap_or_default();
                ok(
                    set_integration_config(ctx.db.as_ref(), &id, &patch).await?,
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
        "/api/integrations/email/log",
        |ctx| {
            Box::pin(async move {
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                ok(list_mail_log(ctx.db.as_ref(), page, per_page).await?, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/integrations/analytics/status",
        |ctx| Box::pin(async move { ok(get_analytics_status(ctx.db.as_ref()).await?, 200) }),
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/integrations/cloudflare",
        |ctx| {
            Box::pin(async move {
                // P5 F10: v1 returns the binding list as a bare array,
                // not wrapped in an object.
                ok(cloudflare_binding_status(&ctx.int_bindings), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    // --------------------------------------------------------- marketplace
    router.get(
        "/api/marketplace",
        |ctx| Box::pin(async move { ok(list_marketplace_extensions(ctx.db.as_ref()).await?, 200) }),
        RouteOptions {
            require_auth: false,
            capability: Some("install_plugins"),
        },
    );

    router.post(
        "/api/marketplace/install",
        |ctx| {
            Box::pin(async move {
                let store = require_store(ctx)?;
                let content_type = ctx.header("content-type").unwrap_or("").to_string();
                let form = parse_multipart(ctx.body(), &content_type)?;
                let Some(file) = form.file("file") else {
                    return Err(bad_request(
                        "Marketplace install requires a multipart \"file\" field.",
                    ));
                };
                let filename = file
                    .filename
                    .clone()
                    .unwrap_or_else(|| "extension.zip".to_string());
                let data = file.bytes.clone();
                // Build the M11 compatibility report BEFORE installing:
                // analyse every PHP file, auto-convert, migrate literal
                // option defaults, store the generated TS in R2. Any
                // step that cannot run leaves `report: None` ("not
                // analysed") and the install still proceeds — the
                // analyser is advisory, the installer is authoritative.
                let report = build_marketplace_report(ctx, &data).await;
                let installed = install_extension_from_zip(
                    ctx.db.as_ref(),
                    store.as_ref(),
                    &filename,
                    &data,
                    report.as_ref(),
                )
                .await?;
                log_action(ctx, "marketplace.install", "extension", 0).await;
                ok(installed, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("install_plugins"),
        },
    );

    router.post(
        "/api/marketplace/:slug/activate",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                let record = set_marketplace_extension_status(
                    ctx.db.as_ref(),
                    &slug,
                    ExtensionStatus::Active,
                )
                .await?;
                log_action(ctx, "marketplace.activate", "extension", 0).await;
                ok(record, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("install_plugins"),
        },
    );

    router.post(
        "/api/marketplace/:slug/deactivate",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                let record = set_marketplace_extension_status(
                    ctx.db.as_ref(),
                    &slug,
                    ExtensionStatus::Inactive,
                )
                .await?;
                log_action(ctx, "marketplace.deactivate", "extension", 0).await;
                ok(record, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("install_plugins"),
        },
    );

    router.get(
        "/api/marketplace/:slug/report",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                let report = get_marketplace_extension_report(ctx.db.as_ref(), &slug).await?;
                ok(json!({ "slug": slug, "report": report }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("install_plugins"),
        },
    );

    // ------------------------------------------------------------ commerce
    router.get(
        "/api/commerce/products",
        |ctx| {
            Box::pin(async move {
                let status = ctx.query("status").map(|s| s.to_string());
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                ok(
                    list_products(ctx.db.as_ref(), status.as_deref(), page, per_page).await?,
                    200,
                )
            })
        },
        RouteOptions::default(),
    );

    router.post(
        "/api/commerce/products",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let product = create_product(
                    ctx.db.as_ref(),
                    ProductInput {
                        name: json_field(&body, "name"),
                        slug: json_field(&body, "slug"),
                        description: json_field(&body, "description"),
                        price_minor: json_field(&body, "priceMinor"),
                        currency: json_field(&body, "currency"),
                        stock: json_field(&body, "stock"),
                        status: json_field(&body, "status"),
                        image_id: json_field(&body, "imageId"),
                        // P8: the same table carries variable products
                        // and SKUs; the P3 endpoint passes them through.
                        product_type: json_field(&body, "type"),
                        sku: json_field(&body, "sku"),
                    },
                )
                .await?;
                log_action(ctx, "commerce.product_create", "product", product.id).await;
                ok(product, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/commerce/products/:id",
        |ctx| {
            Box::pin(async move {
                let raw = ctx.param("id").unwrap_or("").to_string();
                let product = match raw.parse::<i64>() {
                    Ok(id) => get_product(ctx.db.as_ref(), id)
                        .await?
                        .ok_or_else(|| not_found(&format!("Product \"{raw}\" not found.")))?,
                    Err(_) => get_product_by_slug(ctx.db.as_ref(), &raw)
                        .await?
                        .ok_or_else(|| not_found(&format!("Product \"{raw}\" not found.")))?,
                };
                ok(product, 200)
            })
        },
        RouteOptions::default(),
    );

    router.put(
        "/api/commerce/products/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "product id")?;
                let body = read_json_body(ctx)?;
                let product = update_product(
                    ctx.db.as_ref(),
                    id,
                    ProductInput {
                        name: json_field(&body, "name"),
                        slug: json_field(&body, "slug"),
                        description: json_field(&body, "description"),
                        price_minor: json_field(&body, "priceMinor"),
                        currency: json_field(&body, "currency"),
                        stock: json_field(&body, "stock"),
                        status: json_field(&body, "status"),
                        image_id: json_field(&body, "imageId"),
                        // P8: same passthrough as the create handler.
                        product_type: json_field(&body, "type"),
                        sku: json_field(&body, "sku"),
                    },
                )
                .await?;
                log_action(ctx, "commerce.product_update", "product", product.id).await;
                ok(product, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.delete(
        "/api/commerce/products/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "product id")?;
                let deleted = delete_product(ctx.db.as_ref(), id).await?;
                log_action(ctx, "commerce.product_delete", "product", id).await;
                ok_json(json!({ "deleted": deleted }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/commerce/cart",
        |ctx| {
            Box::pin(async move {
                let (token, set_cookie) = cart_token(ctx);
                let cart = get_cart(ctx.db.as_ref(), &token).await?;
                let mut resp = ok(cart, 200)?;
                if let Some(cookie) = set_cookie {
                    resp = resp.with_header("Set-Cookie", &cookie);
                }
                Ok(resp)
            })
        },
        RouteOptions::default(),
    );

    router.post(
        "/api/commerce/cart",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let (token, set_cookie) = cart_token(ctx);
                let product_id = body["productId"]
                    .as_i64()
                    .ok_or_else(|| bad_request("Field \"productId\" must be an integer."))?;
                let cart =
                    add_to_cart(ctx.db.as_ref(), &token, product_id, &body["quantity"]).await?;
                let mut resp = ok(cart, 200)?;
                if let Some(cookie) = set_cookie {
                    resp = resp.with_header("Set-Cookie", &cookie);
                }
                Ok(resp)
            })
        },
        RouteOptions::default(),
    );

    router.put(
        "/api/commerce/cart",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let (token, _) = cart_token(ctx);
                let product_id = body["productId"]
                    .as_i64()
                    .ok_or_else(|| bad_request("Field \"productId\" must be an integer."))?;
                let cart =
                    set_cart_quantity(ctx.db.as_ref(), &token, product_id, &body["quantity"])
                        .await?;
                ok(cart, 200)
            })
        },
        RouteOptions::default(),
    );

    router.delete(
        "/api/commerce/cart",
        |ctx| {
            Box::pin(async move {
                let (token, _) = cart_token(ctx);
                clear_cart(ctx.db.as_ref(), &token).await?;
                ok_json(json!({ "cleared": true }), 200)
            })
        },
        RouteOptions::default(),
    );

    router.post(
        "/api/commerce/checkout",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let (token, _) = cart_token(ctx);
                let gateway = gateway_for(
                    &resolve_payment_provider(ctx.db.as_ref(), &ctx.payment_secrets)
                        .await?
                        .provider_id,
                    &ctx.payment_secrets,
                )
                .unwrap_or_else(|| Rc::new(MockCheckoutGateway) as Rc<dyn CheckoutGateway>);
                // P5 F2: v1 returns 201 and builds every checkout URL
                // from the stored site URL (`siteUrl(db)`, default
                // http://localhost:8787) — never from the request host.
                let base = site_url(ctx.db.as_ref(), ctx.site_id).await?;
                let outcome = start_checkout(
                    ctx.db.as_ref(),
                    &ctx.payment_secrets,
                    gateway.as_ref(),
                    &token,
                    &body["customerName"],
                    &body["customerEmail"],
                    &base,
                    &format!("{base}/shop/checkout/success"),
                    &format!("{base}/shop/checkout/cancelled"),
                )
                .await?;
                ok(outcome, 201)
            })
        },
        RouteOptions::default(),
    );

    router.get(
        "/api/commerce/orders",
        |ctx| {
            Box::pin(async move {
                let status = ctx.query("status").map(|s| s.to_string());
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                ok(
                    list_orders(ctx.db.as_ref(), status.as_deref(), page, per_page).await?,
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
        "/api/commerce/orders/:id",
        |ctx| {
            Box::pin(async move {
                let id = ctx.param("id").unwrap_or("").to_string();
                let order = get_order(ctx.db.as_ref(), &id)
                    .await?
                    .ok_or_else(|| not_found(&format!("Order \"{id}\" not found.")))?;
                ok(order, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.post(
        "/api/commerce/orders/:id/transition",
        |ctx| {
            Box::pin(async move {
                let id = ctx.param("id").unwrap_or("").to_string();
                let body = read_json_body(ctx)?;
                let order =
                    transition_order(ctx.db.as_ref(), &id, &body_str(&body, "status")).await?;
                if order.status == "paid" {
                    send_order_confirmation(ctx, &order).await;
                }
                log_action(ctx, "commerce.order_transition", "order", 0).await;
                ok(order, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/commerce/payments/providers",
        |ctx| {
            Box::pin(async move {
                // v1 returns just the provider list here.
                ok(list_payment_providers(&ctx.payment_secrets), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/commerce/payments/selected",
        |ctx| {
            Box::pin(async move {
                let provider = get_selected_payment_provider_id(ctx.db.as_ref()).await?;
                let mode = get_payment_mode(ctx.db.as_ref()).await?;
                ok(json!({ "provider": provider, "mode": mode }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.post(
        "/api/commerce/payments/select",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                // v1 always (re)sets the provider — an absent value
                // clears the selection — and sets the mode only when
                // the body carries one.
                let provider = set_selected_provider_id(
                    ctx.db.as_ref(),
                    opt_str(&body, "provider").as_deref(),
                )
                .await?;
                let mode = match opt_str(&body, "mode") {
                    Some(mode) => set_payment_mode(ctx.db.as_ref(), &mode).await?,
                    None => get_payment_mode(ctx.db.as_ref()).await?,
                };
                ok(
                    json!({
                        "provider": provider,
                        "mode": mode,
                        "providers": list_payment_providers(&ctx.payment_secrets),
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

    router.get(
        "/api/commerce/webhooks/log",
        |ctx| {
            Box::pin(async move {
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                ok(
                    list_webhook_log(ctx.db.as_ref(), page, per_page).await?,
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
        "/api/commerce/webhooks/:provider",
        |ctx| {
            Box::pin(async move {
                let provider = ctx.param("provider").unwrap_or("").to_string();
                let raw_body = String::from_utf8_lossy(ctx.body()).into_owned();
                let headers: Vec<(String, String)> = ctx
                    .headers
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let header_refs: Vec<(&str, &str)> = headers
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.as_str()))
                    .collect();
                let outcome = handle_payment_webhook(
                    ctx.db.as_ref(),
                    &ctx.payment_secrets,
                    &provider,
                    &raw_body,
                    &header_refs,
                    (now_ms() / 1000) as i64,
                )
                .await?;
                if outcome.http_status == 401 {
                    return Err(WordlessError::new(
                        "webhook_rejected",
                        401,
                        &outcome.detail,
                    ));
                }
                if outcome.order == Some(WebhookOrderOutcome::Paid) {
                    // Newly-paid: confirmation mail + outbound
                    // `order.paid` event, both best-effort.
                    if let Some(order) = order_for_webhook(ctx, &raw_body).await {
                        send_order_confirmation(ctx, &order).await;
                        dispatch_webhook(
                            ctx,
                            "order.paid",
                            json!({ "orderId": order.id, "reference": order.payment_reference, "totalMinor": order.total_minor }),
                        )
                        .await;
                    }
                }
                ok(outcome, 200)
            })
        },
        RouteOptions::default(),
    );

    // ------------------------------------------------------------ security
    router.get(
        "/api/security/overview",
        |ctx| Box::pin(async move { ok(security_overview(ctx.db.as_ref()).await?, 200) }),
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    // -------------------------------------------------------------- backup
    router.post(
        "/api/backup/create",
        |ctx| {
            Box::pin(async move {
                let store = require_store(ctx)?;
                let job_id = new_job_id()?;
                let payload = json!({ "createdBy": actor(ctx) }).to_string();
                let now = now_iso();
                ctx.db
                    .as_ref()
                    .prepare(
                        "INSERT INTO wl_jobs (id, kind, status, payload, created_at, updated_at) VALUES (?, 'backup_create', 'pending', ?, ?, ?)",
                    )
                    .bind(vec![
                        Param::Text(job_id.clone()),
                        Param::Text(payload),
                        Param::Text(now.clone()),
                        Param::Text(now),
                    ])
                    .run()
                    .await?;
                // The job runs synchronously in the request (v1
                // pattern); a Queue consumer would call the same
                // runner in production.
                run_backup_job(ctx.db.as_ref(), store.as_ref(), &job_id, VERSION).await?;
                let job = job_row(ctx.db.as_ref(), &job_id)
                    .await?
                    .ok_or_else(|| internal_error("Backup job vanished after running."))?;
                // P5 G6: v1's create endpoints return the slim shape
                // {id, status, result} — the full job view (kind,
                // createdAt, updatedAt) is only the job-get shape.
                ok(job_create_view(&job, false), 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/backup",
        |ctx| Box::pin(async move { ok(list_backups(ctx.db.as_ref()).await?, 200) }),
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/backup/:id/download",
        |ctx| {
            Box::pin(async move {
                let store = require_store(ctx)?;
                let id = ctx.param("id").unwrap_or("").to_string();
                let (filename, bytes, _size) =
                    get_backup_download(ctx.db.as_ref(), store.as_ref(), &id).await?;
                Ok(
                    RouteResponse::bytes(bytes, "application/zip", 200).with_header(
                        "Content-Disposition",
                        &format!("attachment; filename=\"{filename}\""),
                    ),
                )
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.post(
        "/api/backup/:id/verify",
        |ctx| {
            Box::pin(async move {
                let store = require_store(ctx)?;
                let id = ctx.param("id").unwrap_or("").to_string();
                ok(
                    verify_backup_report(ctx.db.as_ref(), store.as_ref(), &id).await?,
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
        "/api/backup/:id/restore",
        |ctx| {
            Box::pin(async move {
                let store = require_store(ctx)?;
                let id = ctx.param("id").unwrap_or("").to_string();
                let body = read_json_body(ctx)?;
                let confirmation = body_str(&body, "confirmation");
                ok(
                    restore_backup(
                        ctx.db.as_ref(),
                        store.as_ref(),
                        &id,
                        &confirmation,
                        VERSION,
                        actor(ctx),
                    )
                    .await?,
                    200,
                )
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    // -------------------------------------------------------------- import
    router.post(
        "/api/import/wxr",
        |ctx| {
            Box::pin(async move {
                let store = require_store(ctx)?;
                let content_type = ctx.header("content-type").unwrap_or("").to_string();
                let form = parse_multipart(ctx.body(), &content_type)?;
                let Some(file) = form.file("file") else {
                    return Err(bad_request("Import requires a multipart \"file\" field."));
                };
                if file.bytes.len() > MAX_IMPORT_BYTES {
                    return Err(bad_request("Import file is too large (max 50 MB)."));
                }
                let filename = file
                    .filename
                    .clone()
                    .unwrap_or_else(|| "import.xml".to_string());
                let job =
                    start_import_job(ctx, &store, "import_wxr", &filename, &file.bytes).await?;
                run_import_job(ctx, &store, &job, "import_wxr").await?;
                let row = job_row(ctx.db.as_ref(), &job)
                    .await?
                    .ok_or_else(|| internal_error("Import job vanished after running."))?;
                ok(job_create_view(&row, true), 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.post(
        "/api/import/sql",
        |ctx| {
            Box::pin(async move {
                let store = require_store(ctx)?;
                let content_type = ctx.header("content-type").unwrap_or("").to_string();
                let form = parse_multipart(ctx.body(), &content_type)?;
                let Some(file) = form.file("file") else {
                    return Err(bad_request("Import requires a multipart \"file\" field."));
                };
                if file.bytes.len() > MAX_IMPORT_BYTES {
                    return Err(bad_request("Import file is too large (max 50 MB)."));
                }
                let filename = file
                    .filename
                    .clone()
                    .unwrap_or_else(|| "import.sql".to_string());
                let job =
                    start_import_job(ctx, &store, "import_sql", &filename, &file.bytes).await?;
                run_import_job(ctx, &store, &job, "import_sql").await?;
                let row = job_row(ctx.db.as_ref(), &job)
                    .await?
                    .ok_or_else(|| internal_error("Import job vanished after running."))?;
                ok(job_create_view(&row, true), 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/import/jobs/:id",
        |ctx| {
            Box::pin(async move {
                let id = ctx.param("id").unwrap_or("").to_string();
                let row = job_row(ctx.db.as_ref(), &id)
                    .await?
                    .ok_or_else(|| not_found("Import job not found."))?;
                ok(job_view(&row), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    // ------------------------------------------------------------- graphql
    router.post(
        "/api/graphql",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let query = body_str(&body, "query");
                if query.is_empty() {
                    return Err(bad_request("Field \"query\" is required."));
                }
                let variables = body.get("variables").cloned();
                let operation_name = opt_str(&body, "operationName");
                let outcome = graphql_execute(
                    ctx.db.as_ref(), ctx.site_id,
                    ctx.user(),
                    &query,
                    variables,
                    operation_name.as_deref(),
                )
                .await;
                // GraphQL answers 200 even with field errors (the
                // errors array carries them); only a missing query
                // is a 400 above.
                ok_json(
                    json!({ "data": outcome.data, "errors": outcome.errors }),
                    200,
                )
            })
        },
        RouteOptions::default(),
    );

    // ------------------------------------------- outbound webhooks (v1.3)
    router.get(
        "/api/webhooks/subscriptions",
        |ctx| Box::pin(async move { ok(list_subscriptions(ctx.db.as_ref()).await?, 200) }),
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.post(
        "/api/webhooks/subscriptions",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let url = body_str(&body, "url");
                let events: Vec<String> = body
                    .get("events")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                let (subscription, secret) =
                    create_subscription(ctx.db.as_ref(), &url, &events).await?;
                // The secret is returned exactly once, at creation.
                ok(
                    json!({ "subscription": subscription, "secret": secret }),
                    201,
                )
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.delete(
        "/api/webhooks/subscriptions/:id",
        |ctx| {
            Box::pin(async move {
                let id = ctx.param("id").unwrap_or("").to_string();
                let deleted = delete_subscription(ctx.db.as_ref(), &id).await?;
                ok_json(json!({ "deleted": deleted }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/webhooks/deliveries",
        |ctx| {
            Box::pin(async move {
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                let (deliveries, total) = list_deliveries(ctx.db.as_ref(), page, per_page).await?;
                ok(
                    json!({
                        "deliveries": deliveries,
                        "total": total,
                        "page": page,
                        "perPage": per_page,
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
        "/api/webhooks/deliveries/:id/retry",
        |ctx| {
            Box::pin(async move {
                let id = ctx.param("id").unwrap_or("").to_string();
                let retried = retry_one(ctx.db.as_ref(), ctx.http_post.as_ref(), &id).await?;
                ok_json(json!({ "retried": retried }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.post(
        "/api/webhooks/deliveries/retry-due",
        |ctx| {
            Box::pin(async move {
                let retried = retry_due(ctx.db.as_ref(), ctx.http_post.as_ref()).await?;
                ok_json(json!({ "retried": retried }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    // ----------------------------------------------------------------- shop
    // Public storefront (v1 `/shop` pages). Forms are urlencoded or
    // multipart; cart state travels in the `wordless_cart` cookie.
    router.get(
        "/shop",
        |ctx| {
            Box::pin(async move {
                let page = page_param(ctx)?;
                let (token, set_cookie) = cart_token(ctx);
                let cart = get_cart(ctx.db.as_ref(), &token).await.ok();
                // P5 F6: shop pages carry the SEO head (v1 finishPage
                // builds it with kind "page", the site description,
                // and canonical = the page path).
                let tagline = get_settings(ctx.db.as_ref(), ctx.site_id)
                    .await
                    .map(|s| s.site_tagline)
                    .unwrap_or_default();
                let opts = with_seo(
                    ctx,
                    render_opts(ctx).await,
                    PageKind::Page,
                    "Shop".to_string(),
                    tagline,
                    Some("/shop".to_string()),
                    None,
                    vec![Breadcrumb {
                        name: "Shop".to_string(),
                        path: "/shop".to_string(),
                    }],
                    None,
                )
                .await;
                let rendered = render_shop(ctx.db.as_ref(), ctx.site_id, page, cart.as_ref(), &opts).await?;
                let mut resp = html_page(rendered);
                if let Some(cookie) = set_cookie {
                    resp = resp.with_header("Set-Cookie", &cookie);
                }
                Ok(resp)
            })
        },
        RouteOptions::default(),
    );

    router.get(
        "/shop/cart",
        |ctx| {
            Box::pin(async move {
                let (token, set_cookie) = cart_token(ctx);
                let cart = get_cart(ctx.db.as_ref(), &token).await?;
                let notice = ctx.query("notice").map(|s| s.to_string());
                let tagline = get_settings(ctx.db.as_ref(), ctx.site_id)
                    .await
                    .map(|s| s.site_tagline)
                    .unwrap_or_default();
                let opts = with_seo(
                    ctx,
                    render_opts(ctx).await,
                    PageKind::Page,
                    "Your cart".to_string(),
                    tagline,
                    Some("/shop/cart".to_string()),
                    None,
                    Vec::new(),
                    None,
                )
                .await;
                let rendered =
                    render_cart_page(ctx.db.as_ref(), ctx.site_id, &cart, &opts, notice.as_deref()).await?;
                let mut resp = html_page(rendered);
                if let Some(cookie) = set_cookie {
                    resp = resp.with_header("Set-Cookie", &cookie);
                }
                Ok(resp)
            })
        },
        RouteOptions::default(),
    );

    router.get(
        "/shop/checkout/test",
        |ctx| {
            Box::pin(async move {
                let order_id = ctx.query("order").unwrap_or("").to_string();
                // v1 renders the not-found page when the order is
                // missing OR no reference was supplied.
                let reference = ctx.query("reference").unwrap_or("").to_string();
                let order = get_order(ctx.db.as_ref(), &order_id).await?;
                if order.is_none() || reference.is_empty() {
                    return render_not_found_page(ctx).await;
                }
                let order = order.unwrap();
                let tagline = get_settings(ctx.db.as_ref(), ctx.site_id)
                    .await
                    .map(|s| s.site_tagline)
                    .unwrap_or_default();
                let opts = with_seo(
                    ctx,
                    render_opts(ctx).await,
                    PageKind::Page,
                    "Test checkout".to_string(),
                    tagline,
                    Some("/shop/checkout/test".to_string()),
                    None,
                    Vec::new(),
                    None,
                )
                .await;
                let rendered =
                    render_test_checkout(ctx.db.as_ref(), ctx.site_id, &order, &reference, &opts).await?;
                Ok(html_page(rendered))
            })
        },
        RouteOptions::default(),
    );

    router.get(
        "/shop/checkout/success",
        |ctx| {
            Box::pin(async move {
                let order = match ctx.query("order") {
                    Some(id) => get_order(ctx.db.as_ref(), id).await.ok().flatten(),
                    None => None,
                };
                let tagline = get_settings(ctx.db.as_ref(), ctx.site_id)
                    .await
                    .map(|s| s.site_tagline)
                    .unwrap_or_default();
                // v1's renderCheckoutResult always uses the success
                // path as canonical, for both success and cancelled.
                let opts = with_seo(
                    ctx,
                    render_opts(ctx).await,
                    PageKind::Page,
                    "Checkout".to_string(),
                    tagline,
                    Some("/shop/checkout/success".to_string()),
                    None,
                    Vec::new(),
                    None,
                )
                .await;
                let rendered =
                    render_checkout_result(ctx.db.as_ref(), ctx.site_id, order.as_ref(), &opts).await?;
                Ok(html_page(rendered))
            })
        },
        RouteOptions::default(),
    );

    router.get(
        "/shop/checkout/cancelled",
        |ctx| {
            Box::pin(async move {
                let order = match ctx.query("order") {
                    Some(id) => get_order(ctx.db.as_ref(), id).await.ok().flatten(),
                    None => None,
                };
                let tagline = get_settings(ctx.db.as_ref(), ctx.site_id)
                    .await
                    .map(|s| s.site_tagline)
                    .unwrap_or_default();
                let opts = with_seo(
                    ctx,
                    render_opts(ctx).await,
                    PageKind::Page,
                    "Checkout".to_string(),
                    tagline,
                    Some("/shop/checkout/success".to_string()),
                    None,
                    Vec::new(),
                    None,
                )
                .await;
                let rendered =
                    render_checkout_result(ctx.db.as_ref(), ctx.site_id, order.as_ref(), &opts).await?;
                Ok(html_page(rendered))
            })
        },
        RouteOptions::default(),
    );

    router.get(
        "/shop/:slug",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                let Some(product) = get_shop_product(ctx.db.as_ref(), &slug).await? else {
                    return crate::routes::render_not_found_page(ctx).await;
                };
                let tagline = get_settings(ctx.db.as_ref(), ctx.site_id)
                    .await
                    .map(|s| s.site_tagline)
                    .unwrap_or_default();
                // P10: Product JSON-LD + visible breadcrumbs (Shop →
                // product). Variable products report the parent's
                // minimum price ("From …" on the page) and are
                // reported in stock — per-variation stock is not
                // visible to this handler (recorded limit).
                let base_url = site_url(ctx.db.as_ref(), ctx.site_id).await?;
                let image_url = if product.image_id > 0 {
                    match get_media_record(ctx.db.as_ref(), ctx.site_id, product.image_id).await? {
                        Some(record) if !record.url.is_empty() => {
                            Some(if record.url.starts_with("http") {
                                record.url.clone()
                            } else {
                                format!("{base_url}{}", record.url)
                            })
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                let product_seo = ProductSeo {
                    name: product.name.clone(),
                    description: plain_text_excerpt(&product.description, 160),
                    url: format!("{base_url}/shop/{}", product.slug),
                    price_minor: product.price_minor,
                    currency: product.currency.clone(),
                    in_stock: if product.product_type == "variable" {
                        true
                    } else {
                        product.stock != 0
                    },
                    image_url,
                };
                let opts = with_seo(
                    ctx,
                    render_opts(ctx).await,
                    PageKind::Page,
                    product.name.clone(),
                    tagline,
                    Some(format!("/shop/{}", product.slug)),
                    None,
                    vec![
                        Breadcrumb {
                            name: "Shop".to_string(),
                            path: "/shop".to_string(),
                        },
                        Breadcrumb {
                            name: product.name.clone(),
                            path: format!("/shop/{}", product.slug),
                        },
                    ],
                    Some(product_seo),
                )
                .await;
                let rendered = render_product_page(ctx.db.as_ref(), ctx.site_id, &product, &opts).await?;
                Ok(html_page(rendered))
            })
        },
        RouteOptions::default(),
    );

    router.post(
        "/shop/cart/add",
        |ctx| {
            Box::pin(async move {
                let form = parse_form(ctx)?;
                let (token, set_cookie) = cart_token(ctx);
                // P5 F3: v1's shop forms use camelCase (`productId`).
                let product_id = form
                    .field("productId")
                    .and_then(|v| v.parse::<i64>().ok())
                    .ok_or_else(|| bad_request("Field \"productId\" must be an integer."))?;
                let quantity = form
                    .field("quantity")
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(1);
                // v1 catches an add failure and 303s back to the cart
                // with the reason as a notice — never a bare JSON error.
                // P8: the variation comes from an explicit variationId
                // or from the shopper's attribute choices (`attr_<slug>`
                // fields on the product page), resolved server-side to
                // the matching published variation.
                let added = async {
                    // Size remedy: variation resolution lives in the
                    // Woo Worker — the main Worker forwards the raw
                    // form (the Woo Worker lists the attributes and
                    // matches the selections itself). Behaviour is
                    // preserved exactly: a form with NO attr_ fields
                    // still resolves to None (add_to_cart_variation
                    // refuses a bare variable product downstream);
                    // attr_ fields with no match are still the same
                    // 400 ("No variation matches those options…").
                    let variation_id = match form
                        .field("variationId")
                        .and_then(|v| v.parse::<i64>().ok())
                    {
                        Some(id) => Some(id),
                        None => {
                            if !form.fields.iter().any(|(k, _)| k.starts_with("attr_")) {
                                None
                            } else {
                                let client = woo_client(ctx)?;
                                let form_json = Value::Object(
                                    form.fields
                                        .iter()
                                        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                                        .collect(),
                                );
                                let resolved = client
                                    .resolve_variation(product_id, &form_json)
                                    .await
                                    .map_err(|e| e.to_wordless())?;
                                match resolved {
                                    Some(id) => Some(id),
                                    // `?` keeps the failure inside
                                    // `added`, so the notice branch
                                    // below 303s with it.
                                    None => Err(bad_request(
                                        "No variation matches those options — please choose each option.",
                                    ))?,
                                }
                            }
                        }
                    };
                    add_to_cart_variation(
                        ctx.db.as_ref(),
                        &token,
                        product_id,
                        &json!(quantity),
                        variation_id,
                    )
                    .await
                }
                .await;
                if let Err(e) = added
                {
                    let message = if e.message.is_empty() {
                        "Could not add that product to the cart.".to_string()
                    } else {
                        e.message.clone()
                    };
                    let mut resp = see_other_response(&format!(
                        "/shop/cart?notice={}",
                        url_encode(&message)
                    ));
                    if let Some(cookie) = set_cookie {
                        resp = resp.with_header("Set-Cookie", &cookie);
                    }
                    return Ok(resp);
                }
                let mut resp = see_other_response("/shop/cart");
                if let Some(cookie) = set_cookie {
                    resp = resp.with_header("Set-Cookie", &cookie);
                }
                Ok(resp)
            })
        },
        RouteOptions::default(),
    );

    router.post(
        "/shop/cart/remove",
        |ctx| {
            Box::pin(async move {
                let form = parse_form(ctx)?;
                let (token, _) = cart_token(ctx);
                let product_id = form
                    .field("productId")
                    .and_then(|v| v.parse::<i64>().ok())
                    .ok_or_else(|| bad_request("Field \"productId\" must be an integer."))?;
                // Quantity 0 removes the line (cart service rule).
                // P8: the variation id picks the (product, variation)
                // line; without it the simple-product line is removed.
                let variation_id = form
                    .field("variationId")
                    .and_then(|v| v.parse::<i64>().ok());
                set_cart_quantity_variation(
                    ctx.db.as_ref(),
                    &token,
                    product_id,
                    &json!(0),
                    variation_id,
                )
                .await?;
                Ok(see_other_response("/shop/cart"))
            })
        },
        RouteOptions::default(),
    );

    router.post(
        "/shop/cart/quantity",
        |ctx| {
            Box::pin(async move {
                let form = parse_form(ctx)?;
                let (token, _) = cart_token(ctx);
                let product_id = form
                    .field("productId")
                    .and_then(|v| v.parse::<i64>().ok())
                    .ok_or_else(|| bad_request("Field \"productId\" must be an integer."))?;
                let quantity = form
                    .field("quantity")
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(1);
                // P8: the variation id picks the (product, variation)
                // line; without it the simple-product line is set.
                let variation_id = form
                    .field("variationId")
                    .and_then(|v| v.parse::<i64>().ok());
                set_cart_quantity_variation(
                    ctx.db.as_ref(),
                    &token,
                    product_id,
                    &json!(quantity),
                    variation_id,
                )
                .await?;
                Ok(see_other_response("/shop/cart"))
            })
        },
        RouteOptions::default(),
    );

    router.post(
        "/shop/checkout",
        |ctx| {
            Box::pin(async move {
                let form = parse_form(ctx)?;
                let (token, _) = cart_token(ctx);
                // P5 F3/F4: v1's shop forms use camelCase; a checkout
                // that cannot start 303s to the cart with the reason
                // as a notice — never a bare JSON error.
                let customer_name = form.field("customerName").unwrap_or("").to_string();
                let customer_email = form.field("customerEmail").unwrap_or("").to_string();
                let base = site_url(ctx.db.as_ref(), ctx.site_id).await?;
                let started = async {
                    // Size remedy: the whole Woo checkout (totals
                    // pre-check — a bad coupon is still a 400 before
                    // the provider is resolved — provider resolution,
                    // gateway, order) runs in the Woo Worker, in one
                    // hop. The outcome shape (order, checkout_url,
                    // reference) is unchanged, so the redirect logic
                    // below holds.
                    let client = woo_client(ctx)?;
                    let totals_input = json!({
                        "couponCode": form.field("couponCode").unwrap_or(""),
                        "country": form.field("country").unwrap_or(""),
                        "state": "",
                        "shippingMethodId": form
                            .field("shippingMethodId")
                            .and_then(|v| v.parse::<i64>().ok()),
                    });
                    let outcome = client
                        .shop_checkout(
                            &token,
                            &json!(customer_name),
                            &json!(customer_email),
                            &totals_input,
                            &base,
                            &format!("{base}/shop/checkout/success"),
                            &format!("{base}/shop/checkout/cancelled"),
                        )
                        .await
                        .map_err(|e| e.to_wordless())?;
                    let provider_id = outcome["providerId"].as_str().unwrap_or("").to_string();
                    let order_id = outcome["order"]["id"].as_str().unwrap_or("").to_string();
                    let reference = outcome["reference"].as_str().unwrap_or("").to_string();
                    let checkout_url = outcome["checkoutUrl"].as_str().unwrap_or("").to_string();
                    Ok::<_, WordlessError>((provider_id, order_id, reference, checkout_url))
                }
                .await;
                match started {
                    Ok((provider_id, order_id, reference, checkout_url)) => {
                        if provider_id == PAYMENT_PROVIDER_MOCK {
                            // Mock checkout: the test-payment page lives
                            // at /shop/checkout/test (v1 303s there with
                            // the order + reference in the query).
                            Ok(see_other_response(&format!(
                                "/shop/checkout/test?order={}&reference={}",
                                order_id,
                                url_encode(&reference)
                            )))
                        } else {
                            Ok(see_other_response(&checkout_url))
                        }
                    }
                    Err(e) => {
                        let message = if e.message.is_empty() {
                            "Checkout could not be started.".to_string()
                        } else {
                            e.message.clone()
                        };
                        Ok(see_other_response(&format!(
                            "/shop/cart?notice={}",
                            url_encode(&message)
                        )))
                    }
                }
            })
        },
        RouteOptions::default(),
    );

    router.post(
        "/shop/checkout/test",
        |ctx| {
            Box::pin(async move {
                // P5 F5: v1's branches, exactly. The SUBMITTED
                // reference is the one the webhook is simulated
                // with — never the stored one.
                let form = parse_form(ctx)?;
                let order_id = form.field("orderId").unwrap_or("").to_string();
                let reference = form.field("reference").unwrap_or("").to_string();
                let order = get_order(ctx.db.as_ref(), &order_id).await?;
                let Some(order) = order else {
                    return Ok(see_other_response("/shop/cart"));
                };
                if order.payment_provider != PAYMENT_PROVIDER_MOCK
                    || order.payment_reference != reference
                {
                    return Ok(see_other_response(&format!(
                        "/shop/checkout/success?order={}",
                        order.id
                    )));
                }
                let Some(secret) = ctx
                    .payment_secrets
                    .mock_webhook_secret
                    .clone()
                    .filter(|k| !k.is_empty())
                else {
                    return Ok(see_other_response(&format!(
                        "/shop/cart?notice={}",
                        url_encode("Test checkout is not enabled: set the MOCK_PAYMENT_WEBHOOK_SECRET secret (wrangler secret put) first.")
                    )));
                };
                // Simulate the provider's webhook: the mock provider's
                // own signature scheme signs the event, and the real
                // webhook handler does the verification + the
                // pending→paid transition.
                let body = json!({
                    "event": "payment.paid",
                    "reference": reference,
                })
                .to_string();
                let signature = hmac_hex_sha256(&secret, &body);
                let outcome = handle_payment_webhook(
                    ctx.db.as_ref(),
                    &ctx.payment_secrets,
                    "mock",
                    &body,
                    &[("x-mock-signature", signature.as_str())],
                    (now_ms() / 1000) as i64,
                )
                .await?;
                // v1 redirects to success either way; a webhook that
                // did not pay adds &webhook=failed.
                let paid = outcome.order == Some(WebhookOrderOutcome::Paid);
                if paid {
                    send_order_confirmation(ctx, &order).await;
                    dispatch_webhook(
                        ctx,
                        "order.paid",
                        json!({ "orderId": order.id, "reference": order.payment_reference, "totalMinor": order.total_minor }),
                    )
                    .await;
                }
                Ok(see_other_response(&format!(
                    "/shop/checkout/success?order={}{}",
                    order.id,
                    if paid { "" } else { "&webhook=failed" }
                )))
            })
        },
        RouteOptions::default(),
    );
}

// ------------------------------------------------------------------ helpers

/// 50 MB import cap (v1 `MAX_IMPORT_BYTES`, M11).
const MAX_IMPORT_BYTES: usize = 50 * 1024 * 1024;

/// Random UUID v4 (same shape as the conversion crate's `new_job_id`).
fn new_job_id() -> Result<String, WordlessError> {
    let bytes = wordless_kernel::crypto::random_bytes(16)
        .map_err(|e| internal_error(&format!("random ID failed: {e}")))?;
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

/// One JSON field, or `None` when it is absent or JSON null.
/// (v1's `body.field ?? undefined`: null counts as absent. Passing
/// `Some(Null)` instead would make the service validators reject the
/// field — e.g. `require_int` on a missing `imageId`.)
fn json_field(body: &Value, key: &str) -> Option<Value> {
    body.get(key).filter(|v| !v.is_null()).cloned()
}

/// Read a JSON body leniently: empty or invalid bodies become `Null`
/// (v1's approve/reject routes tolerate a missing body).
fn read_json_body_lenient(ctx: &Ctx) -> Value {
    if ctx.body().is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(ctx.body()).unwrap_or(Value::Null)
}

/// One `wl_jobs` row, or `None`.
async fn job_row(db: &dyn Db, id: &str) -> Result<Option<Row>, WordlessError> {
    db.prepare("SELECT * FROM wl_jobs WHERE id = ? LIMIT 1")
        .bind(vec![Param::Text(id.to_string())])
        .first::<Row>()
        .await
        .map_err(WordlessError::from)
}

/// The v1 job view: `{id, kind, status, result, createdAt, updatedAt}`.
/// The create-endpoint shape (v1): backup-create returns
/// `{id, status, result}`; import-create returns
/// `{id, kind, status, result}`. The full job view (with `kind`,
/// `createdAt`, `updatedAt`) is only the `GET /api/import/jobs/:id`
/// shape — P5 G6/G8.
fn job_create_view(row: &Row, with_kind: bool) -> Value {
    let text = |k: &str| {
        row.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let result: Value = row
        .get("result")
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);
    let mut out = json!({
        "id": text("id"),
        "status": text("status"),
        "result": result,
    });
    if with_kind {
        out["kind"] = Value::String(text("kind"));
    }
    out
}

fn job_view(row: &Row) -> Value {
    let text = |k: &str| {
        row.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let result: Value = row
        .get("result")
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);
    json!({
        "id": text("id"),
        "kind": text("kind"),
        "status": text("status"),
        "result": result,
        "createdAt": text("created_at"),
        "updatedAt": text("updated_at"),
    })
}

/// Parse a shop form: multipart via the media parser, urlencoded via a
/// small hand-rolled parser (the multipart parser is multipart-only).
/// The WooCommerce Worker client, or a clear 500 when no transport
/// is configured (the shop handlers are strict: unlike the public
/// render, a checkout that cannot reach the Woo Worker fails
/// loudly instead of silently degrading).
fn woo_client(ctx: &Ctx) -> Result<Rc<dyn WooClient>, WordlessError> {
    ctx.woo_client.clone().ok_or_else(|| {
        WordlessError::new(
            "internal_error",
            500,
            "WooCommerce service is not configured.",
        )
    })
}

fn parse_form(ctx: &Ctx) -> Result<wordless_media::multipart::MultipartForm, WordlessError> {
    let content_type = ctx.header("content-type").unwrap_or("").to_string();
    if content_type.contains("application/x-www-form-urlencoded") {
        let mut form = wordless_media::multipart::MultipartForm::default();
        let body = String::from_utf8_lossy(ctx.body());
        for pair in body.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (k, v) = match pair.split_once('=') {
                Some((k, v)) => (k, v),
                None => (pair, ""),
            };
            form.fields.push((url_decode(k), url_decode(v)));
        }
        return Ok(form);
    }
    parse_multipart(ctx.body(), &content_type)
}

/// Percent-decode a urlencoded field (`+` → space, `%XX` → byte).
fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = |c: u8| match c {
                    b'0'..=b'9' => Some(c - b'0'),
                    b'a'..=b'f' => Some(c - b'a' + 10),
                    b'A'..=b'F' => Some(c - b'A' + 10),
                    _ => None,
                };
                match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push(h * 16 + l);
                        i += 3;
                    }
                    _ => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The cart token from the request cookie, or a fresh one (with the
/// `Set-Cookie` value to send back when a new token was minted).
fn cart_token(ctx: &Ctx) -> (String, Option<String>) {
    match parse_cart_cookie(ctx.header("cookie")) {
        Some(token) => (token, None),
        None => match new_cart_token() {
            Ok(token) => {
                let cookie = cart_cookie_header(&token);
                (token, Some(cookie))
            }
            Err(_) => (String::new(), None),
        },
    }
}

/// Best-effort outbound webhook dispatch: a delivery failure must
/// never fail the post/comment/order call that triggered it.
pub(crate) async fn dispatch_webhook(ctx: &Ctx, event: &str, payload: Value) {
    let _ = dispatch_event(ctx.db.as_ref(), ctx.http_post.as_ref(), event, &payload).await;
}

/// The webhook payload for a post event (id/slug/title/status/type).
pub(crate) fn post_webhook_payload(post: &CmsPost) -> Value {
    json!({
        "id": post.id,
        "slug": post.slug,
        "title": post.title,
        "status": post.status,
        "type": post.post_type,
    })
}

/// Best-effort order-confirmation mail (v1 sends it when a webhook
/// marks an order paid). With the mock email provider this only logs
/// to `wl_mail_log` — never presented as delivered.
async fn send_order_confirmation(ctx: &Ctx, order: &wordless_commerce::types::Order) {
    let amount = wordless_commerce::money::format_money(order.total_minor, &order.currency);
    let _ = send_mail(
        ctx.db.as_ref(),
        &ctx.int_secrets,
        &WorkerHttpClient,
        &SendMailInput {
            to: order.customer_email.clone(),
            subject: format!("Your order {} is confirmed", order.id),
            text: format!(
                "Thank you — your order {} for {} is confirmed and paid.",
                order.id, amount
            ),
            html: None,
        },
    )
    .await;
}

/// Find the order a payment webhook refers to, by trying the
/// reference shapes each provider puts in its payload
/// (`data.reference` / `data.tx_ref` / `data.object.id` /
/// `data.id` / top-level `reference`). Best-effort: `None` just
/// means no confirmation mail / `order.paid` dispatch for this call.
async fn order_for_webhook(ctx: &Ctx, raw_body: &str) -> Option<wordless_commerce::types::Order> {
    let body: Value = serde_json::from_str(raw_body).ok()?;
    let candidates = [
        body.pointer("/data/reference"),
        body.pointer("/data/tx_ref"),
        body.pointer("/data/object/id"),
        body.pointer("/data/id"),
        body.get("reference"),
    ];
    for candidate in candidates.into_iter().flatten() {
        if let Some(reference) = candidate.as_str() {
            if !reference.is_empty() {
                if let Ok(Some(order)) = get_order_by_reference(ctx.db.as_ref(), reference).await {
                    return Some(order);
                }
            }
        }
    }
    None
}

/// Store an import upload in R2 and create its `wl_jobs` row (v1
/// import route shape). Returns the job id.
async fn start_import_job(
    ctx: &Ctx,
    store: &Rc<dyn wordless_kernel::storage::ObjectStore>,
    kind: &str,
    filename: &str,
    bytes: &[u8],
) -> Result<String, WordlessError> {
    let job_id = new_job_id()?;
    let sanitised: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(100)
        .collect();
    let sanitised = if sanitised.is_empty() {
        "import".to_string()
    } else {
        sanitised
    };
    let r2_key = format!("imports/{job_id}-{sanitised}");
    store
        .put_object(&r2_key, bytes.to_vec(), "application/octet-stream")
        .await?;
    // P5 G9: v1 stores the filename in the job payload — the
    // import report's `source` is the filename, not the kind.
    let payload = json!({ "r2Key": r2_key, "filename": filename }).to_string();
    let now = now_iso();
    ctx.db
        .as_ref()
        .prepare(
            "INSERT INTO wl_jobs (id, kind, status, payload, created_at, updated_at) VALUES (?, ?, 'pending', ?, ?, ?)",
        )
        .bind(vec![
            Param::Text(job_id.clone()),
            Param::Text(kind.to_string()),
            Param::Text(payload),
            Param::Text(now.clone()),
            Param::Text(now),
        ])
        .run()
        .await?;
    Ok(job_id)
}

/// Run an import job synchronously (v1 `runImportJob`): read the
/// uploaded file back from R2, import it, and record the outcome on
/// the job row. An import failure marks the job `failed` — it does
/// not propagate as an HTTP error (the job itself was created fine).
async fn run_import_job(
    ctx: &Ctx,
    store: &Rc<dyn wordless_kernel::storage::ObjectStore>,
    job_id: &str,
    kind: &str,
) -> Result<(), WordlessError> {
    let row = job_row(ctx.db.as_ref(), job_id).await?;
    let Some(row) = row else {
        return Ok(());
    };
    let payload: Value = row
        .get("payload")
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);
    let r2_key = payload
        .get("r2Key")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let filename = payload
        .get("filename")
        .and_then(|v| v.as_str())
        .unwrap_or("import")
        .to_string();
    let now = now_iso();
    ctx.db
        .as_ref()
        .prepare("UPDATE wl_jobs SET status = 'running', updated_at = ? WHERE id = ?")
        .bind(vec![Param::Text(now), Param::Text(job_id.to_string())])
        .run()
        .await?;
    let outcome: Result<Value, WordlessError> = async {
        let Some((bytes, _)) = store.get_object(&r2_key).await? else {
            return Err(internal_error("Import file is no longer in storage."));
        };
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let report = if kind == "import_wxr" {
            import_wxr_with_media(
                ctx.db.as_ref(), ctx.site_id,
                &text,
                actor(ctx),
                store.as_ref(),
                &WorkerAttachmentFetcher,
                &filename,
            )
            .await?
        } else {
            import_sql(ctx.db.as_ref(), ctx.site_id, &text, actor(ctx), &filename).await?
        };
        serde_json::to_value(&report)
            .map_err(|e| internal_error(&format!("Import report JSON failed: {e}")))
    }
    .await;
    let now = now_iso();
    match outcome {
        Ok(result) => {
            ctx.db
                .as_ref()
                .prepare(
                    "UPDATE wl_jobs SET status = 'done', result = ?, updated_at = ? WHERE id = ?",
                )
                .bind(vec![
                    Param::Text(result.to_string()),
                    Param::Text(now),
                    Param::Text(job_id.to_string()),
                ])
                .run()
                .await?;
        }
        Err(e) => {
            let result = json!({ "error": e.message }).to_string();
            ctx.db
                .as_ref()
                .prepare(
                    "UPDATE wl_jobs SET status = 'failed', result = ?, updated_at = ? WHERE id = ?",
                )
                .bind(vec![
                    Param::Text(result),
                    Param::Text(now),
                    Param::Text(job_id.to_string()),
                ])
                .run()
                .await?;
        }
    }
    Ok(())
}

/// Extract the text files of a marketplace ZIP: at most 10,000 files
/// / 50 MB total (M11 caps), traversal rejected, a single top-level
/// folder stripped (the usual plugin-ZIP layout). Binary files are
/// skipped for analysis but counted in `totalFiles` by the caller —
/// this helper returns only decodable text files.
fn extract_zip_text_files(data: &[u8]) -> Result<Vec<(String, String)>, String> {
    use std::io::Cursor;
    let mut archive =
        zip::ZipArchive::new(Cursor::new(data)).map_err(|e| format!("Invalid ZIP file: {e}"))?;
    if archive.len() > 10_000 {
        return Err("ZIP contains more than 10,000 files.".to_string());
    }
    let mut total_bytes: usize = 0;
    let mut paths: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("Failed to read ZIP entry: {e}"))?;
        let name = file.name().to_string();
        if file.is_dir() || name.ends_with('/') {
            continue;
        }
        // Traversal / absolute paths are rejected outright.
        if name.starts_with('/') || name.contains("..") {
            return Err(format!("ZIP entry \"{name}\" escapes its folder."));
        }
        let mut bytes = Vec::new();
        use std::io::Read;
        file.read_to_end(&mut bytes)
            .map_err(|e| format!("Failed to read ZIP entry \"{name}\": {e}"))?;
        total_bytes += bytes.len();
        if total_bytes > 50 * 1024 * 1024 {
            return Err("ZIP is larger than 50 MB.".to_string());
        }
        paths.push((name, bytes));
    }
    // Strip a single shared top-level folder (plugin ZIPs ship as
    // `slug/...`).
    let first_segments: Vec<&str> = paths
        .iter()
        .map(|(p, _)| p.split('/').next().unwrap_or(""))
        .collect();
    let shared: Option<String> = first_segments
        .first()
        .copied()
        .filter(|first| {
            !first.is_empty()
                && first_segments.iter().all(|s| s == first)
                && paths.iter().any(|(p, _)| p.contains('/'))
        })
        .map(|s| s.to_string());
    let mut out = Vec::new();
    for (path, bytes) in paths {
        let stripped = match shared.as_deref() {
            Some(prefix) => path
                .strip_prefix(&format!("{prefix}/"))
                .unwrap_or(&path)
                .to_string(),
            None => path,
        };
        if stripped.is_empty() {
            continue;
        }
        // Only text files are analysable; the rest (images, etc.)
        // are counted by totalFiles in the report but not analysed.
        let text = String::from_utf8_lossy(&bytes).into_owned();
        out.push((stripped, text));
    }
    Ok(out)
}

/// Build the M11 compatibility report for a marketplace upload
/// (v1 `analyzePluginFiles` shape): per-file verdicts from the
/// faithful analyser, top blocking reasons, option defaults, and the
/// auto-convert record with generated TS stored in R2. Returns
/// `None` when the upload cannot be analysed (no manifest slug, or
/// the ZIP cannot be read) — the install then proceeds with
/// `report: None` ("not analysed").
async fn build_marketplace_report(ctx: &Ctx, data: &[u8]) -> Option<Value> {
    let files = extract_zip_text_files(data).ok()?;
    let total_files = files.len();
    let manifest = files
        .iter()
        .find(|(p, _)| p == "wordless.json")
        .map(|(_, s)| s.clone())?;
    let slug = serde_json::from_str::<Value>(&manifest)
        .ok()?
        .get("slug")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())?;
    if slug.is_empty() {
        return None;
    }
    let php_files: Vec<&(String, String)> = files
        .iter()
        .filter(|(p, _)| p.to_lowercase().ends_with(".php"))
        .collect();
    // Split (2026-11-20): the analyser runs in the wordless-converter
    // Worker. Files within the analyser caps (2,000 PHP files, 1 MB
    // each) go over in ONE round trip; the rest are skipped exactly as
    // before. A missing/unreachable converter returns None ("not
    // analysed") and the install still proceeds — the analyser is
    // advisory, the installer is authoritative (see the caller's
    // comment at the marketplace install route).
    let converter = ctx.converter.as_deref()?;
    let mut skipped_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut to_analyse: Vec<(String, String)> = Vec::new();
    for (path, source) in &php_files {
        if to_analyse.len() >= 2000 || source.len() > 1024 * 1024 {
            skipped_paths.insert((*path).clone());
            continue;
        }
        to_analyse.push(((*path).clone(), (*source).clone()));
    }
    let skipped = skipped_paths.len();
    let analyses = converter.analyze_files(&to_analyse).await.ok()?;
    let mut analysed: std::collections::HashMap<String, Option<FileAnalysisJson>> =
        std::collections::HashMap::new();
    for item in analyses.as_array().cloned().unwrap_or_default() {
        let Some(path) = item
            .get("path")
            .and_then(|v| v.as_str())
            .map(str::to_string)
        else {
            continue;
        };
        let analysis = match item.get("analysis") {
            Some(v) if !v.is_null() => serde_json::from_value::<FileAnalysisJson>(v.clone()).ok(),
            _ => None,
        };
        analysed.insert(path, analysis);
    }
    // Per-file verdicts (analyser caps: 2,000 PHP files, 1 MB each).
    let mut convertible = 0usize;
    let mut partial = 0usize;
    let mut blocked = 0usize;
    let mut unparsed = 0usize;
    let mut file_rows: Vec<Value> = Vec::new();
    let mut reasons: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();
    let mut option_defaults: Vec<Value> = Vec::new();
    let mut seen_keys: Vec<String> = Vec::new();
    for (path, _source) in &php_files {
        if skipped_paths.contains(path.as_str()) {
            continue;
        }
        match analysed.get(path.as_str()).and_then(|a| a.as_ref()) {
            Some(analysis) => {
                let verdict = match analysis.verdict.as_str() {
                    "convertible" => {
                        convertible += 1;
                        "convertible"
                    }
                    "blocked" => {
                        blocked += 1;
                        "blocked"
                    }
                    _ => {
                        partial += 1;
                        "partial"
                    }
                };
                for feature in &analysis.unconvertible {
                    let entry = reasons
                        .entry(feature.kind.clone())
                        .or_insert((0, Vec::new()));
                    entry.0 += 1;
                    if entry.1.len() < 5 && !entry.1.contains(path) {
                        entry.1.push((*path).clone());
                    }
                }
                for default in &analysis.option_defaults {
                    if !seen_keys.contains(&default.key) {
                        seen_keys.push(default.key.clone());
                        option_defaults.push(json!({
                            "key": default.key,
                            "defaultValue": default.default_value,
                            "file": default.file,
                        }));
                    }
                }
                file_rows.push(json!({
                    "file": path,
                    "verdict": verdict,
                    "reasons": analysis.reasons,
                }));
            }
            None => {
                unparsed += 1;
                let entry = reasons
                    .entry("parse-error".to_string())
                    .or_insert((0, Vec::new()));
                entry.0 += 1;
                if entry.1.len() < 5 {
                    entry.1.push((*path).clone());
                }
                file_rows.push(json!({
                    "file": path,
                    "verdict": "unparsed",
                    "reasons": ["Failed to parse PHP file."],
                }));
            }
        }
    }
    // Blocked first, then partial, unparsed, convertible; capped 500.
    let rank = |v: &str| match v {
        "blocked" => 0,
        "partial" => 1,
        "unparsed" => 2,
        _ => 3,
    };
    file_rows.sort_by_key(|r| rank(r["verdict"].as_str().unwrap_or("convertible")));
    let files_truncated = file_rows.len() > 500;
    file_rows.truncate(500);
    let mut top_reasons: Vec<(String, usize, Vec<String>)> = reasons
        .into_iter()
        .map(|(kind, (count, files))| (kind, count, files))
        .collect();
    top_reasons.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    top_reasons.truncate(8);
    let top_reasons_json: Vec<Value> = top_reasons
        .into_iter()
        .map(|(kind, count, files)| json!({ "kind": kind, "count": count, "files": files }))
        .collect();
    // Auto-convert: generate TS, migrate literal option defaults,
    // store the generated files in R2 (never executed). The conversion
    // runs in the converter Worker; the generated code and the report
    // come back over the same call, and the main Worker stores them.
    let mut conversion_json = Value::Null;
    if !php_files.is_empty() {
        let owned: Vec<(String, String)> = files.clone();
        let converted = converter
            .convert_files(&owned, &slug, "plugin")
            .await
            .ok()?;
        let generated: Vec<(String, String)> =
            serde_json::from_value(converted.get("generated").cloned().unwrap_or(Value::Null))
                .unwrap_or_default();
        let mut conv_report = converted.get("report").cloned().unwrap_or(Value::Null);
        let defaults: Vec<Value> = converted
            .get("defaults")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if let Ok(migrated) = migrate_option_defaults(ctx.db.as_ref(), &defaults).await {
            conv_report["settings"] = migrated;
        }
        if let Ok(store) = require_store(ctx) {
            for (path, code) in &generated {
                // `path` already carries the `converted/` prefix
                // (convert_php_files keys by `converted/<path>.ts`) —
                // do NOT prepend another one (that double-prefixes the
                // key and the P6 loader then maps it back to a
                // `converted/<path>.php` php path).
                let key = format!("extensions/{slug}/{path}");
                let _ = store
                    .put_object(&key, code.as_bytes().to_vec(), "text/plain; charset=utf-8")
                    .await;
            }
        }
        conversion_json = conv_report;
    }
    let verdict = if php_files.is_empty() {
        "not-analysed"
    } else if blocked > 0 {
        "blocked"
    } else if partial > 0 {
        "partial"
    } else {
        "compatible"
    };
    Some(json!({
        "phpFiles": php_files.len(),
        "totalFiles": total_files,
        "convertible": convertible,
        "partial": partial,
        "blocked": blocked,
        "unparsed": unparsed,
        "skipped": skipped,
        "verdict": verdict,
        "topReasons": top_reasons_json,
        "files": file_rows,
        "filesTruncated": files_truncated,
        "optionDefaults": option_defaults,
        "conversion": conversion_json,
        "analysedAt": now_iso(),
    }))
}
