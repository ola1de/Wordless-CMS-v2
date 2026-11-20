//! Route-handler helpers, copied verbatim from the main Worker's
//! `routes.rs` (same shapes, same messages) so the moved P8 routes
//! behave identically here. Kept in this crate instead of the kernel
//! on purpose: the kernel stays free of handler conveniences, and
//! these helpers are only a few lines each.
//!
//! `log_action` is the one deliberate deviation: the main Worker
//! reaches it through `wordless-cms`, which this Worker does not
//! depend on. The local copy performs the same best-effort
//! `wl_audit_log` insert (same columns, same values, failures
//! swallowed) — the entry it would have read back is discarded by
//! every caller, so nothing observable changes.

use serde::Serialize;
use serde_json::Value;
use wordless_db::options::get_option;
use wordless_db::{Db, Param};
use wordless_kernel::envelope::RouteResponse;
use wordless_kernel::error::{bad_request, WordlessError};
use wordless_kernel::router::{HandlerResult, RequestContext};
use wordless_kernel::time::now_iso;
use wordless_kernel::validate::int_in_range;

use crate::ctx::Ctx;

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

pub(crate) fn opt_str_array(body: &Value, field: &str) -> Option<Vec<String>> {
    body.get(field).and_then(|v| v.as_array()).map(|a| {
        a.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect()
    })
}

/// Best-effort activity log (v1 `log()` swallows errors; never breaks
/// the response). Same insert as `wordless-cms::activity::log`.
pub(crate) async fn log_action(ctx: &Ctx, action: &str, object_type: &str, object_id: i64) {
    let _ = ctx
        .db
        .prepare(
            "INSERT INTO wl_audit_log (user_id, action, object_type, object_id, details, ip_hash, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(vec![
            Param::Integer(actor(ctx)),
            Param::Text(action.to_string()),
            Param::from(object_type.to_string()),
            Param::from(object_id.to_string()),
            Param::from(None::<String>),
            Param::from(None::<String>),
            Param::Text(now_iso()),
        ])
        .run()
        .await;
}

/// Site URL (the `siteurl` option). Default `http://localhost:8787`.
/// Same rule as `wordless-render::feeds::site_url`.
pub(crate) async fn site_url(db: &dyn Db) -> Result<String, WordlessError> {
    let url: String = get_option(db, "siteurl", "http://localhost:8787".to_string())
        .await
        .map_err(WordlessError::from)?;
    Ok(url.trim_end_matches('/').to_string())
}
