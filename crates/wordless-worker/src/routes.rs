//! The route table (P2: the full v1 contract, §2.5).
//!
//! All 104 P2 routes from `P2-NOTES.md`, plus the P1 routes they
//! include (health, site-health, login, logout, me). Paths, methods,
//! guards, and response shapes match v1 (`src/index.ts`) exactly;
//! service calls live in the `wordless-*` crates and this file only
//! wires them to HTTP.
//!
//! Notes on scope:
//! - P4 adds the CPT/taxonomy REST routes (`/api/post-types`,
//!   `/api/taxonomies`), user profiles (`PUT /api/users/:id/profile`,
//!   `GET /api/users/:id`), forced password change
//!   (`POST /api/auth/change-password`, `mustChangePassword` on login
//!   and `/me`), scheduling (`publishAt` on post create/update, the
//!   lazy publish check on single-post reads), and the Plugins list
//!   (in `routes_p3.rs`). See `wordless-cms::scheduling` for the
//!   scheduling decision (draft + `_wl_publish_at` meta, no `future`
//!   status) and `wordless-auth::users` for the forced-reset design.
//! - `GET /api/posts/:slug` carries `seo` (omitted when the SEO read
//!   fails) and `hasElementorData`.
//! - Media derivatives: the Images binding is NOT declared in
//!   `wrangler.toml`, so variants are never faked — the original is
//!   served with `X-Wordless-Derivatives: unavailable`.
//! - Theme imports carry the M11 auto-convert report in
//!   `"conversion"` (P5 F9: always, even for PHP-free themes).
//! - Public rendering has a CPT-archive renderer in `wordless-render`
//!   (P3 closed the P2 gap).
//! - Cache purge covers API post create/update (v1 limit preserved)
//!   plus redirect creates; handlers push URLs into
//!   `Ctx::purge_urls` and the worker entry deletes them best-effort.

use std::collections::BTreeMap;
use std::rc::Rc;

use serde::Serialize;
use serde_json::{json, Value};
use wordless_auth::{
    build_default_session_cookie, change_password, clear_session_cookie, confirm_totp,
    disable_totp, get_user_profile, is_password_change_required, is_totp_enabled,
    list_user_sessions, login_guarded, parse_session_cookie, revoke_all_sessions_for_user,
    revoke_session, set_password_change_required, setup_totp, update_user_profile, LoginOutcome,
    UpdateProfileInput, PASSWORD_RESET_META_KEY,
};
use wordless_auth::{
    create_user, delete_user, delete_user_meta, list_users, reset_user_password, set_user_role,
    CreateUserInput,
};
use wordless_cms::activity::{list_activity, log as log_activity, ListActivityOptions};
use wordless_cms::comments::{
    create_comment, delete_comment, list_approved_threaded, list_comments, set_comment_status,
    CreateCommentInput, ListCommentsOptions,
};
use wordless_cms::login_branding::get_login_branding;
use wordless_cms::menus::{
    add_menu_item, create_menu, delete_menu, get_menu_locations, list_menu_items, list_menus,
    remove_menu_item, resolve_menu_by_location, set_menu_location, update_menu_item,
    CreateMenuItemInput, UpdateMenuItemInput,
};
use wordless_cms::options::get_option_value;
use wordless_cms::permalinks::{
    get_permalink_structure, post_url as cms_post_url, resolve_path as cms_resolve_path,
    resolve_post_by_id as cms_resolve_post_by_id, term_url as cms_term_url,
    ResolvedPath as CmsResolvedPath,
};
use wordless_cms::post_types::find_post_type;
use wordless_cms::post_types::{
    delete_post_type, get_post_type, list_post_types, register_post_type, update_post_type,
    RegisterPostTypeInput, UpdatePostTypeInput,
};
use wordless_cms::postmeta::update_post_meta;
use wordless_cms::posts::{
    create_post, delete_post, get_featured_image_id, get_post_by_id, get_post_by_slug,
    get_post_by_slug_or_id, list_posts, restore_post, trash_post, update_post, CreatePostInput,
    ListPostsOptions, Post as CmsPost, PostStatus, UpdatePostInput, THUMBNAIL_META_KEY,
};
use wordless_cms::revisions::{
    diff_revisions, get_revision, list_revisions, restore_revision, save_autosave,
};
use wordless_cms::scheduling::{get_publish_at, maybe_publish_due, set_publish_at};
use wordless_cms::search::search_posts as cms_search_posts;
use wordless_cms::search::SearchOptions;
use wordless_cms::settings::{get_settings, update_settings};
use wordless_cms::taxonomies::{
    delete_taxonomy, get_taxonomy, list_taxonomies, register_taxonomy, update_taxonomy,
    RegisterTaxonomyInput, UpdateTaxonomyInput,
};
use wordless_cms::terms::{
    create_term, delete_term, get_post_terms, get_term, list_terms, remove_post_term,
    set_post_terms, update_term, CreateTermInput, ListTermsOptions, Term as CmsTerm,
    UpdateTermInput,
};
use wordless_kernel::capabilities::{user_can, Role};
use wordless_kernel::envelope::RouteResponse;
use wordless_kernel::error::{bad_request, forbidden, not_found, unauthorized, WordlessError};
use wordless_kernel::router::{read_json_body, RequestContext, RouteOptions, Router};
use wordless_kernel::storage::ObjectStore;
use wordless_kernel::time::{ms_from_iso, now_iso, now_ms};
use wordless_kernel::validate::{int_in_range, require_string};
use wordless_media::files::{
    delete_file, download_file, list_files, rename_file, upload_file, UploadFileInput,
};
use wordless_media::multipart::parse_multipart;
use wordless_media::service::{
    delete_media, derivative_status, find_media_by_key, get_media_record, list_media,
    resolve_media_request, serve_stored_object, update_media_alt, upload_media, ListMediaOpts,
    MediaResolution, UploadMediaInput,
};
use wordless_render::cache::urls_to_purge_for_post;
use wordless_render::feeds::rss_feed;
use wordless_render::pages::{
    has_elementor_data, render_date_archive, render_home, render_not_found, render_page,
    render_post_type_archive, render_search_page, render_single, render_term_archive, PageSeoHead,
    Post as RenderPost, RenderOptions, RenderedPage, Term as RenderTerm, ThemeView,
};
// SEO split (2026-11-20): only the page-head rendering stays
// in-process (public page templates need it); the SEO API +
// public sitemap/robots/llms routes moved to the SEO Worker and
// are proxied from routes_proxy.rs. The post-detail payload still
// reads its SEO fields in-process (M7) via the core crate.
use wordless_seo_core::head::{build_page_seo, Breadcrumb, PageKind, PageSeoContext, ProductSeo};
use wordless_seo_core::meta::get_post_seo;
use wordless_themes::customizer::{
    create_preview, delete_variation, get_variation, resolve_variation, save_variation,
};
use wordless_themes::registry::{
    activate_theme, delete_theme, ensure_default_theme, get_active_stylesheet,
    get_active_theme_slug, get_theme, get_theme_detail, import_classic_theme, import_theme,
    load_active_theme,
};

use crate::context::Ctx;
use crate::health::site_health;
use crate::routes_p3::{dispatch_webhook, post_webhook_payload};

/// v2 development version, reported by `/api/health`.
pub const VERSION: &str = "2.0.0-dev";

/// A route handler's result: a platform-neutral response or a typed error
/// (the router turns the error into the `{ok:false}` envelope).
type HandlerResult = Result<RouteResponse, WordlessError>;

// ---------------------------------------------------------------- helpers

fn to_value<T: Serialize>(v: T) -> Result<Value, WordlessError> {
    serde_json::to_value(v).map_err(|e| WordlessError::new("internal_error", 500, &e.to_string()))
}

pub(crate) fn ok<T: Serialize>(v: T, status: u16) -> HandlerResult {
    Ok(RouteResponse::json_ok(to_value(v)?, status))
}

pub(crate) fn ok_json(v: Value, status: u16) -> HandlerResult {
    Ok(RouteResponse::json_ok(v, status))
}

/// The acting user id, or 0 for anonymous/system (v1 `user?.id ?? 0`).
pub(crate) fn actor(ctx: &Ctx) -> i64 {
    ctx.user().map(|u| u.id).unwrap_or(0)
}

/// v1 `parseIdParam`: an integer >= 1, else `Invalid <field>.`
pub(crate) fn parse_id(ctx: &Ctx, param: &str, field: &str) -> Result<i64, WordlessError> {
    let raw = ctx.param(param).unwrap_or("");
    let n: i64 = raw.parse().unwrap_or(0);
    if n < 1 {
        return Err(bad_request(&format!("Invalid {field}.")));
    }
    Ok(n)
}

pub(crate) fn page_param(ctx: &Ctx) -> Result<i64, WordlessError> {
    int_in_range(ctx.query("page"), "page", 1, 10000, 1)
}

pub(crate) fn per_page_param(ctx: &Ctx) -> Result<i64, WordlessError> {
    int_in_range(ctx.query("per_page"), "per_page", 1, 100, 20)
}

pub(crate) fn opt_str(body: &Value, field: &str) -> Option<String> {
    body.get(field)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

pub(crate) fn opt_i64(body: &Value, field: &str) -> Option<i64> {
    body.get(field).and_then(|v| v.as_i64())
}

pub(crate) fn body_str(body: &Value, field: &str) -> String {
    opt_str(body, field).unwrap_or_default()
}

pub(crate) fn opt_bool(body: &Value, field: &str) -> Option<bool> {
    body.get(field).and_then(|v| v.as_bool())
}

pub(crate) fn opt_str_array(body: &Value, field: &str) -> Option<Vec<String>> {
    body.get(field).and_then(|v| v.as_array()).map(|a| {
        a.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect()
    })
}

/// Scheduling (`publishAt`) on a post create/update body.
///
/// Returns `Ok(None)` when the body has no `publishAt` field (no
/// change), `Ok(Some(None))` when it is JSON null (clear any pending
/// schedule), and `Ok(Some(Some(iso)))` when a time was supplied
/// (validated ISO-8601; a non-string / malformed value is a 400).
/// The caller applies it with `set_publish_at` after the post row
/// exists. A future time forces the post to `draft` first — see
/// `future_publish_at` below.
pub(crate) fn body_publish_at(body: &Value) -> Result<Option<Option<String>>, WordlessError> {
    match body.get("publishAt") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(raw)) => {
            let _ = ms_from_iso(raw.trim()).ok_or_else(|| {
                bad_request(
                    "Field \"publishAt\" must be an ISO-8601 UTC time, e.g. \"2026-12-01T09:00:00.000Z\".",
                )
            })?;
            Ok(Some(Some(raw.clone())))
        }
        Some(_) => Err(bad_request(
            "Field \"publishAt\" must be an ISO-8601 UTC time string, or null to clear the schedule.",
        )),
    }
}

/// True when the body's `publishAt` is a time in the future: the
/// post must be created/updated as a draft, not published, so no
/// `post.published` webhook fires for a post that is not public yet.
pub(crate) fn future_publish_at(body: &Value, now: u64) -> bool {
    match body.get("publishAt").and_then(|v| v.as_str()) {
        Some(raw) => ms_from_iso(raw.trim()).map(|ms| ms > now).unwrap_or(false),
        None => false,
    }
}

/// Apply a `publishAt` instruction to an existing post and return
/// the pending schedule (the ISO time, or `None` when nothing is
/// pending). No-op when the body carries no `publishAt` field.
pub(crate) async fn apply_publish_at(
    ctx: &Ctx,
    post_id: i64,
    body: &Value,
) -> Result<Option<String>, WordlessError> {
    let Some(instruction) = body_publish_at(body)? else {
        return Ok(get_publish_at(ctx.db.as_ref(), ctx.site_id, post_id).await?);
    };
    set_publish_at(ctx.db.as_ref(), ctx.site_id, post_id, instruction.as_deref(), now_ms()).await
}

/// Attach the pending `publishAt` (when one exists) to a post JSON
/// value, so the editor can show the schedule without a second call.
pub(crate) async fn attach_publish_at(ctx: &Ctx, mut value: Value, post_id: i64) -> Value {
    if let Ok(Some(iso)) = get_publish_at(ctx.db.as_ref(), ctx.site_id, post_id).await {
        value["publishAt"] = Value::String(iso);
    }
    value
}

/// `featuredImageId` on a post create/update body — tri-state like
/// `publishAt`: absent = no change, null = clear the image, a number
/// = set it. A non-integer value is a 400 (caught here, not silently
/// ignored the way `opt_i64` would). The attachment itself is
/// validated in the service (`validate_featured_image_id`).
pub(crate) fn body_featured_image_id(body: &Value) -> Result<Option<Option<i64>>, WordlessError> {
    match body.get("featuredImageId") {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::Number(n)) => match n.as_i64() {
            Some(id) => Ok(Some(Some(id))),
            None => Err(bad_request(
                "Field \"featuredImageId\" must be an integer media attachment id, or null to clear the featured image.",
            )),
        },
        Some(_) => Err(bad_request(
            "Field \"featuredImageId\" must be an integer media attachment id, or null to clear the featured image.",
        )),
    }
}

/// Attach the featured-image id (when one is set) to a post JSON
/// value, so the editor picker can show the current image without a
/// second call. Absent when no `_thumbnail_id` meta is stored.
pub(crate) async fn attach_featured_image(ctx: &Ctx, mut value: Value, post_id: i64) -> Value {
    if let Ok(Some(fid)) = get_featured_image_id(ctx.db.as_ref(), ctx.site_id, post_id).await {
        value["featuredImageId"] = Value::from(fid);
    }
    value
}

/// R2-backed storage, or a 500 when the MEDIA binding is absent.
pub(crate) fn require_store(ctx: &Ctx) -> Result<Rc<dyn ObjectStore>, WordlessError> {
    ctx.media.clone().ok_or_else(|| {
        WordlessError::new("internal_error", 500, "Media storage is not configured.")
    })
}

/// Best-effort activity log (v1 `log()` swallows errors; never breaks
/// the response).
pub(crate) async fn log_action(ctx: &Ctx, action: &str, object_type: &str, object_id: i64) {
    let _ = log_activity(ctx.db.as_ref(), actor(ctx), action, object_type, object_id).await;
}

/// After a successful post create/update, queue the v1 purge list
/// (own URL, home, affected term pages) — publish only.
pub(crate) async fn purge_post(ctx: &mut Ctx, post: &CmsPost) {
    if post.status != PostStatus::Publish {
        return;
    }
    let terms = get_post_terms(ctx.db.as_ref(), ctx.site_id, post.id, None)
        .await
        .unwrap_or_default();
    let slugs: Vec<&str> = terms.iter().map(|t| t.slug.as_str()).collect();
    let urls = urls_to_purge_for_post(&ctx.origin, &post.slug, &slugs, false);
    ctx.purge_urls.extend(urls);
}

/// A raw (non-JSON) response: HTML, XML, CSS, media bytes, …
///
/// `RouteResponse`'s fields are public, so the worker crate can build
/// one directly instead of forcing these bodies through the JSON
/// envelope.
fn raw_response(status: u16, body: Vec<u8>, content_type: &str) -> RouteResponse {
    RouteResponse {
        status,
        body,
        content_type: content_type.to_string(),
        extra_headers: Vec::new(),
    }
}

fn raw_text(status: u16, body: String, content_type: &str) -> RouteResponse {
    raw_response(status, body.into_bytes(), content_type)
}

pub(crate) fn html_page(page: RenderedPage) -> RouteResponse {
    RouteResponse::html(page.html, page.status)
}

/// 301 with an empty body (v1 `new Response(null, {status: 301,
/// headers: {Location}})`).
pub(crate) fn redirect_response(target: &str) -> RouteResponse {
    RouteResponse::empty(301).with_header("Location", target)
}

/// 303 with an empty body (v1 comment-form success redirect).
pub(crate) fn see_other_response(target: &str) -> RouteResponse {
    RouteResponse::empty(303).with_header("Location", target)
}

/// Escape text for the plain HTML error page used by the public
/// comment form (v1 `handlePublicCommentPost`).
fn escape_html_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ------------------------------------------------------- cms -> render types

/// `wordless-cms` and `wordless-render` carry their own `Post`/`Term`
/// shapes (different serde renames), so public rendering converts
/// field by field.
fn to_render_post(p: &CmsPost) -> RenderPost {
    RenderPost {
        id: p.id,
        author_id: p.author_id,
        date: p.date.clone(),
        date_gmt: p.date_gmt.clone(),
        content: p.content.clone(),
        title: p.title.clone(),
        excerpt: p.excerpt.clone(),
        status: p.status.as_str().to_string(),
        comment_status: p.comment_status.clone(),
        ping_status: p.ping_status.clone(),
        password: p.password.clone(),
        slug: p.slug.clone(),
        modified: p.modified.clone(),
        modified_gmt: p.modified_gmt.clone(),
        content_filtered: p.content_filtered.clone(),
        parent_id: p.parent_id,
        guid: p.guid.clone(),
        menu_order: p.menu_order,
        post_type: p.post_type.clone(),
        mime_type: p.mime_type.clone(),
        comment_count: p.comment_count,
    }
}

fn to_render_term(t: &CmsTerm) -> RenderTerm {
    RenderTerm {
        id: t.id,
        term_taxonomy_id: t.term_taxonomy_id,
        name: t.name.clone(),
        slug: t.slug.clone(),
        taxonomy: t.taxonomy.clone(),
        description: t.description.clone(),
        parent_id: t.parent_id,
        count: t.count,
    }
}

// ------------------------------------------------------------ render options

/// Build `RenderOptions` for a public request: variation overrides
/// (preview token wins, else the active theme's saved variation) and
/// the active theme, best-effort — any failure falls back to the M2
/// layout with inline CSS (same as v1 when no theme loads).
pub(crate) async fn render_opts(ctx: &Ctx) -> RenderOptions {
    let preview_token = ctx.query("wl_preview").map(|s| s.to_string());
    let variation = resolve_variation(ctx.db.as_ref(), ctx.site_id, preview_token.as_deref())
        .await
        .ok()
        .flatten();
    let theme =
        match ctx.media.clone() {
            Some(store) => {
                let loaded =
                    match load_active_theme(ctx.db.as_ref(), ctx.site_id, store.as_ref(), variation.as_ref())
                        .await
                    {
                        Ok(l) => Some(l),
                        Err(_) => {
                            // The default theme row may not exist yet (fresh
                            // install): create it once and retry, then give up
                            // to the M2 layout.
                            let _ = ensure_default_theme(ctx.db.as_ref()).await;
                            load_active_theme(ctx.db.as_ref(), ctx.site_id, store.as_ref(), variation.as_ref())
                                .await
                                .ok()
                        }
                    };
                loaded.map(|l| ThemeView {
                    kind: l.record.kind.clone(),
                    files: l.files.into_iter().collect::<BTreeMap<String, String>>(),
                    stylesheet_hash: l.stylesheet_hash.clone(),
                })
            }
            None => None,
        };
    RenderOptions {
        theme,
        seo: None,
        preview_token,
        // Split (2026-11-20): Elementor rendering goes through the
        // converter Worker.
        converter: ctx.converter.clone(),
        // Size remedy (2026-11-20): the storefront reaches the Woo
        // Worker the same way.
        woo_client: ctx.woo_client.clone(),
    }
}

/// Attach a precomputed SEO head to `opts` (best-effort: on any error
/// the renderer falls back to plain title/description).
pub(crate) async fn with_seo(
    ctx: &Ctx,
    mut opts: RenderOptions,
    kind: PageKind,
    title: String,
    description: String,
    canonical_path: Option<String>,
    post: Option<CmsPost>,
    breadcrumbs: Vec<Breadcrumb>,
    product: Option<ProductSeo>,
) -> RenderOptions {
    let seo_ctx = PageSeoContext {
        kind,
        title,
        description,
        canonical_path,
        post,
        breadcrumbs,
        product,
    };
    if let Ok(seo) = build_page_seo(ctx.db.as_ref(), ctx.site_id, seo_ctx).await {
        opts.seo = Some(PageSeoHead {
            title: seo.title,
            description: seo.description,
            head_tags: seo.head_tags,
            breadcrumbs_html: seo.breadcrumbs_html,
        });
    }
    opts
}

/// Render one post/page through the public single/page renderers.
async fn render_post_page(ctx: &Ctx, post: &CmsPost) -> HandlerResult {
    let opts = render_opts(ctx).await;
    let canonical = cms_post_url(ctx.db.as_ref(), ctx.site_id, post).await.ok();
    let kind = if post.post_type == "page" {
        PageKind::Page
    } else {
        PageKind::Single
    };
    // P5 G2: v1's finishPage adds a breadcrumb [{post title, post URL}]
    // only when kind is "single" — pages (kind "page") carry NO
    // BreadcrumbList. The run-4 public-page diff proved the point.
    let crumbs = if kind == PageKind::Single {
        match &canonical {
            Some(path) => vec![Breadcrumb {
                name: if post.title.is_empty() {
                    "(untitled)".to_string()
                } else {
                    post.title.clone()
                },
                path: path.clone(),
            }],
            None => Vec::new(),
        }
    } else {
        Vec::new()
    };
    let opts = with_seo(
        ctx,
        opts,
        kind,
        if post.title.is_empty() {
            "(untitled)".to_string()
        } else {
            post.title.clone()
        },
        post.excerpt.clone(),
        canonical,
        Some(post.clone()),
        crumbs,
        None,
    )
    .await;
    let rendered = if post.post_type == "page" {
        render_page(ctx.db.as_ref(), ctx.site_id, &to_render_post(post), &opts).await?
    } else {
        render_single(ctx.db.as_ref(), ctx.site_id, &to_render_post(post), &opts).await?
    };
    Ok(html_page(rendered))
}

async fn render_home_page(ctx: &Ctx, page: i64) -> HandlerResult {
    let opts = render_opts(ctx).await;
    let tagline = get_settings(ctx.db.as_ref(), ctx.site_id)
        .await
        .map(|s| s.site_tagline)
        .unwrap_or_default();
    let opts = with_seo(
        ctx,
        opts,
        PageKind::Home,
        "Home".to_string(),
        tagline,
        Some("/".to_string()),
        None,
        Vec::new(),
        None,
    )
    .await;
    Ok(html_page(render_home(ctx.db.as_ref(), ctx.site_id, page, &opts).await?))
}

pub(crate) async fn render_not_found_page(ctx: &Ctx) -> HandlerResult {
    let opts = render_opts(ctx).await;
    let opts = with_seo(
        ctx,
        opts,
        PageKind::NotFound,
        "Not found".to_string(),
        String::new(),
        None,
        None,
        Vec::new(),
        None,
    )
    .await;
    Ok(html_page(render_not_found(ctx.db.as_ref(), ctx.site_id, &opts).await?))
}

/// The public GET fallback: `?p=<id>` first, else the permalink
/// resolver (v1 `handlePublicGet`). Returns the JSON 404 envelope
/// only when the caller should keep it (the worker entry replaces a
/// router 404 with this result).
async fn public_get(ctx: &Ctx) -> HandlerResult {
    let page = page_param(ctx)?;
    if let Some(p) = ctx.query("p").map(|s| s.to_string()) {
        if let Ok(id) = p.parse::<i64>() {
            if let Some(post) = cms_resolve_post_by_id(ctx.db.as_ref(), ctx.site_id, id).await? {
                return render_post_page(ctx, &post).await;
            }
        }
        return render_not_found_page(ctx).await;
    }
    match cms_resolve_path(ctx.db.as_ref(), ctx.site_id, &ctx.path).await? {
        CmsResolvedPath::Redirect { target } => Ok(redirect_response(&target)),
        CmsResolvedPath::Home => render_home_page(ctx, page).await,
        CmsResolvedPath::Post { post } => render_post_page(ctx, &post).await,
        CmsResolvedPath::Page { post } => render_post_page(ctx, &post).await,
        CmsResolvedPath::Term { term } => {
            let opts = render_opts(ctx).await;
            let kind_label = match term.taxonomy.as_str() {
                "category" => "Category".to_string(),
                "tag" => "Tag".to_string(),
                other => other.to_string(),
            };
            // P5 G2: v1 archives carry [{archive title, archive URL}].
            let archive_title = format!("{kind_label}: {}", term.name);
            let archive_url = cms_term_url(&term.taxonomy, &term.slug);
            let opts = with_seo(
                ctx,
                opts,
                PageKind::Archive,
                archive_title.clone(),
                term.description.clone(),
                Some(archive_url.clone()),
                None,
                vec![Breadcrumb {
                    name: archive_title,
                    path: archive_url,
                }],
                None,
            )
            .await;
            Ok(html_page(
                render_term_archive(ctx.db.as_ref(), ctx.site_id, &to_render_term(&term), page, &opts).await?,
            ))
        }
        CmsResolvedPath::DateArchive { year, month } => {
            let opts = render_opts(ctx).await;
            let title = match month {
                Some(m) => format!("Archive: {year}-{m:02}"),
                None => format!("Archive: {year}"),
            };
            let canonical = match month {
                Some(m) => format!("/{year}/{m:02}"),
                None => format!("/{year}"),
            };
            let opts = with_seo(
                ctx,
                opts,
                PageKind::Archive,
                title.clone(),
                String::new(),
                Some(canonical.clone()),
                None,
                vec![Breadcrumb {
                    name: title,
                    path: canonical,
                }],
                None,
            )
            .await;
            Ok(html_page(
                render_date_archive(ctx.db.as_ref(), ctx.site_id, year, month, page, &opts).await?,
            ))
        }
        // CPT archive (P2 gap, closed in P3 wiring): the resolver
        // already confirmed the type exists and has an archive.
        CmsResolvedPath::PostTypeArchive { post_type } => {
            let Some(def) = find_post_type(ctx.db.as_ref(), ctx.site_id, &post_type).await? else {
                return render_not_found_page(ctx).await;
            };
            let opts = render_opts(ctx).await;
            let opts = with_seo(
                ctx,
                opts,
                PageKind::Archive,
                def.label.clone(),
                def.description.clone(),
                Some(format!("/{}", def.rewrite_slug)),
                None,
                vec![Breadcrumb {
                    name: def.label.clone(),
                    path: format!("/{}", def.rewrite_slug),
                }],
                None,
            )
            .await;
            Ok(html_page(
                render_post_type_archive(
                    ctx.db.as_ref(), ctx.site_id,
                    &def.slug,
                    &def.label,
                    &def.rewrite_slug,
                    page,
                    &opts,
                )
                .await?,
            ))
        }
        CmsResolvedPath::NotFound => render_not_found_page(ctx).await,
    }
}

/// Public comment submission from the rendered form:
/// `POST /{slug}/comments` (v1 `handlePublicCommentPost`). Form
/// bodies (urlencoded/multipart) or JSON; success is a 303 back to
/// the post, failure a 400 HTML page — never a JSON envelope.
async fn public_comment_post(ctx: &mut Ctx) -> RouteResponse {
    let path = ctx.path.trim_matches('/').to_string();
    let segments: Vec<&str> = path.split('/').collect();
    let slug = if segments.len() == 2 && segments[1] == "comments" {
        segments[0].to_string()
    } else {
        return match render_not_found_page(ctx).await {
            Ok(r) => r,
            Err(e) => RouteResponse::json_error(&e.code, e.status, &e.message),
        };
    };
    let post = match get_post_by_slug(ctx.db.as_ref(), ctx.site_id, &slug).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return match render_not_found_page(ctx).await {
                Ok(r) => r,
                Err(e) => RouteResponse::json_error(&e.code, e.status, &e.message),
            }
        }
        Err(e) => return RouteResponse::json_error(&e.code, e.status, &e.message),
    };
    let content_type = ctx.header("content-type").unwrap_or("").to_string();
    let (author, author_email, author_url, content, parent_id, honeypot) = if content_type
        .contains("multipart/form-data")
        || content_type.contains("application/x-www-form-urlencoded")
    {
        match parse_multipart(ctx.body(), &content_type) {
                Ok(form) => (
                    form.field("author").unwrap_or("").to_string(),
                    form.field("author_email").unwrap_or("").to_string(),
                    form.field("author_url").unwrap_or("").to_string(),
                    form.field("content").unwrap_or("").to_string(),
                    form.field("parent_id").and_then(|v| v.parse::<i64>().ok()).unwrap_or(0),
                    form.field("website").unwrap_or("").to_string(),
                ),
                Err(e) => {
                    return raw_text(
                        400,
                        format!(
                            "<!DOCTYPE html><html><head><title>Comment error</title></head><body><h1>Could not save your comment</h1><p>{}</p><p><a href=\"/\">Go home</a></p></body></html>",
                            escape_html_text(&e.message)
                        ),
                        "text/html; charset=utf-8",
                    )
                }
            }
    } else {
        match read_json_body(ctx) {
                Ok(body) => (
                    body_str(&body, "author"),
                    body_str(&body, "author_email"),
                    body_str(&body, "author_url"),
                    body_str(&body, "content"),
                    opt_i64(&body, "parentId").unwrap_or(0),
                    body_str(&body, "website"),
                ),
                Err(e) => {
                    return raw_text(
                        400,
                        format!(
                            "<!DOCTYPE html><html><head><title>Comment error</title></head><body><h1>Could not save your comment</h1><p>{}</p><p><a href=\"/\">Go home</a></p></body></html>",
                            escape_html_text(&e.message)
                        ),
                        "text/html; charset=utf-8",
                    )
                }
            }
    };
    let can_moderate = ctx
        .user()
        .map(|u| user_can(u, "moderate_comments"))
        .unwrap_or(false);
    let input = CreateCommentInput {
        post_id: post.id,
        author,
        author_email: Some(author_email),
        author_url: Some(author_url),
        content,
        parent_id: if parent_id > 0 { Some(parent_id) } else { None },
        user_id: Some(actor(ctx)),
        author_ip: Some(ctx.header("cf-connecting-ip").unwrap_or("").to_string()),
        honeypot: Some(honeypot),
    };
    match create_comment(ctx.db.as_ref(), ctx.site_id, input, can_moderate).await {
        Ok(comment) => {
            log_action(ctx, "comment.create", "comment", comment.id).await;
            dispatch_webhook(
                ctx,
                "comment.created",
                serde_json::json!({
                    "id": comment.id,
                    "postId": comment.post_id,
                    "author": comment.author,
                }),
            )
            .await;
            match cms_post_url(ctx.db.as_ref(), ctx.site_id, &post).await {
                Ok(link) => see_other_response(&format!("{link}#comments")),
                Err(e) => RouteResponse::json_error(&e.code, e.status, &e.message),
            }
        }
        Err(e) => raw_text(
            400,
            format!(
                "<!DOCTYPE html><html><head><title>Comment error</title></head><body><h1>Could not save your comment</h1><p>{}</p><p><a href=\"/\">Go home</a></p></body></html>",
                escape_html_text(&e.message)
            ),
            "text/html; charset=utf-8",
        ),
    }
}

/// Public fallback for paths the router did not match (non-API,
/// non-admin). Called by the worker entry after a router 404.
pub async fn handle_public_fallback(ctx: &mut Ctx) -> RouteResponse {
    if ctx.method == "GET" {
        return match public_get(ctx).await {
            Ok(r) => r,
            Err(e) => RouteResponse::json_error(&e.code, e.status, &e.message),
        };
    }
    if ctx.method == "POST" && ctx.path.ends_with("/comments") {
        return public_comment_post(ctx).await;
    }
    RouteResponse::json_error("not_found", 404, "Route not found.")
}

pub fn build_router() -> Router<Ctx> {
    let mut router: Router<Ctx> = Router::new();

    // ---------------------------------------------------------------- health
    router.get(
        "/api/health",
        |_ctx| {
            Box::pin(async move {
                Ok(RouteResponse::json_ok(
                    json!({ "status": "ok", "version": VERSION, "time": now_iso() }),
                    200,
                ))
            })
        },
        RouteOptions::default(),
    );

    router.get(
        "/api/site-health",
        |ctx| {
            Box::pin(async move {
                let media = ctx.media.clone();
                let health =
                    site_health(ctx.db.as_ref(), media.as_deref(), &ctx.bindings, VERSION).await;
                let value = serde_json::to_value(&health)
                    .map_err(|e| WordlessError::new("internal_error", 500, &e.to_string()))?;
                Ok(RouteResponse::json_ok(value, 200))
            })
        },
        RouteOptions::default(),
    );

    router.get(
        "/api/activity",
        |ctx| {
            Box::pin(async move {
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                ok(
                    list_activity(
                        ctx.db.as_ref(),
                        ListActivityOptions {
                            page: Some(page),
                            per_page: Some(per_page),
                        },
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

    // ------------------------------------------------------------------ posts
    router.get(
        "/api/posts",
        |ctx| {
            Box::pin(async move {
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                let status = ctx.query("status").map(|s| s.to_string());
                let post_type = ctx.query("type").map(|s| s.to_string());
                ok(
                    list_posts(
                        ctx.db.as_ref(), ctx.site_id,
                        ListPostsOptions {
                            status,
                            post_type,
                            author_id: None,
                            page: Some(page),
                            per_page: Some(per_page),
                            include_trash: false,
                        },
                    )
                    .await?,
                    200,
                )
            })
        },
        RouteOptions::default(),
    );

    router.post(
        "/api/posts",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                // Validate `publishAt` before creating anything; a
                // future time stores the post as a draft (the schedule
                // wins over a `status: "publish"` in the same request).
                let _ = body_publish_at(&body)?;
                let status = if future_publish_at(&body, now_ms()) {
                    Some("draft".to_string())
                } else {
                    opt_str(&body, "status")
                };
                let mut post = create_post(
                    ctx.db.as_ref(), ctx.site_id,
                    CreatePostInput {
                        title: body_str(&body, "title"),
                        content: opt_str(&body, "content"),
                        excerpt: opt_str(&body, "excerpt"),
                        slug: opt_str(&body, "slug"),
                        status,
                        post_type: opt_str(&body, "type"),
                        author_id: actor(ctx),
                        parent_id: None,
                        mime_type: None,
                        // P4: null and absent both mean "no image" on
                        // create; a number is validated in the service.
                        featured_image_id: body_featured_image_id(&body)?.flatten(),
                    },
                )
                .await?;
                let id = post.id;
                apply_publish_at(ctx, id, &body).await?;
                // Re-read: a past `publishAt` may have published the
                // post, a future one forced it to draft — webhooks and
                // the purge list must see the final state, not the
                // pre-schedule one.
                if let Some(fresh) = get_post_by_id(ctx.db.as_ref(), ctx.site_id, id).await? {
                    post = fresh;
                }
                log_action(ctx, "post.create", "post", id).await;
                purge_post(ctx, &post).await;
                dispatch_webhook(ctx, "post.created", post_webhook_payload(&post)).await;
                if post.status == PostStatus::Publish {
                    dispatch_webhook(ctx, "post.published", post_webhook_payload(&post)).await;
                }
                let value = attach_publish_at(ctx, to_value(&post)?, id).await;
                let value = attach_featured_image(ctx, value, id).await;
                ok_json(value, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.put(
        "/api/posts/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "post id")?;
                let body = read_json_body(ctx)?;
                let _ = body_publish_at(&body)?;
                let was_published = get_post_by_id(ctx.db.as_ref(), ctx.site_id, id)
                    .await?
                    .map(|p| p.status == PostStatus::Publish)
                    .unwrap_or(false);
                // A future `publishAt` forces the update to draft, so
                // a scheduled post is never published early by the
                // status in the same request.
                let status = if future_publish_at(&body, now_ms()) {
                    Some("draft".to_string())
                } else {
                    opt_str(&body, "status")
                };
                let mut post = update_post(
                    ctx.db.as_ref(), ctx.site_id,
                    id,
                    UpdatePostInput {
                        title: opt_str(&body, "title"),
                        content: opt_str(&body, "content"),
                        excerpt: opt_str(&body, "excerpt"),
                        slug: opt_str(&body, "slug"),
                        status,
                        post_type: opt_str(&body, "type"),
                        parent_id: opt_i64(&body, "parentId"),
                        featured_image_id: body_featured_image_id(&body)?,
                    },
                )
                .await?;
                if body.get("publishAt").is_some() {
                    apply_publish_at(ctx, id, &body).await?;
                } else if post.status == PostStatus::Publish {
                    // A manual publish with no schedule clears a stale
                    // pending schedule (publishing early is deliberate).
                    let _ = wordless_cms::scheduling::delete_schedule(ctx.db.as_ref(), ctx.site_id, id).await;
                }
                if let Some(fresh) = get_post_by_id(ctx.db.as_ref(), ctx.site_id, id).await? {
                    post = fresh;
                }
                log_action(ctx, "post.update", "post", post.id).await;
                purge_post(ctx, &post).await;
                dispatch_webhook(ctx, "post.updated", post_webhook_payload(&post)).await;
                if post.status == PostStatus::Publish && !was_published {
                    dispatch_webhook(ctx, "post.published", post_webhook_payload(&post)).await;
                }
                let value = attach_publish_at(ctx, to_value(&post)?, id).await;
                let value = attach_featured_image(ctx, value, id).await;
                ok_json(value, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    // P4: generic postmeta set/delete for the editor (the featured
    // image picker and any other meta-backed field). Body is a flat
    // object of key -> value; `null` deletes the key, strings/numbers/
    // booleans are stored as their string form. `_wl_publish_at` is
    // rejected here (use `publishAt`); `_thumbnail_id` is validated
    // as an attachment id, like `featuredImageId`. Returns the post's
    // full meta map. Guarded like post update (`edit_posts`).
    router.put(
        "/api/posts/:id/meta",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "post id")?;
                let body = read_json_body(ctx)?;
                let obj = body.as_object().ok_or_else(|| {
                    bad_request(
                        "PUT /api/posts/:id/meta expects a JSON object of meta key to value (null deletes the key).",
                    )
                })?;
                let mut entries: Vec<(String, Option<String>)> = Vec::new();
                let mut thumbnail_changed = false;
                for (key, value) in obj {
                    let parsed = match value {
                        Value::Null => None,
                        Value::String(s) => Some(s.clone()),
                        Value::Number(n) => Some(n.to_string()),
                        Value::Bool(b) => Some(b.to_string()),
                        _ => {
                            return Err(bad_request(&format!(
                                "Meta value for \"{key}\" must be a string, number, boolean, or null."
                            )));
                        }
                    };
                    if key == THUMBNAIL_META_KEY && parsed.is_some() {
                        thumbnail_changed = true;
                    }
                    entries.push((key.clone(), parsed));
                }
                let meta = update_post_meta(ctx.db.as_ref(), ctx.site_id, id, &entries).await?;
                if thumbnail_changed {
                    // The featured image is visible on the rendered
                    // post, so a published post's cache is purged.
                    if let Some(post) = get_post_by_id(ctx.db.as_ref(), ctx.site_id, id).await? {
                        purge_post(ctx, &post).await;
                    }
                }
                log_action(ctx, "post.meta", "post", id).await;
                ok(meta, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.post(
        "/api/posts/:id/trash",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "post id")?;
                let post = trash_post(ctx.db.as_ref(), ctx.site_id, id).await?;
                log_action(ctx, "post.trash", "post", post.id).await;
                ok(post, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("delete_posts"),
        },
    );

    router.post(
        "/api/posts/:id/restore",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "post id")?;
                let post = restore_post(ctx.db.as_ref(), ctx.site_id, id).await?;
                log_action(ctx, "post.restore", "post", post.id).await;
                ok(post, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.delete(
        "/api/posts/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "post id")?;
                let deleted = delete_post(ctx.db.as_ref(), ctx.site_id, id).await?;
                log_action(ctx, "post.delete", "post", id).await;
                ok_json(json!({ "deleted": deleted }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("delete_posts"),
        },
    );

    router.get(
        "/api/posts/:slug",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                // Numeric "slugs" are post IDs (the editor loads
                // /api/posts/<id>); trashed posts are never returned.
                let post = get_post_by_slug_or_id(ctx.db.as_ref(), ctx.site_id, &slug).await?;
                let Some(post) = post else {
                    return Err(not_found(&format!("No post found with slug \"{slug}\".")));
                };
                // Lazy publish: a draft whose schedule is due flips
                // the moment it is read (the local-dev half of the
                // scheduling mechanism — the cron sweep is the other).
                let post = maybe_publish_due(ctx.db.as_ref(), post, now_ms()).await?;
                // Drafts and private posts are visible only to
                // logged-in users.
                if post.status != PostStatus::Publish && ctx.user().is_none() {
                    return Err(unauthorized("Authentication required to view this post."));
                }
                let mut value = to_value(&post)?;
                // M7: the post detail payload carries its SEO fields;
                // a failed SEO read omits the field rather than
                // failing the request. M11: whether it carries
                // Elementor data (editor notice).
                if let Ok(seo) = get_post_seo(ctx.db.as_ref(), ctx.site_id, post.id).await {
                    if let Ok(seo_value) = to_value(seo) {
                        value["seo"] = seo_value;
                    }
                }
                value["hasElementorData"] = json!(has_elementor_data(ctx.db.as_ref(), ctx.site_id, post.id)
                    .await
                    .unwrap_or(false));
                // P4: a pending schedule is exposed so the editor can
                // show it (absent when nothing is scheduled), and so
                // is the featured image id (absent when unset).
                let value = attach_publish_at(ctx, value, post.id).await;
                let value = attach_featured_image(ctx, value, post.id).await;
                ok_json(value, 200)
            })
        },
        RouteOptions::default(),
    );

    // -------------------------------------------------------------- revisions
    router.get(
        "/api/posts/:id/revisions",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "post id")?;
                ok(list_revisions(ctx.db.as_ref(), ctx.site_id, id).await?, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/posts/:id/autosave",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "post id")?;
                let body = read_json_body(ctx)?;
                let title = opt_str(&body, "title");
                let content = opt_str(&body, "content");
                let excerpt = opt_str(&body, "excerpt");
                ok(
                    save_autosave(
                        ctx.db.as_ref(), ctx.site_id,
                        id,
                        actor(ctx),
                        title.as_deref(),
                        content.as_deref(),
                        excerpt.as_deref(),
                    )
                    .await?,
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
        "/api/revisions/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "revision id")?;
                ok(get_revision(ctx.db.as_ref(), ctx.site_id, id).await?, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.get(
        "/api/revisions/:from/diff/:to",
        |ctx| {
            Box::pin(async move {
                let from = parse_id(ctx, "from", "revision id")?;
                let to = parse_id(ctx, "to", "revision id")?;
                ok(diff_revisions(ctx.db.as_ref(), ctx.site_id, from, to).await?, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/revisions/:id/restore",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "revision id")?;
                let post = restore_revision(ctx.db.as_ref(), ctx.site_id, id).await?;
                log_action(ctx, "post.revision_restore", "post", post.id).await;
                ok(post, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    // ------------------------------------------------------------------ terms
    router.get(
        "/api/terms",
        |ctx| {
            Box::pin(async move {
                let taxonomy = ctx.query("taxonomy").unwrap_or("category").to_string();
                let parent_id = ctx.query("parent").and_then(|s| s.parse::<i64>().ok());
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                ok(
                    list_terms(
                        ctx.db.as_ref(), ctx.site_id,
                        &taxonomy,
                        ListTermsOptions {
                            parent_id,
                            page: Some(page),
                            per_page: Some(per_page),
                        },
                    )
                    .await?,
                    200,
                )
            })
        },
        RouteOptions::default(),
    );

    router.post(
        "/api/terms",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let term = create_term(
                    ctx.db.as_ref(), ctx.site_id,
                    CreateTermInput {
                        name: body_str(&body, "name"),
                        slug: opt_str(&body, "slug"),
                        taxonomy: body_str(&body, "taxonomy"),
                        description: opt_str(&body, "description"),
                        parent_id: opt_i64(&body, "parentId"),
                    },
                )
                .await?;
                log_action(ctx, "term.create", "term", term.term_taxonomy_id).await;
                ok(term, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_categories"),
        },
    );

    router.get(
        "/api/terms/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "term id")?;
                ok(get_term(ctx.db.as_ref(), ctx.site_id, id).await?, 200)
            })
        },
        RouteOptions::default(),
    );

    router.put(
        "/api/terms/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "term id")?;
                let body = read_json_body(ctx)?;
                let term = update_term(
                    ctx.db.as_ref(), ctx.site_id,
                    id,
                    UpdateTermInput {
                        name: opt_str(&body, "name"),
                        slug: opt_str(&body, "slug"),
                        description: opt_str(&body, "description"),
                        parent_id: opt_i64(&body, "parentId"),
                    },
                )
                .await?;
                log_action(ctx, "term.update", "term", term.term_taxonomy_id).await;
                ok(term, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_categories"),
        },
    );

    router.delete(
        "/api/terms/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "term id")?;
                let _ = delete_term(ctx.db.as_ref(), ctx.site_id, id).await?;
                log_action(ctx, "term.delete", "term", id).await;
                ok_json(json!({ "deleted": true }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_categories"),
        },
    );

    router.get(
        "/api/posts/:id/terms",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "post id")?;
                let taxonomy = ctx.query("taxonomy").map(|s| s.to_string());
                ok(
                    get_post_terms(ctx.db.as_ref(), ctx.site_id, id, taxonomy.as_deref()).await?,
                    200,
                )
            })
        },
        RouteOptions::default(),
    );

    router.post(
        "/api/posts/:id/terms",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "post id")?;
                let body = read_json_body(ctx)?;
                let taxonomy = require_string(&body["taxonomy"], "taxonomy", 1, 100)?;
                let term_ids: Vec<i64> = body
                    .get("termIds")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_i64()).collect())
                    .unwrap_or_default();
                let terms = set_post_terms(ctx.db.as_ref(), ctx.site_id, id, &taxonomy, &term_ids).await?;
                log_action(ctx, "post.terms_set", "post", id).await;
                ok(terms, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.delete(
        "/api/posts/:id/terms/:termId",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "post id")?;
                let term_id = parse_id(ctx, "termId", "term id")?;
                let terms = remove_post_term(ctx.db.as_ref(), ctx.site_id, id, term_id).await?;
                log_action(ctx, "post.term_remove", "post", id).await;
                ok(terms, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    // ------------------------------------------------- post types (P4)
    // Custom post types: service functions are P2 (persistent JSON in
    // `wp_options` under `wl_post_types`); P4 adds the REST routes the
    // admin UI needs. Reads are public (mirroring terms reads — the
    // definitions are not sensitive, and the public archive renderer
    // needs them); writes are `manage_options` (administrator-only),
    // because a post type is site structure, not content.
    router.get(
        "/api/post-types",
        |ctx| Box::pin(async move { ok(list_post_types(ctx.db.as_ref(), ctx.site_id).await?, 200) }),
        RouteOptions::default(),
    );

    router.post(
        "/api/post-types",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let created = register_post_type(
                    ctx.db.as_ref(), ctx.site_id,
                    RegisterPostTypeInput {
                        slug: body_str(&body, "slug"),
                        label: body_str(&body, "label"),
                        singular_label: opt_str(&body, "singularLabel"),
                        description: opt_str(&body, "description"),
                        public: opt_bool(&body, "public"),
                        has_archive: opt_bool(&body, "hasArchive"),
                        hierarchical: opt_bool(&body, "hierarchical"),
                        supports: opt_str_array(&body, "supports"),
                        rewrite_slug: opt_str(&body, "rewriteSlug"),
                        taxonomies: opt_str_array(&body, "taxonomies"),
                    },
                )
                .await?;
                log_action(ctx, "post_type.create", "post_type", 0).await;
                ok(created, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/post-types/:slug",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                ok(get_post_type(ctx.db.as_ref(), ctx.site_id, &slug).await?, 200)
            })
        },
        RouteOptions::default(),
    );

    router.put(
        "/api/post-types/:slug",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                let body = read_json_body(ctx)?;
                let updated = update_post_type(
                    ctx.db.as_ref(), ctx.site_id,
                    &slug,
                    UpdatePostTypeInput {
                        label: opt_str(&body, "label"),
                        singular_label: opt_str(&body, "singularLabel"),
                        description: opt_str(&body, "description"),
                        public: opt_bool(&body, "public"),
                        has_archive: opt_bool(&body, "hasArchive"),
                        hierarchical: opt_bool(&body, "hierarchical"),
                        supports: opt_str_array(&body, "supports"),
                        rewrite_slug: opt_str(&body, "rewriteSlug"),
                        taxonomies: opt_str_array(&body, "taxonomies"),
                    },
                )
                .await?;
                log_action(ctx, "post_type.update", "post_type", 0).await;
                ok(updated, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.delete(
        "/api/post-types/:slug",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                let deleted = delete_post_type(ctx.db.as_ref(), ctx.site_id, &slug).await?;
                log_action(ctx, "post_type.delete", "post_type", 0).await;
                ok_json(json!({ "deleted": deleted }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    // ------------------------------------------------- taxonomies (P4)
    // Same shape and guards as post types: reads public, writes
    // `manage_options`; definitions persist as JSON in `wp_options`
    // under `wl_taxonomies`.
    router.get(
        "/api/taxonomies",
        |ctx| Box::pin(async move { ok(list_taxonomies(ctx.db.as_ref(), ctx.site_id).await?, 200) }),
        RouteOptions::default(),
    );

    router.post(
        "/api/taxonomies",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let created = register_taxonomy(
                    ctx.db.as_ref(), ctx.site_id,
                    RegisterTaxonomyInput {
                        slug: body_str(&body, "slug"),
                        label: body_str(&body, "label"),
                        singular_label: opt_str(&body, "singularLabel"),
                        description: opt_str(&body, "description"),
                        hierarchical: opt_bool(&body, "hierarchical"),
                        public: opt_bool(&body, "public"),
                        object_types: opt_str_array(&body, "objectTypes"),
                        rewrite_slug: opt_str(&body, "rewriteSlug"),
                    },
                )
                .await?;
                log_action(ctx, "taxonomy.create", "taxonomy", 0).await;
                ok(created, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/taxonomies/:slug",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                ok(get_taxonomy(ctx.db.as_ref(), ctx.site_id, &slug).await?, 200)
            })
        },
        RouteOptions::default(),
    );

    router.put(
        "/api/taxonomies/:slug",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                let body = read_json_body(ctx)?;
                let updated = update_taxonomy(
                    ctx.db.as_ref(), ctx.site_id,
                    &slug,
                    UpdateTaxonomyInput {
                        label: opt_str(&body, "label"),
                        singular_label: opt_str(&body, "singularLabel"),
                        description: opt_str(&body, "description"),
                        hierarchical: opt_bool(&body, "hierarchical"),
                        public: opt_bool(&body, "public"),
                        object_types: opt_str_array(&body, "objectTypes"),
                        rewrite_slug: opt_str(&body, "rewriteSlug"),
                    },
                )
                .await?;
                log_action(ctx, "taxonomy.update", "taxonomy", 0).await;
                ok(updated, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.delete(
        "/api/taxonomies/:slug",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                let deleted = delete_taxonomy(ctx.db.as_ref(), ctx.site_id, &slug).await?;
                log_action(ctx, "taxonomy.delete", "taxonomy", 0).await;
                ok_json(json!({ "deleted": deleted }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    // --------------------------------------------------------------- comments
    router.get(
        "/api/comments",
        |ctx| {
            Box::pin(async move {
                let post_id = ctx.query("postId").and_then(|s| s.parse::<i64>().ok());
                let status = ctx.query("status").map(|s| s.to_string());
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                ok(
                    list_comments(
                        ctx.db.as_ref(), ctx.site_id,
                        ListCommentsOptions {
                            post_id,
                            status,
                            page: Some(page),
                            per_page: Some(per_page),
                        },
                    )
                    .await?,
                    200,
                )
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("moderate_comments"),
        },
    );

    router.get(
        "/api/posts/:slug/comments",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                let post = get_post_by_slug(ctx.db.as_ref(), ctx.site_id, &slug).await?;
                let Some(post) = post else {
                    return Err(not_found("Post not found."));
                };
                ok(list_approved_threaded(ctx.db.as_ref(), ctx.site_id, post.id).await?, 200)
            })
        },
        RouteOptions::default(),
    );

    router.post(
        "/api/posts/:slug/comments",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                let post = get_post_by_slug(ctx.db.as_ref(), ctx.site_id, &slug).await?;
                let Some(post) = post else {
                    return Err(not_found("Post not found."));
                };
                let body = read_json_body(ctx)?;
                let can_moderate = ctx
                    .user()
                    .map(|u| user_can(u, "moderate_comments"))
                    .unwrap_or(false);
                let honeypot = opt_str(&body, "website").or_else(|| opt_str(&body, "honeypot"));
                let comment = create_comment(
                    ctx.db.as_ref(), ctx.site_id,
                    CreateCommentInput {
                        post_id: post.id,
                        author: body_str(&body, "author"),
                        author_email: Some(body_str(&body, "author_email")),
                        author_url: Some(body_str(&body, "author_url")),
                        content: body_str(&body, "content"),
                        parent_id: opt_i64(&body, "parentId"),
                        user_id: Some(actor(ctx)),
                        author_ip: Some(ctx.header("cf-connecting-ip").unwrap_or("").to_string()),
                        honeypot,
                    },
                    can_moderate,
                )
                .await?;
                log_action(ctx, "comment.create", "comment", comment.id).await;
                // NOTE: v1 emails the admin here; there is no mail
                // integration in P2 (P3), so no notification is sent.
                ok(comment, 201)
            })
        },
        RouteOptions::default(),
    );

    router.post(
        "/api/comments/:id/approve",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "comment id")?;
                let comment = set_comment_status(ctx.db.as_ref(), ctx.site_id, id, "approved").await?;
                log_action(ctx, "comment.approve", "comment", comment.id).await;
                ok(comment, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("moderate_comments"),
        },
    );

    router.post(
        "/api/comments/:id/spam",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "comment id")?;
                let comment = set_comment_status(ctx.db.as_ref(), ctx.site_id, id, "spam").await?;
                log_action(ctx, "comment.spam", "comment", comment.id).await;
                ok(comment, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("moderate_comments"),
        },
    );

    router.post(
        "/api/comments/:id/trash",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "comment id")?;
                let comment = set_comment_status(ctx.db.as_ref(), ctx.site_id, id, "trash").await?;
                log_action(ctx, "comment.trash", "comment", comment.id).await;
                ok(comment, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("moderate_comments"),
        },
    );

    router.delete(
        "/api/comments/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "comment id")?;
                let _ = delete_comment(ctx.db.as_ref(), ctx.site_id, id).await?;
                log_action(ctx, "comment.delete", "comment", id).await;
                ok_json(json!({ "deleted": true }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("moderate_comments"),
        },
    );

    // ------------------------------------------------------------------ menus
    router.get(
        "/api/menus",
        |ctx| Box::pin(async move { ok(list_menus(ctx.db.as_ref(), ctx.site_id).await?, 200) }),
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/menus",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let name = require_string(&body["name"], "name", 1, 200)?;
                let menu = create_menu(ctx.db.as_ref(), ctx.site_id, &name).await?;
                log_action(ctx, "menu.create", "menu", menu.id).await;
                ok(menu, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_theme_options"),
        },
    );

    // Registered before /api/menus/:id so "locations" is not read as an id.
    router.get(
        "/api/menus/locations",
        |ctx| Box::pin(async move { ok(get_menu_locations(ctx.db.as_ref(), ctx.site_id).await?, 200) }),
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/menus/locations",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let location = require_string(&body["location"], "location", 1, 200)?;
                let menu_id = match body.get("menuId") {
                    Some(Value::Null) | None => None,
                    Some(v) => v.as_i64(),
                };
                let locations = set_menu_location(ctx.db.as_ref(), ctx.site_id, &location, menu_id).await?;
                log_action(ctx, "menu.location_set", "menu", 0).await;
                ok(locations, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_theme_options"),
        },
    );

    router.get(
        "/api/menus/location/:location",
        |ctx| {
            Box::pin(async move {
                let location = ctx.param("location").unwrap_or("").to_string();
                ok(
                    resolve_menu_by_location(ctx.db.as_ref(), ctx.site_id, &location).await?,
                    200,
                )
            })
        },
        RouteOptions::default(),
    );

    router.delete(
        "/api/menus/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "menu id")?;
                let _ = delete_menu(ctx.db.as_ref(), ctx.site_id, id).await?;
                log_action(ctx, "menu.delete", "menu", id).await;
                ok_json(json!({ "deleted": true }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_theme_options"),
        },
    );

    router.get(
        "/api/menus/:id/items",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "menu id")?;
                ok(list_menu_items(ctx.db.as_ref(), ctx.site_id, id).await?, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/menus/:id/items",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "menu id")?;
                let body = read_json_body(ctx)?;
                let item = add_menu_item(
                    ctx.db.as_ref(), ctx.site_id,
                    id,
                    CreateMenuItemInput {
                        title: body_str(&body, "title"),
                        item_type: body_str(&body, "type"),
                        url: opt_str(&body, "url"),
                        object_id: opt_i64(&body, "objectId"),
                        parent_id: opt_i64(&body, "parentId"),
                        position: opt_i64(&body, "position"),
                    },
                )
                .await?;
                log_action(ctx, "menu.item_add", "menu", id).await;
                ok(item, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_theme_options"),
        },
    );

    router.put(
        "/api/menus/:id/items/:itemId",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "menu id")?;
                let item_id = parse_id(ctx, "itemId", "menu item id")?;
                let body = read_json_body(ctx)?;
                let item = update_menu_item(
                    ctx.db.as_ref(), ctx.site_id,
                    id,
                    item_id,
                    UpdateMenuItemInput {
                        title: opt_str(&body, "title"),
                        url: opt_str(&body, "url"),
                        parent_id: opt_i64(&body, "parentId"),
                        position: opt_i64(&body, "position"),
                    },
                )
                .await?;
                log_action(ctx, "menu.item_update", "menu", id).await;
                ok(item, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_theme_options"),
        },
    );

    router.delete(
        "/api/menus/:id/items/:itemId",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "menu id")?;
                let item_id = parse_id(ctx, "itemId", "menu item id")?;
                let _ = remove_menu_item(ctx.db.as_ref(), ctx.site_id, id, item_id).await?;
                log_action(ctx, "menu.item_remove", "menu", id).await;
                ok_json(json!({ "deleted": true }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_theme_options"),
        },
    );

    // ----------------------------------------------------------------- search
    router.get(
        "/api/search",
        |ctx| {
            Box::pin(async move {
                let q = ctx.query("q").unwrap_or("").to_string();
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                let post_type = ctx.query("type").map(|s| s.to_string());
                ok(
                    cms_search_posts(
                        ctx.db.as_ref(), ctx.site_id,
                        &q,
                        SearchOptions {
                            page: Some(page),
                            per_page: Some(per_page),
                            post_type,
                        },
                    )
                    .await?,
                    200,
                )
            })
        },
        RouteOptions::default(),
    );

    // -------------------------------------------------------------- permalinks
    router.get(
        "/api/permalinks",
        |ctx| {
            Box::pin(async move {
                let structure = get_permalink_structure(ctx.db.as_ref(), ctx.site_id).await?;
                ok_json(json!({ "structure": structure.as_str() }), 200)
            })
        },
        RouteOptions::default(),
    );

    router.put(
        "/api/permalinks",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let structure_raw = require_string(&body["structure"], "structure", 1, 100)?;
                let structure = wordless_cms::permalinks::set_permalink_structure(
                    ctx.db.as_ref(), ctx.site_id,
                    &structure_raw,
                )
                .await?;
                log_action(ctx, "settings.permalink", "option", 0).await;
                ok_json(json!({ "structure": structure.as_str() }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    // ------------------------------------------------------------------ media
    router.get(
        "/api/media",
        |ctx| {
            Box::pin(async move {
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                let mime_type = ctx.query("mime").map(|s| s.to_string());
                ok(
                    list_media(
                        ctx.db.as_ref(), ctx.site_id,
                        ListMediaOpts {
                            page: Some(page),
                            per_page: Some(per_page),
                            mime_type,
                        },
                    )
                    .await?,
                    200,
                )
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("upload_files"),
        },
    );

    router.post(
        "/api/media",
        |ctx| {
            Box::pin(async move {
                let store = require_store(ctx)?;
                let content_type = ctx.header("content-type").unwrap_or("").to_string();
                let form = parse_multipart(ctx.body(), &content_type)?;
                let Some(file) = form.file("file") else {
                    return Err(bad_request("Upload requires a multipart \"file\" field."));
                };
                let record = upload_media(
                    ctx.db.as_ref(), ctx.site_id,
                    store.as_ref(),
                    UploadMediaInput {
                        title: form.field("title").map(|s| s.to_string()),
                        filename: file.filename.clone().unwrap_or_else(|| "file".to_string()),
                        mime_type: file
                            .content_type
                            .clone()
                            .unwrap_or_else(|| "application/octet-stream".to_string()),
                        data: file.bytes.clone(),
                        author_id: actor(ctx),
                        alt: form.field("alt").map(|s| s.to_string()),
                    },
                )
                .await?;
                log_action(ctx, "media.upload", "media", record.id).await;
                ok(record, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("upload_files"),
        },
    );

    // NOTE: this static route is registered BEFORE "/api/media/:id" —
    // the router matches in registration order, and ":id" would swallow
    // "derivatives" otherwise.
    router.get(
        "/api/media/derivatives",
        |ctx| {
            Box::pin(async move {
                // The Images binding is NOT declared in wrangler.toml,
                // so it is always unbound in P2.
                ok(derivative_status(ctx.db.as_ref(), false).await?, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("upload_files"),
        },
    );

    router.get(
        "/api/media/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "media id")?;
                let record = get_media_record(ctx.db.as_ref(), ctx.site_id, id).await?;
                let Some(record) = record else {
                    return Err(not_found("Media item not found."));
                };
                ok(record, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("upload_files"),
        },
    );

    router.put(
        "/api/media/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "media id")?;
                let body = read_json_body(ctx)?;
                let record =
                    update_media_alt(ctx.db.as_ref(), ctx.site_id, id, body_str(&body, "alt").as_str()).await?;
                log_action(ctx, "media.alt_update", "media", record.id).await;
                ok(record, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("upload_files"),
        },
    );

    router.delete(
        "/api/media/:id",
        |ctx| {
            Box::pin(async move {
                let store = require_store(ctx)?;
                let id = parse_id(ctx, "id", "media id")?;
                let deleted = delete_media(ctx.db.as_ref(), ctx.site_id, store.as_ref(), id).await?;
                log_action(ctx, "media.delete", "media", id).await;
                ok_json(json!({ "deleted": deleted }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("upload_files"),
        },
    );

    // ------------------------------------------------------------ file manager
    router.get(
        "/api/files",
        |ctx| {
            Box::pin(async move {
                let store = require_store(ctx)?;
                let prefix = ctx.query("prefix").unwrap_or("").to_string();
                ok(
                    list_files(ctx.db.as_ref(), ctx.site_id, store.as_ref(), &prefix).await?,
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
        "/api/files/upload",
        |ctx| {
            Box::pin(async move {
                let store = require_store(ctx)?;
                let content_type = ctx.header("content-type").unwrap_or("").to_string();
                let form = parse_multipart(ctx.body(), &content_type)?;
                let Some(file) = form.file("file") else {
                    return Err(bad_request("Upload requires a multipart \"file\" field."));
                };
                let prefix = form.field("prefix").unwrap_or("").to_string();
                let entry = upload_file(
                    store.as_ref(),
                    &prefix,
                    UploadFileInput {
                        filename: file.filename.clone().unwrap_or_else(|| "file".to_string()),
                        mime_type: file
                            .content_type
                            .clone()
                            .unwrap_or_else(|| "application/octet-stream".to_string()),
                        data: file.bytes.clone(),
                    },
                )
                .await?;
                log_action(ctx, "files.upload", "file", 0).await;
                ok(entry, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.get(
        "/api/files/download",
        |ctx| {
            Box::pin(async move {
                let store = require_store(ctx)?;
                let key = ctx.query("key").unwrap_or("").to_string();
                let found = download_file(store.as_ref(), &key).await?;
                let mut resp = raw_response(200, found.data, &found.content_type);
                resp.extra_headers.push((
                    "Content-Disposition".to_string(),
                    format!("attachment; filename=\"{}\"", found.name.replace('"', "")),
                ));
                resp.extra_headers
                    .push(("Cache-Control".to_string(), "no-store".to_string()));
                Ok(resp)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.post(
        "/api/files/rename",
        |ctx| {
            Box::pin(async move {
                let store = require_store(ctx)?;
                let body = read_json_body(ctx)?;
                let entry = rename_file(
                    ctx.db.as_ref(), ctx.site_id,
                    store.as_ref(),
                    &body_str(&body, "key"),
                    &body_str(&body, "newName"),
                )
                .await?;
                log_action(ctx, "files.rename", "file", 0).await;
                ok(entry, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.post(
        "/api/files/delete",
        |ctx| {
            Box::pin(async move {
                let store = require_store(ctx)?;
                let body = read_json_body(ctx)?;
                let result =
                    delete_file(ctx.db.as_ref(), ctx.site_id, store.as_ref(), &body_str(&body, "key")).await?;
                log_action(ctx, "files.delete", "file", 0).await;
                ok(result, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    // Serve a stored object: GET /media/<object-key> (flat keys, no
    // slashes). A resize (?width=&format=) is never faked: with Images
    // unbound the original is served and the response says so
    // (X-Wordless-Derivatives: unavailable). Whole object, 200 — v1
    // has no Range support.
    router.get(
        "/media/:key",
        |ctx| {
            Box::pin(async move {
                let store = require_store(ctx)?;
                let key = ctx.param("key").unwrap_or("").to_string();
                let record = find_media_by_key(ctx.db.as_ref(), ctx.site_id, &key).await?;
                let Some(record) = record else {
                    return Err(not_found("Media file not found."));
                };
                let width = ctx.query("width").and_then(|s| s.parse::<i64>().ok());
                let format = ctx.query("format").map(|s| s.to_string());
                let resolution = resolve_media_request(
                    ctx.db.as_ref(),
                    &record,
                    width,
                    format.as_deref(),
                    false,
                )
                .await?;
                let served = match &resolution {
                    MediaResolution::Original {
                        object_key,
                        derivatives_unavailable,
                    } => {
                        serve_stored_object(
                            store.as_ref(),
                            object_key,
                            &record.mime_type,
                            *derivatives_unavailable,
                        )
                        .await?
                    }
                    MediaResolution::Derivative { object_key, format } => {
                        serve_stored_object(
                            store.as_ref(),
                            object_key,
                            &format!("image/{format}"),
                            false,
                        )
                        .await?
                    }
                    // Unreachable with Images unbound, but serve the
                    // original rather than failing if it ever occurs.
                    MediaResolution::NeedsDerivation { object_key, .. } => {
                        serve_stored_object(store.as_ref(), object_key, &record.mime_type, true)
                            .await?
                    }
                };
                let mut resp = raw_response(200, served.bytes, &served.content_type);
                resp.extra_headers.push((
                    "Cache-Control".to_string(),
                    served.cache_control.to_string(),
                ));
                if let Some(header) = served.derivatives_header {
                    resp.extra_headers
                        .push(("X-Wordless-Derivatives".to_string(), header.to_string()));
                }
                Ok(resp)
            })
        },
        RouteOptions::default(),
    );


    // ----------------------------------------------------------------- themes
    // Literal paths are registered before /api/themes/:slug so "active",
    // "import" and "preview" are never read as a slug (first match wins).
    router.get(
        "/api/themes",
        |ctx| {
            Box::pin(async move {
                ok(
                    wordless_themes::registry::list_themes(ctx.db.as_ref()).await?,
                    200,
                )
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_theme_options"),
        },
    );

    router.get(
        "/api/themes/active",
        |ctx| {
            Box::pin(async move {
                let slug = get_active_theme_slug(ctx.db.as_ref(), ctx.site_id).await?;
                let theme = get_theme(ctx.db.as_ref(), &slug).await?;
                let variation = get_variation(ctx.db.as_ref(), ctx.site_id, &slug).await?;
                ok_json(
                    json!({ "active": to_value(theme)?, "variation": to_value(variation)? }),
                    200,
                )
            })
        },
        RouteOptions::default(),
    );

    router.post(
        "/api/themes/import",
        |ctx| {
            Box::pin(async move {
                let store = require_store(ctx)?;
                let content_type = ctx.header("content-type").unwrap_or("").to_string();
                let form = parse_multipart(ctx.body(), &content_type)?;
                let Some(file) = form.file("file") else {
                    return Err(bad_request(
                        "Import requires a multipart \"file\" field (.zip).",
                    ));
                };
                let filename = file
                    .filename
                    .clone()
                    .unwrap_or_else(|| "theme.zip".to_string());
                let result = import_theme(
                    ctx.db.as_ref(),
                    store.as_ref(),
                    &filename,
                    &file.bytes,
                    ctx.converter.as_deref(),
                )
                .await?;
                log_action(ctx, "theme.import", "theme", 0).await;
                ok(result, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_theme_options"),
        },
    );

    router.post(
        "/api/themes/import-classic",
        |ctx| {
            Box::pin(async move {
                let store = require_store(ctx)?;
                let content_type = ctx.header("content-type").unwrap_or("").to_string();
                let form = parse_multipart(ctx.body(), &content_type)?;
                let Some(file) = form.file("file") else {
                    return Err(bad_request(
                        "Import requires a multipart \"file\" field (.zip).",
                    ));
                };
                let filename = file
                    .filename
                    .clone()
                    .unwrap_or_else(|| "theme.zip".to_string());
                let result = import_classic_theme(
                    ctx.db.as_ref(),
                    store.as_ref(),
                    &filename,
                    &file.bytes,
                    ctx.converter.as_deref(),
                )
                .await?;
                log_action(ctx, "theme.import_classic", "theme", 0).await;
                ok(result, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_theme_options"),
        },
    );

    router.post(
        "/api/themes/preview",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let slug = require_string(&body["slug"], "slug", 1, 100)?;
                let Some(settings) = body.get("settings") else {
                    return Err(bad_request("Preview requires a \"settings\" object."));
                };
                if !settings.is_object() {
                    return Err(bad_request("Preview requires a \"settings\" object."));
                }
                ok(create_preview(ctx.db.as_ref(), ctx.site_id, &slug, settings).await?, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_theme_options"),
        },
    );

    router.get(
        "/api/themes/:slug",
        |ctx| {
            Box::pin(async move {
                let store = require_store(ctx)?;
                let slug = ctx.param("slug").unwrap_or("").to_string();
                ok(
                    get_theme_detail(ctx.db.as_ref(), store.as_ref(), &slug).await?,
                    200,
                )
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_theme_options"),
        },
    );

    router.post(
        "/api/themes/:slug/activate",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                let theme = activate_theme(ctx.db.as_ref(), ctx.site_id, &slug).await?;
                log_action(ctx, "theme.activate", "theme", 0).await;
                ok(theme, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_theme_options"),
        },
    );

    router.post(
        "/api/themes/:slug/delete",
        |ctx| {
            Box::pin(async move {
                let store = require_store(ctx)?;
                let slug = ctx.param("slug").unwrap_or("").to_string();
                let deleted = delete_theme(ctx.db.as_ref(), store.as_ref(), &slug).await?;
                log_action(ctx, "theme.delete", "theme", 0).await;
                ok_json(json!({ "deleted": deleted }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_theme_options"),
        },
    );

    router.get(
        "/api/themes/:slug/variation",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                let variation = get_variation(ctx.db.as_ref(), ctx.site_id, &slug).await?;
                ok_json(json!({ "variation": to_value(variation)? }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_theme_options"),
        },
    );

    router.put(
        "/api/themes/:slug/variation",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                let body = read_json_body(ctx)?;
                let name = opt_str(&body, "name");
                let settings = body
                    .get("settings")
                    .cloned()
                    .unwrap_or(Value::Object(Default::default()));
                let variation =
                    save_variation(ctx.db.as_ref(), ctx.site_id, &slug, name.as_deref(), &settings).await?;
                log_action(ctx, "theme.variation_save", "theme", 0).await;
                ok(variation, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_theme_options"),
        },
    );

    router.delete(
        "/api/themes/:slug/variation",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("").to_string();
                let deleted = delete_variation(ctx.db.as_ref(), ctx.site_id, &slug).await?;
                log_action(ctx, "theme.variation_reset", "theme", 0).await;
                ok_json(json!({ "deleted": deleted }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_theme_options"),
        },
    );

    // The active theme's stylesheet: generated theme.json CSS +
    // style.css (block themes) or the imported style.css (classic
    // themes). A preview token (?wl_preview=…) applies unsaved
    // variation overrides; ?v=<hash> is accepted and ignored (the
    // hash is recomputed from the served CSS).
    router.get(
        "/theme.css",
        |ctx| {
            Box::pin(async move {
                let store = require_store(ctx)?;
                let preview_token = ctx.query("wl_preview").map(|s| s.to_string());
                let variation =
                    resolve_variation(ctx.db.as_ref(), ctx.site_id, preview_token.as_deref()).await?;
                let sheet =
                    get_active_stylesheet(ctx.db.as_ref(), ctx.site_id, store.as_ref(), variation.as_ref())
                        .await?;
                Ok(raw_text(200, sheet.css, "text/css; charset=utf-8"))
            })
        },
        RouteOptions::default(),
    );

    // ------------------------------------------------------------------- auth
    // Guarded login (v1 M9): lockout (429), TOTP-required (401),
    // forced-reset (403), generic 401 otherwise. Only the lockout and
    // the TOTP-required states are distinguished, because the admin UI
    // needs them to prompt correctly.
    router.post(
        "/api/auth/login",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let login_name = require_string(&body["login"], "login", 1, 60)?;
                let password = require_string(&body["password"], "password", 1, 200)?;
                let totp = opt_str(&body, "totp");
                let ip = ctx.header("cf-connecting-ip").map(|s| s.to_string());
                let user_agent = ctx.header("user-agent").map(|s| s.to_string());
                let outcome = login_guarded(
                    ctx.db.as_ref(),
                    &login_name,
                    &password,
                    totp.as_deref(),
                    ip.as_deref(),
                    user_agent.as_deref(),
                    now_ms(),
                )
                .await?;
                match outcome {
                    LoginOutcome::Ok { user, token, .. } => {
                        // Log with the just-authenticated user explicitly:
                        // log_action would read the request session, which
                        // does not exist yet at login time (actor 0).
                        let _ = log_activity(ctx.db.as_ref(), user.id, "auth.login", "user", user.id).await;
                        // P4: a user signing in with a temporary
                        // password after an admin reset is told, in
                        // the response, that they must set a new one
                        // (`mustChangePassword`). Additive — no v1
                        // field changes shape.
                        let must_change_password = is_password_change_required(ctx.db.as_ref(), user.id)
                            .await
                            .unwrap_or(false);
                        let response = RouteResponse::json_ok(
                            json!({ "user": user.to_auth_user().to_json(), "mustChangePassword": must_change_password }),
                            200,
                        )
                        .with_header("Set-Cookie", &build_default_session_cookie(&token));
                        Ok(response)
                    }
                    LoginOutcome::Invalid => {
                        Err(unauthorized("Invalid login or password."))
                    }
                    LoginOutcome::TotpRequired => Err(WordlessError::new(
                        "totp_required",
                        401,
                        "Two-factor authentication is required for this account.",
                    )),
                    LoginOutcome::LockedUntil { locked_until } => Err(WordlessError::new(
                        "locked_out",
                        429,
                        &format!("Too many failed login attempts. Try again after {locked_until}."),
                    )),
                    LoginOutcome::ForcedReset => Err(WordlessError::new(
                        "password_reset_required",
                        403,
                        "This account was imported from WordPress and needs a password reset before it can sign in. Ask an administrator to reset the password.",
                    )),
                }
            })
        },
        RouteOptions::default(),
    );

    router.get(
        "/api/auth/2fa/status",
        |ctx| {
            Box::pin(async move {
                let user = ctx
                    .user()
                    .cloned()
                    .ok_or_else(|| unauthorized("Authentication required."))?;
                let enabled = is_totp_enabled(ctx.db.as_ref(), user.id).await?;
                ok_json(json!({ "enabled": enabled }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/auth/2fa/setup",
        |ctx| {
            Box::pin(async move {
                let user = ctx
                    .user()
                    .cloned()
                    .ok_or_else(|| unauthorized("Authentication required."))?;
                let setup = setup_totp(ctx.db.as_ref(), user.id, &user.login).await?;
                log_action(ctx, "auth.2fa_setup_started", "user", user.id).await;
                ok(setup, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/auth/2fa/confirm",
        |ctx| {
            Box::pin(async move {
                let user = ctx
                    .user()
                    .cloned()
                    .ok_or_else(|| unauthorized("Authentication required."))?;
                let body = read_json_body(ctx)?;
                let code = require_string(&body["code"], "code", 1, 12)?;
                // Recovery codes are returned ONCE here — the admin must
                // store them somewhere safe; they are never shown again.
                let codes = confirm_totp(ctx.db.as_ref(), user.id, &code, now_ms()).await?;
                log_action(ctx, "auth.2fa_enabled", "user", user.id).await;
                ok_json(json!({ "recoveryCodes": codes }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/auth/2fa/disable",
        |ctx| {
            Box::pin(async move {
                let user = ctx
                    .user()
                    .cloned()
                    .ok_or_else(|| unauthorized("Authentication required."))?;
                let body = read_json_body(ctx)?;
                let password = require_string(&body["password"], "password", 1, 200)?;
                let disabled = disable_totp(ctx.db.as_ref(), user.id, &password).await?;
                log_action(ctx, "auth.2fa_disabled", "user", user.id).await;
                // Disabling 2FA signs the user out everywhere, including
                // this session.
                let response = RouteResponse::json_ok(json!({ "disabled": disabled }), 200)
                    .with_header("Set-Cookie", &clear_session_cookie());
                Ok(response)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.get(
        "/api/auth/sessions",
        |ctx| {
            Box::pin(async move {
                let user = ctx
                    .user()
                    .cloned()
                    .ok_or_else(|| unauthorized("Authentication required."))?;
                let token = parse_session_cookie(ctx.header("cookie"));
                ok(
                    list_user_sessions(ctx.db.as_ref(), user.id, token.as_deref()).await?,
                    200,
                )
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/auth/sessions/revoke-all",
        |ctx| {
            Box::pin(async move {
                let user = ctx
                    .user()
                    .cloned()
                    .ok_or_else(|| unauthorized("Authentication required."))?;
                let revoked = revoke_all_sessions_for_user(ctx.db.as_ref(), user.id).await?;
                log_action(ctx, "auth.sessions_revoked", "user", user.id).await;
                let response = RouteResponse::json_ok(json!({ "revoked": revoked }), 200)
                    .with_header("Set-Cookie", &clear_session_cookie());
                Ok(response)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/auth/logout",
        |ctx| {
            Box::pin(async move {
                if let Some(token) = parse_session_cookie(ctx.header("cookie")) {
                    revoke_session(ctx.db.as_ref(), &token).await?;
                }
                let response = RouteResponse::json_ok(json!({ "loggedOut": true }), 200)
                    .with_header("Set-Cookie", &clear_session_cookie());
                Ok(response)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    // P4: the signed-in user sets a new password (the forced-change
    // step after an admin reset, and the ordinary change flow). The
    // service verifies the current password, hashes the new one,
    // clears the forced-change flag, and revokes every session — the
    // response clears the cookie and the client signs in again.
    router.post(
        "/api/auth/change-password",
        |ctx| {
            Box::pin(async move {
                let user = ctx
                    .user()
                    .cloned()
                    .ok_or_else(|| unauthorized("Authentication required."))?;
                let body = read_json_body(ctx)?;
                let current_password =
                    require_string(&body["currentPassword"], "currentPassword", 1, 200)?;
                let new_password = require_string(&body["newPassword"], "newPassword", 1, 200)?;
                let changed =
                    change_password(ctx.db.as_ref(), user.id, &current_password, &new_password)
                        .await?;
                log_action(ctx, "auth.password_change", "user", user.id).await;
                let response = RouteResponse::json_ok(json!({ "changed": changed }), 200)
                    .with_header("Set-Cookie", &clear_session_cookie());
                Ok(response)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.get(
        "/api/auth/me",
        |ctx| {
            Box::pin(async move {
                // requireAuth guarantees user is non-null; the guard
                // above is the check (same comment as v1).
                let user = ctx
                    .user()
                    .cloned()
                    .ok_or_else(|| unauthorized("Authentication required."))?;
                let must_change_password = is_password_change_required(ctx.db.as_ref(), user.id)
                    .await
                    .unwrap_or(false);
                Ok(RouteResponse::json_ok(
                    json!({ "user": user.to_json(), "mustChangePassword": must_change_password }),
                    200,
                ))
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    // ---------------------------------------------------------------- options
    router.get(
        "/api/options/:key",
        |ctx| {
            Box::pin(async move {
                let key = ctx.param("key").unwrap_or("").to_string();
                let value = wordless_multisite::get_site_option::<serde_json::Value>(
                    ctx.db.as_ref(),
                    ctx.site_id,
                    &key,
                    serde_json::Value::Null,
                )
                .await?;
                ok_json(json!({ "key": key, "value": value }), 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    // --------------------------------------------------------------- settings
    router.get(
        "/api/settings",
        |ctx| Box::pin(async move { ok(get_settings(ctx.db.as_ref(), ctx.site_id).await?, 200) }),
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    // P4 login customiser: PUBLIC (no auth) — the login page renders
    // before anyone signs in, so it must be able to read its own
    // branding. Read-only, returns only the eleven branding values
    // (no secrets, no other options). Writes go through
    // `PUT /api/settings` (manage_options), which accepts the same
    // fields and stores them under the `wl_login_*` keys.
    router.get(
        "/api/login-branding",
        |ctx| Box::pin(async move { ok(get_login_branding(ctx.db.as_ref(), ctx.site_id).await?, 200) }),
        RouteOptions::default(),
    );

    router.put(
        "/api/settings",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let settings = update_settings(ctx.db.as_ref(), ctx.site_id, &body).await?;
                log_action(ctx, "settings.update", "option", 0).await;
                ok(settings, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    // ------------------------------------------------------------------ users
    router.get(
        "/api/users",
        |ctx| {
            Box::pin(async move {
                let page = page_param(ctx)?;
                let per_page = per_page_param(ctx)?;
                ok(list_users(ctx.db.as_ref(), page, per_page).await?, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_users"),
        },
    );

    // P4: one user with their profile fields (bio, avatar, theme,
    // forced-change flag). A user may read their own profile; anyone
    // else's needs `edit_users`. (v1 has no single-user GET at all.)
    router.get(
        "/api/users/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "user id")?;
                let actor_user = ctx
                    .user()
                    .cloned()
                    .ok_or_else(|| unauthorized("Authentication required."))?;
                if actor_user.id != id && !user_can(&actor_user, "edit_users") {
                    return Err(forbidden("You do not have permission to view this user."));
                }
                ok(get_user_profile(ctx.db.as_ref(), id).await?, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    // P4: full profile editing (§0.1 A). A user may edit their own
    // profile; editing another user's needs `edit_users`.
    router.put(
        "/api/users/:id/profile",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "user id")?;
                let actor_user = ctx
                    .user()
                    .cloned()
                    .ok_or_else(|| unauthorized("Authentication required."))?;
                if actor_user.id != id && !user_can(&actor_user, "edit_users") {
                    return Err(forbidden("You do not have permission to edit this user."));
                }
                let body = read_json_body(ctx)?;
                // `avatarId`: absent = leave alone, null = clear,
                // number = set to that attachment id.
                let avatar = match body.get("avatarId") {
                    None => None,
                    Some(Value::Null) => Some(None),
                    Some(v) => match v.as_i64() {
                        Some(n) => Some(Some(n)),
                        None => {
                            return Err(bad_request(
                                "Field \"avatarId\" must be a media attachment id, or null to clear it.",
                            ))
                        }
                    },
                };
                let updated = update_user_profile(
                    ctx.db.as_ref(),
                    id,
                    UpdateProfileInput {
                        display_name: opt_str(&body, "displayName"),
                        email: opt_str(&body, "email"),
                        bio: opt_str(&body, "bio"),
                        avatar,
                        theme: opt_str(&body, "theme"),
                    },
                )
                .await?;
                log_action(ctx, "user.profile_update", "user", updated.user.id).await;
                ok(updated, 200)
            })
        },
        RouteOptions {
            require_auth: true,
            capability: None,
        },
    );

    router.post(
        "/api/users",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let role = opt_str(&body, "role").and_then(|r| Role::from_str(&r));
                let created = create_user(
                    ctx.db.as_ref(),
                    CreateUserInput {
                        login: body_str(&body, "login"),
                        email: body_str(&body, "email"),
                        password: body_str(&body, "password"),
                        display_name: opt_str(&body, "displayName"),
                        nicename: None,
                        url: None,
                        role,
                    },
                )
                .await?;
                log_action(ctx, "user.create", "user", created.id).await;
                ok(created, 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_users"),
        },
    );

    router.put(
        "/api/users/:id/role",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "user id")?;
                let body = read_json_body(ctx)?;
                let Some(role) = opt_str(&body, "role").and_then(|r| Role::from_str(&r)) else {
                    return Err(bad_request("Field \"role\" must be a valid role."));
                };
                let updated = set_user_role(ctx.db.as_ref(), id, role).await?;
                log_action(ctx, "user.role_change", "user", updated.id).await;
                ok(updated, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_users"),
        },
    );

    router.delete(
        "/api/users/:id",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "user id")?;
                let deleted = delete_user(ctx.db.as_ref(), id).await?;
                log_action(ctx, "user.delete", "user", id).await;
                ok_json(json!({ "deleted": deleted }), 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_users"),
        },
    );

    router.post(
        "/api/users/:id/reset-password",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "user id")?;
                // The temporary password is returned ONCE in this
                // response. It is never logged and never stored in
                // plaintext (only the hash is).
                let (user, temporary_password) =
                    reset_user_password(ctx.db.as_ref(), id).await?;
                // P4 (§0.1 A) — CHANGED vs v1: v1 cleared the
                // imported-user flag here so the user could sign in
                // with the temporary password and was merely asked by
                // email to change it. The admin reset now SETS the
                // forced-change flag instead, so the user signs in
                // with the temporary password AND the login/`/me`
                // responses carry `mustChangePassword: true` until
                // they set a new one via `POST /api/auth/change-password`.
                // The imported flag itself is still cleared (as in v1):
                // an imported user who was blocked can now sign in.
                let _ = delete_user_meta(ctx.db.as_ref(), id, PASSWORD_RESET_META_KEY).await;
                let _ = set_password_change_required(ctx.db.as_ref(), id).await;
                log_action(ctx, "user.password_reset", "user", id).await;
                // NOTE: v1 emails the temporary password when the mail
                // integration can send; there is no mail integration
                // in P2 (P3), so `emailed` is false and the password
                // in this response is the admin's only copy.
                ok_json(
                    json!({ "user": to_value(user)?, "temporaryPassword": temporary_password, "emailed": false }),
                    200,
                )
            })
        },
        RouteOptions { require_auth: false, capability: Some("edit_users") },
    );

    // ------------------------------------------------------- feeds + sitemap
    router.get(
        "/feed",
        |ctx| {
            Box::pin(async move {
                let xml = rss_feed(ctx.db.as_ref(), ctx.site_id).await?;
                Ok(raw_text(200, xml, "application/rss+xml; charset=utf-8"))
            })
        },
        RouteOptions::default(),
    );


    // ------------------------------------------------------------- public site
    router.get(
        "/",
        |ctx| {
            Box::pin(async move {
                // Plain permalinks: /?p=<id> is a post URL.
                if let Some(p) = ctx.query("p").map(|s| s.to_string()) {
                    if let Ok(id) = p.parse::<i64>() {
                        if let Some(post) = cms_resolve_post_by_id(ctx.db.as_ref(), ctx.site_id, id).await? {
                            return render_post_page(ctx, &post).await;
                        }
                    }
                    return render_not_found_page(ctx).await;
                }
                let page = page_param(ctx)?;
                render_home_page(ctx, page).await
            })
        },
        RouteOptions::default(),
    );

    router.get(
        "/search",
        |ctx| {
            Box::pin(async move {
                let q = ctx.query("q").unwrap_or("").to_string();
                if q.trim().is_empty() {
                    return render_home_page(ctx, 1).await;
                }
                let page = page_param(ctx)?;
                let opts = render_opts(ctx).await;
                let opts = with_seo(
                    ctx,
                    opts,
                    PageKind::Search,
                    format!("Search: {q}"),
                    String::new(),
                    None,
                    None,
                    Vec::new(),
                    None,
                )
                .await;
                Ok(html_page(
                    render_search_page(ctx.db.as_ref(), ctx.site_id, &q, page, &opts).await?,
                ))
            })
        },
        RouteOptions::default(),
    );

    // P3 routes (conversion, AI, commerce, integrations,
    // marketplace, security, backup, importer, GraphQL, outbound
    // webhooks, shop) live in `routes_p3.rs`.
    crate::routes_p3::register_p3_routes(&mut router);

    // P6 routes (extension runtime: load/unload, permissions, logs,
    // script, host endpoint, hook invocation) live in `routes_p6.rs`.
    crate::routes_p6::register_p6_routes(&mut router);

    // P11 routes (Visual Builder: palette, preview, extension-widget
    // registry, postmeta read) live in `routes_p11.rs`.
    crate::routes_p11::register_p11_routes(&mut router);

    // P12 routes (Multisite: sites API) live in `routes_p12.rs`.
    crate::routes_p12::register_p12_routes(&mut router);

    // P7 + P8 + P9 routes (social, WooCommerce, school) moved to
    // their own Workers in the 2026-11-20 size remedy. The routes are
    // still registered here — same paths, methods, and guards — and
    // each handler proxies to the owning Worker (`routes_proxy.rs`).
    crate::routes_proxy::register_proxy_routes(&mut router);

    router
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::rc::Rc;
    use wordless_db::d1::D1Db;
    use wordless_kernel::capabilities::{AuthUser, Role};

    /// Test double for outbound webhook delivery: accepts everything.
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

    fn test_ctx(method: &str, path: &str, user: Option<AuthUser>) -> Ctx {
        Ctx {
            method: method.to_string(),
            site_id: 1,
            site_slug: String::new(),
            site_archived: false,
            path: path.to_string(),
            query: HashMap::new(),
            headers: HashMap::new(),
            body: Vec::new(),
            params: HashMap::new(),
            user,
            db: Rc::new(D1Db::stub()),
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

    // The route table is exercised end-to-end by the wrangler-dev smoke
    // test (scripts/smoke-p2.sh). Unit coverage here is the envelope
    // shape of the health route through the real router.
    #[test]
    fn health_route_returns_the_v1_envelope_shape() {
        let mut ctx = test_ctx("GET", "/api/health", None);
        let router = build_router();
        let resp = futures_executor::block_on(router.handle(&mut ctx));
        assert_eq!(resp.status, 200);
        let v: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["error"], serde_json::Value::Null);
        assert_eq!(v["data"]["status"], "ok");
        assert_eq!(v["data"]["version"], "2.0.0-dev");
        assert!(v["data"]["time"].as_str().is_some());
    }

    #[test]
    fn me_route_requires_auth() {
        let mut ctx = test_ctx("GET", "/api/auth/me", None);
        let router = build_router();
        let resp = futures_executor::block_on(router.handle(&mut ctx));
        assert_eq!(resp.status, 401);

        let mut ctx2 = test_ctx(
            "GET",
            "/api/auth/me",
            Some(AuthUser {
                id: 1,
                login: "admin".into(),
                email: "a@example.com".into(),
                display_name: "Admin".into(),
                role: Role::Administrator,
            }),
        );
        let resp = futures_executor::block_on(router.handle(&mut ctx2));
        assert_eq!(resp.status, 200);
        let v: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
        assert_eq!(v["data"]["user"]["login"], "admin");
        assert_eq!(v["data"]["user"]["role"], "administrator");
    }

    #[test]
    fn unknown_public_path_falls_through_as_router_404() {
        // The router itself has no wildcard: unknown public paths 404
        // here and the worker entry runs the public fallback.
        let mut ctx = test_ctx("GET", "/hello-world", None);
        let router = build_router();
        let resp = futures_executor::block_on(router.handle(&mut ctx));
        assert_eq!(resp.status, 404);
    }

    #[test]
    fn guarded_route_rejects_anonymous_and_wrong_role() {
        // GET /api/settings needs a logged-in user; PUT needs
        // manage_options, which a subscriber lacks.
        let mut ctx = test_ctx("GET", "/api/settings", None);
        let router = build_router();
        let resp = futures_executor::block_on(router.handle(&mut ctx));
        assert_eq!(resp.status, 401);

        let mut ctx2 = test_ctx(
            "PUT",
            "/api/settings",
            Some(AuthUser {
                id: 2,
                login: "sub".into(),
                email: "s@example.com".into(),
                display_name: "Sub".into(),
                role: Role::Subscriber,
            }),
        );
        ctx2.body = b"{}".to_vec();
        let resp = futures_executor::block_on(router.handle(&mut ctx2));
        assert_eq!(resp.status, 403);
    }
}
