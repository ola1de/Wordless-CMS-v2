//! SEO route table — the SEO Engine's API + public surface.
//!
//! Moved here verbatim from the main Worker's `routes.rs` in the
//! 2026-11-20 SEO split (only the `Ctx` / helper imports changed,
//! plus the two redirect handlers: their cache purge now crosses
//! the service boundary as an `X-Wordless-Purge` response header
//! — see the comment at those handlers — because the SEO Worker
//! holds no cache binding of its own).
//!
//! Routes (31): the `/api/posts/:id/seo*` + `/api/seo/*` API block
//! (per-post SEO fields, analysis, robots, JSON-LD preview, FAQ,
//! CWV budgets, audit runs, sitemap viewer, templates, calendar,
//! import, redirects CRUD, 404 log) and the public block
//! (`/sitemap.xml`, `/sitemap-index.xml`, `/sitemap-images.xml`,
//! `/sitemap-videos.xml`, `/robots.txt`, `/llms.txt`,
//! `/llms-full.txt`). All routes use the v1 `{ok, data, error}`
//! envelope (API) or raw XML/text (public) and the same
//! auth/capability guards as before the split.
//!
//! Page-head rendering (`build_page_seo`, JSON-LD in `<head>`,
//! breadcrumbs, canonical/meta tags) did NOT move: it runs inside
//! the main Worker's public page templates, in-process, from
//! `wordless-seo-core` — the SEO Worker only serves this route
//! table over the shared D1.

use serde_json::{json, Value};
use wordless_kernel::error::{bad_request, not_found};
use wordless_kernel::router::{read_json_body, RequestContext, RouteOptions, Router};
use wordless_kernel::validate::{int_in_range, require_string};
use wordless_seo::analyze::analyze_post_seo;
use wordless_seo::audit::{
    audit_run_input_from_json, get_audit_run, list_audit_runs, record_audit_run,
};
use wordless_seo::cwv::{get_cwv_budgets, set_cwv_budgets, CwvBudgets};
use wordless_seo::import::import_seo_metadata;
use wordless_seo::llms::{llms_full_txt, llms_txt};
use wordless_seo::redirects::{
    create_redirect, delete_not_found, delete_redirect, list_not_found, list_redirects,
    update_redirect,
};
use wordless_seo::robots::{get_robots_txt, set_robots_txt};
use wordless_seo::sitemap::{
    image_sitemap_xml, sitemap_index_all_xml, sitemap_overview, sitemap_xml, video_sitemap_xml,
};
use wordless_seo::studio::{content_calendar, create_from_template, list_templates};
use wordless_seo_core::head::preview_post_json_ld;
use wordless_seo_core::meta::{
    get_post_faq, get_post_seo, set_post_faq, set_post_seo_from_json, FaqItem,
};
use wordless_seo_core::site::site_url;

use crate::ctx::Ctx;
use crate::helpers::{
    actor, log_action, ok, ok_json, opt_str, page_param, parse_id, per_page_param, raw_text,
};

/// Register every SEO API route on the router built by the SEO Worker entry.
pub fn register_seo_routes(router: &mut Router<Ctx>) {
    // -------------------------------------------------------------------- seo
    router.get(
        "/api/posts/:id/seo",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "post id")?;
                ok(get_post_seo(ctx.db.as_ref(), ctx.site_id, id).await?, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.put(
        "/api/posts/:id/seo",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "post id")?;
                let body = read_json_body(ctx)?;
                let seo = set_post_seo_from_json(ctx.db.as_ref(), ctx.site_id, id, &body).await?;
                log_action(ctx, "seo.update", "post", id).await;
                ok(seo, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.get(
        "/api/posts/:id/seo/analysis",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "post id")?;
                let site_url = site_url(ctx.db.as_ref(), ctx.site_id).await?;
                ok(analyze_post_seo(ctx.db.as_ref(), ctx.site_id, id, &site_url).await?, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.get(
        "/api/seo/robots",
        |ctx| {
            Box::pin(async move {
                let robots = get_robots_txt(ctx.db.as_ref(), ctx.site_id).await?;
                ok_json(json!({ "robots": robots }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.put(
        "/api/seo/robots",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let robots_raw = require_string(&body["robots"], "robots", 1, 10000)?;
                let robots = set_robots_txt(ctx.db.as_ref(), ctx.site_id, &robots_raw).await?;
                log_action(ctx, "seo.robots", "option", 0).await;
                ok_json(json!({ "robots": robots }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/posts/:id/seo/json-ld",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "post id")?;
                let (graph, problems) = preview_post_json_ld(ctx.db.as_ref(), ctx.site_id, id).await?;
                let parsed: Value = serde_json::from_str(&graph).unwrap_or(Value::Null);
                ok_json(
                    json!({ "jsonLd": parsed, "valid": problems.is_empty(), "problems": problems }),
                    200,
                )
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.get(
        "/api/posts/:id/seo/faq",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "post id")?;
                ok(get_post_faq(ctx.db.as_ref(), ctx.site_id, id).await?, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.put(
        "/api/posts/:id/seo/faq",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "post id")?;
                let body = read_json_body(ctx)?;
                // Accept either a bare array or { "faq": [...] }.
                let arr = if body.is_array() {
                    body.as_array().cloned().unwrap_or_default()
                } else {
                    body.get("faq")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default()
                };
                let mut faqs: Vec<FaqItem> = Vec::new();
                for item in &arr {
                    faqs.push(FaqItem {
                        question: item
                            .get("question")
                            .and_then(|q| q.as_str())
                            .unwrap_or("")
                            .to_string(),
                        answer: item
                            .get("answer")
                            .and_then(|a| a.as_str())
                            .unwrap_or("")
                            .to_string(),
                    });
                }
                let saved = set_post_faq(ctx.db.as_ref(), ctx.site_id, id, &faqs).await?;
                log_action(ctx, "seo.faq", "post", id).await;
                ok(saved, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    // P10: Core Web Vitals budgets (stored, admin-editable).
    router.get(
        "/api/seo/cwv-budgets",
        |ctx| Box::pin(async move { ok(get_cwv_budgets(ctx.db.as_ref(), ctx.site_id).await?, 200) }),
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.put(
        "/api/seo/cwv-budgets",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let budgets: CwvBudgets = serde_json::from_value(body).map_err(|_| {
                    bad_request(
                        "Body must be { \"lcpMs\": <int>, \"inpMs\": <int>, \"cls\": <number> }.",
                    )
                })?;
                let saved = set_cwv_budgets(ctx.db.as_ref(), ctx.site_id, budgets).await?;
                log_action(ctx, "seo.cwv_budgets", "option", 0).await;
                ok(saved, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    // P10: audit runs (append-only history of measured scores).
    router.get(
        "/api/seo/audit-runs",
        |ctx| {
            Box::pin(async move {
                let limit = int_in_range(ctx.query("limit"), "limit", 1, 100, 20)?;
                ok(list_audit_runs(ctx.db.as_ref(), ctx.site_id, limit).await?, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.post(
        "/api/seo/audit-runs",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let input = audit_run_input_from_json(&body)?;
                let run = record_audit_run(ctx.db.as_ref(), ctx.site_id, input).await?;
                log_action(ctx, "seo.audit_run", "audit", run.id).await;
                ok(run, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/seo/audit-runs/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "audit run id")?;
                let run = get_audit_run(ctx.db.as_ref(), ctx.site_id, id)
                    .await?
                    .ok_or_else(|| not_found(&format!("Audit run {id} not found.")))?;
                ok(run, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    // P10: sitemap inventory for the admin sitemap viewer.
    router.get(
        "/api/seo/sitemap",
        |ctx| {
            Box::pin(async move {
                let overview = sitemap_overview(ctx.db.as_ref(), ctx.site_id).await?;
                let files: Vec<Value> = overview
                    .into_iter()
                    .map(|(loc, entries)| json!({ "loc": loc, "entries": entries }))
                    .collect();
                ok_json(json!({ "files": files }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/seo/templates",
        |_ctx| Box::pin(async move { ok(list_templates(), 200) }),
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.post(
        "/api/seo/templates/:slug/create",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                // v1 tolerates an empty/invalid body here.
                let body = if ctx.body().is_empty() {
                    Value::Object(Default::default())
                } else {
                    read_json_body(ctx).unwrap_or(Value::Object(Default::default()))
                };
                let title = opt_str(&body, "title");
                let post =
                    create_from_template(ctx.db.as_ref(), ctx.site_id, &slug, actor(ctx), title.as_deref())
                        .await?;
                log_action(ctx, "seo.template_create", "post", post.id).await;
                ok(post, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.get(
        "/api/seo/calendar",
        |ctx| {
            Box::pin(async move {
                let days = int_in_range(ctx.query("days"), "days", 1, 90, 30)?;
                ok(content_calendar(ctx.db.as_ref(), ctx.site_id, days).await?, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/seo/import",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let Some(posts) = body.get("posts").and_then(|v| v.as_array()) else {
                    return Err(bad_request(
                        "Import body must be an object with a \"posts\" array.",
                    ));
                };
                let rows: Vec<wordless_seo::import::SeoImportRow> =
                    serde_json::from_value(Value::Array(posts.clone())).map_err(|_| {
                        bad_request("Import body must be an object with a \"posts\" array.")
                    })?;
                let result = import_seo_metadata(ctx.db.as_ref(), ctx.site_id, &rows).await?;
                log_action(ctx, "seo.import", "option", 0).await;
                ok(result, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/seo/redirects",
        |ctx| {
            Box::pin(async move {
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                ok(
                    list_redirects(ctx.db.as_ref(), ctx.site_id, Some(page), Some(per_page)).await?,
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
        "/api/seo/redirects",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let old_path = require_string(&body["oldPath"], "oldPath", 1, 500)?;
                let new_path = require_string(&body["newPath"], "newPath", 1, 2000)?;
                let redirect = create_redirect(ctx.db.as_ref(), ctx.site_id, &old_path, &new_path).await?;
                log_action(ctx, "seo.redirect_create", "redirect", redirect.id).await;
                // A new redirect can stale a cached 200 at the old
                // path; purge it (the new target is resolved live).
                // The purge crosses the service boundary as a
                // response header: the main Worker's proxy reads
                // `X-Wordless-Purge` back into its own purge list
                // (see routes_proxy.rs `proxy_to_seo`) — the SEO
                // Worker holds no cache binding of its own.
                let purge_url = format!("{}{}", ctx.origin, redirect.old_path);
                let mut resp = ok(redirect, 201)?;
                resp.extra_headers
                    .push(("X-Wordless-Purge".to_string(), purge_url));
                Ok(resp)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.put(
        "/api/seo/redirects/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "redirect id")?;
                let body = read_json_body(ctx)?;
                let new_path = require_string(&body["newPath"], "newPath", 1, 2000)?;
                let redirect = update_redirect(ctx.db.as_ref(), ctx.site_id, id, &new_path).await?;
                log_action(ctx, "seo.redirect_update", "redirect", redirect.id).await;
                // The 301 at the old path is cached; purge it so the
                // new target takes effect immediately.
                // The purge crosses the service boundary as a
                // response header: the main Worker's proxy reads
                // `X-Wordless-Purge` back into its own purge list
                // (see routes_proxy.rs `proxy_to_seo`) — the SEO
                // Worker holds no cache binding of its own.
                let purge_url = format!("{}{}", ctx.origin, redirect.old_path);
                let mut resp = ok(redirect, 200)?;
                resp.extra_headers
                    .push(("X-Wordless-Purge".to_string(), purge_url));
                Ok(resp)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.delete(
        "/api/seo/redirects/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "redirect id")?;
                let deleted = delete_redirect(ctx.db.as_ref(), ctx.site_id, id).await?;
                log_action(ctx, "seo.redirect_delete", "redirect", id).await;
                ok_json(json!({ "deleted": deleted }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/seo/404s",
        |ctx| {
            Box::pin(async move {
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                ok(
                    list_not_found(ctx.db.as_ref(), ctx.site_id, Some(page), Some(per_page)).await?,
                    200,
                )
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.delete(
        "/api/seo/404s",
        |ctx| {
            Box::pin(async move {
                let path = ctx.query("path").unwrap_or("").to_string();
                if path.is_empty() {
                    return Err(bad_request("Delete needs a \"path\" query parameter."));
                }
                let deleted = delete_not_found(ctx.db.as_ref(), ctx.site_id, &path).await?;
                log_action(ctx, "seo.404_delete", "option", 0).await;
                ok_json(json!({ "deleted": deleted }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );
}

/// Register the public SEO routes (sitemaps, robots.txt, llms.txt)
/// on the router built by the SEO Worker entry.
pub fn register_seo_public_routes(router: &mut Router<Ctx>) {
    router.get(
        "/sitemap.xml",
        |ctx| {
            Box::pin(async move {
                let page = match ctx.query("page") {
                    Some(raw) => Some(int_in_range(Some(raw), "page", 1, 100000, 1)?),
                    None => None,
                };
                let xml = sitemap_xml(ctx.db.as_ref(), ctx.site_id, page).await?;
                Ok(raw_text(200, xml, "application/xml; charset=utf-8"))
            })
        },
        RouteOptions::default(),
    );

    router.get(
        "/sitemap-index.xml",
        |ctx| {
            Box::pin(async move {
                let xml = sitemap_index_all_xml(ctx.db.as_ref(), ctx.site_id).await?;
                Ok(raw_text(200, xml, "application/xml; charset=utf-8"))
            })
        },
        RouteOptions::default(),
    );

    router.get(
        "/sitemap-images.xml",
        |ctx| {
            Box::pin(async move {
                let xml = image_sitemap_xml(ctx.db.as_ref(), ctx.site_id).await?;
                Ok(raw_text(200, xml, "application/xml; charset=utf-8"))
            })
        },
        RouteOptions::default(),
    );

    router.get(
        "/sitemap-videos.xml",
        |ctx| {
            Box::pin(async move {
                let xml = video_sitemap_xml(ctx.db.as_ref(), ctx.site_id).await?;
                Ok(raw_text(200, xml, "application/xml; charset=utf-8"))
            })
        },
        RouteOptions::default(),
    );

    router.get(
        "/robots.txt",
        |ctx| {
            Box::pin(async move {
                let txt = get_robots_txt(ctx.db.as_ref(), ctx.site_id).await?;
                Ok(raw_text(200, txt, "text/plain; charset=utf-8"))
            })
        },
        RouteOptions::default(),
    );

    // P10: llms.txt (link list) + llms-full.txt (with full post text).
    router.get(
        "/llms.txt",
        |ctx| {
            Box::pin(async move {
                let txt = llms_txt(ctx.db.as_ref(), ctx.site_id).await?;
                Ok(raw_text(200, txt, "text/plain; charset=utf-8"))
            })
        },
        RouteOptions::default(),
    );

    router.get(
        "/llms-full.txt",
        |ctx| {
            Box::pin(async move {
                let txt = llms_full_txt(ctx.db.as_ref(), ctx.site_id).await?;
                Ok(raw_text(200, txt, "text/plain; charset=utf-8"))
            })
        },
        RouteOptions::default(),
    );
}
