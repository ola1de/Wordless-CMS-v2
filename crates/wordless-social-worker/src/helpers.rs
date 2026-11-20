//! Route-handler helpers, copied verbatim from the main Worker's
//! `routes.rs` (same shapes, same messages) so the moved P7 routes
//! behave identically here. Kept in this crate instead of the kernel
//! on purpose: the kernel stays free of handler conveniences, and
//! these helpers are only a few lines each.

use serde::Serialize;
use serde_json::Value;
use wordless_kernel::envelope::RouteResponse;
use wordless_kernel::error::{bad_request, WordlessError};
use wordless_kernel::router::{HandlerResult, RequestContext};
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
