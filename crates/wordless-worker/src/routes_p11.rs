//! P11 route table — the Visual Builder (v2-only routes, no v1
//! counterpart): the palette, layout preview rendering, the
//! extension-widget registry, and postmeta read for layouts.
//!
//! Registered from `routes::build_router` after the P6 table.
//!
//! Layouts are Elementor-compatible JSON stored under the
//! `_elementor_data` postmeta key (the P3 renderer displays them on
//! the public site; the builder edits them). The palette IS the
//! library: core widgets (10), elements (one entry per admin export,
//! 26), icons (104), devices (3) — plus extension widgets, which
//! appear only when their extension is installed AND loaded (the P6
//! kill switch). Extension widgets do NOT render on the public site
//! in P11: a layout holding one renders its other nodes and lists
//! the type as unsupported, in the preview and on the page alike.

use serde_json::{json, Value};
use wordless_builder::{
    delete_widget, list_widgets, loaded_widgets, palette_json, register_widget,
    validate_layout, BuilderWidget, BUILDER_DEVICES,
};
use wordless_cms::postmeta::get_post_meta_map;
use wordless_cms::posts::get_post_by_id;
use wordless_kernel::error::{bad_request, not_found, WordlessError};
use wordless_kernel::router::{read_json_body, RequestContext, RouteOptions, Router};

use crate::context::Ctx;
use crate::routes::{ok, ok_json, parse_id};

/// Register every P11 route on the router built by `routes.rs`.
pub fn register_p11_routes(router: &mut Router<Ctx>) {
    // ------------------------------------------------------- palette
    router.get(
        "/api/builder/palette",
        |ctx| {
            Box::pin(async move {
                let loaded = loaded_widgets(ctx.db.as_ref()).await?;
                let mut palette = palette_json();
                let ext: Vec<Value> = loaded
                    .iter()
                    .map(|w| extension_widget_json(w, false))
                    .collect();
                palette["extensionWidgets"] = Value::Array(ext);
                ok_json(palette, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    // ------------------------------------------------------- preview
    router.post(
        "/api/builder/preview",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let Some(data) = body.get("data") else {
                    return Err(bad_request(
                        "POST /api/builder/preview needs a \"data\" layout value.",
                    ));
                };
                let device = body
                    .get("device")
                    .and_then(|v| v.as_str())
                    .unwrap_or("desktop");
                let Some(device_def) = BUILDER_DEVICES.iter().find(|d| d.id == device) else {
                    return Err(bad_request(
                        "device must be one of: desktop, tablet, phone.",
                    ));
                };
                // Structural problems are 400 with the whole list; an
                // unknown widget type is NOT an error (see validate).
                let valid = validate_layout(data).map_err(|errs| {
                    bad_request(&format!(
                        "Layout is not valid: {}",
                        errs.join("; ")
                    ))
                })?;
                let Some(converter) = ctx.converter.clone() else {
                    return Err(WordlessError::new(
                        "converter_not_configured",
                        503,
                        "Layout preview needs the converter service, which is not configured.",
                    ));
                };
                let html = converter
                    .render_elementor(data)
                    .await
                    .map_err(|e| e.to_wordless())?;
                ok(
                    json!({
                        "html": html,
                        "unsupported": valid.unsupported,
                        "device": device_def.id,
                        "deviceWidth": device_def.width,
                    }),
                    200,
                )
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    // ------------------------------------------------- extension widgets
    router.get(
        "/api/builder/widgets",
        |ctx| {
            Box::pin(async move {
                let all = list_widgets(ctx.db.as_ref()).await?;
                let out: Vec<Value> = all
                    .iter()
                    .map(|w| extension_widget_json(w, true))
                    .collect();
                ok(out, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );

    router.post(
        "/api/builder/widgets",
        |ctx| {
            Box::pin(async move {
                let body = read_json_body(ctx)?;
                let slug = body_str(&body, "extensionSlug");
                let widget_type = body_str(&body, "widgetType");
                let label = body_str(&body, "label");
                let icon = body_str(&body, "icon");
                let description = body_str(&body, "description");
                if slug.is_empty() || widget_type.is_empty() {
                    return Err(bad_request(
                        "POST /api/builder/widgets needs extensionSlug and widgetType.",
                    ));
                }
                let schema_json = match body.get("settingsSchema") {
                    Some(Value::String(s)) => s.clone(),
                    Some(v) => serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string()),
                    None => "[]".to_string(),
                };
                let widget = register_widget(
                    ctx.db.as_ref(),
                    &slug,
                    &widget_type,
                    &label,
                    &icon,
                    &description,
                    &schema_json,
                )
                .await?;
                ok(extension_widget_json(&widget, true), 201)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    router.delete(
        "/api/builder/widgets/:slug/:type",
        |ctx| {
            Box::pin(async move {
                let slug = ctx.param("slug").unwrap_or("");
                let widget_type = ctx.param("type").unwrap_or("");
                if delete_widget(ctx.db.as_ref(), slug, widget_type).await? {
                    ok(json!({ "deleted": true }), 200)
                } else {
                    Err(not_found("No such registered builder widget."))
                }
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("manage_options"),
        },
    );

    // ------------------------------------------------------- postmeta read
    // The P4 write path is PUT /api/posts/:id/meta; the builder also
    // needs to read a page's layout back, so GET returns the full
    // meta map (same guard, same 404 when the post does not exist).
    router.get(
        "/api/posts/:id/meta",
        |ctx| {
            Box::pin(async move {
                let id = parse_id(ctx, "id", "post id")?;
                if get_post_by_id(ctx.db.as_ref(), ctx.site_id, id).await?.is_none() {
                    return Err(not_found("No such post."));
                }
                let meta = get_post_meta_map(ctx.db.as_ref(), ctx.site_id, id).await?;
                ok(meta, 200)
            })
        },
        RouteOptions {
            require_auth: false,
            capability: Some("edit_posts"),
        },
    );
}

fn body_str(body: &Value, key: &str) -> String {
    body.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// One extension widget as API JSON. `rendersPublicly` is always
/// false in P11 (honest limit: extension widgets do not render on
/// the public site yet); `include_status` adds the extension's
/// runtime status (list endpoint only — the palette shows loaded
/// widgets only, so the status there would be a constant).
fn extension_widget_json(w: &BuilderWidget, include_status: bool) -> Value {
    let schema: Value =
        serde_json::from_str(&w.settings_schema_json).unwrap_or(Value::Array(vec![]));
    let mut out = json!({
        "extensionSlug": w.extension_slug,
        "type": w.widget_type,
        "label": w.label,
        "icon": w.icon,
        "description": w.description,
        "fields": schema,
        "rendersPublicly": false,
    });
    if include_status {
        out["extensionStatus"] = Value::String(w.extension_status.clone());
    }
    out
}
