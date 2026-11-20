//! Shared helpers for the CMS services: pagination clamping and
//! validation wrappers whose error messages match v1 byte for byte
//! (`src/kernel/validate.ts`).

use serde_json::json;
use wordless_kernel::error::{bad_request, WordlessError};

/// Page number: v1 uses `page >= 1 ? floor(page) : 1`.
pub(crate) fn page_of(page: Option<i64>) -> i64 {
    match page {
        Some(p) if p >= 1 => p,
        _ => 1,
    }
}

/// Per-page: v1 clamps to 1–100, default 20.
pub(crate) fn per_page_of(per_page: Option<i64>) -> i64 {
    match per_page {
        Some(p) => p.clamp(1, 100),
        None => 20,
    }
}

/// v1: `total === 0 ? 0 : Math.ceil(total / perPage)`.
/// (Stable form: `i64::div_ceil` is still unstable on this toolchain.)
pub(crate) fn total_pages(total: i64, per_page: i64) -> i64 {
    if total == 0 {
        0
    } else {
        (total + per_page - 1) / per_page
    }
}

/// v1 `requireString` on an already-typed `&str` (trims; min/max bounds).
pub(crate) fn require_str(
    value: &str,
    field: &str,
    min: usize,
    max: usize,
) -> Result<String, WordlessError> {
    wordless_kernel::validate::require_string(&json!(value), field, min, max)
}

/// v1 `optionalString` on an already-typed `Option<&str>` (no trim, max bound).
pub(crate) fn optional_str(
    value: Option<&str>,
    field: &str,
    max: usize,
) -> Result<Option<String>, WordlessError> {
    match value {
        None => Ok(None),
        Some(s) => {
            if s.len() > max {
                return Err(bad_request(&format!(
                    "Field \"{field}\" must be at most {max} characters."
                )));
            }
            Ok(Some(s.to_string()))
        }
    }
}

/// Read a `COUNT(*) AS total` row.
pub(crate) fn count_total(row: &Option<wordless_db::Row>) -> i64 {
    match row {
        Some(r) => wordless_db::row::row_int(r, "total"),
        None => 0,
    }
}
