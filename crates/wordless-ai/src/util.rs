//! Small shared helpers: UUIDs and plain-string validation.
//!
//! UUIDs use the kernel's `random_bytes` (getrandom, `js` feature on
//! WASM) with the same v4 bit-twiddling as `wordless-auth`'s session
//! ids, so task and approval ids are unique without a database round
//! trip.

use wordless_kernel::error::{bad_request, internal_error, WordlessError};

/// A random UUID v4, formatted `xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx`.
pub fn new_uuid_v4() -> Result<String, WordlessError> {
    let mut b = wordless_kernel::crypto::random_bytes(16)
        .map_err(|e| internal_error(&format!("random uuid failed: {e}")))?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13],
        b[14], b[15]
    ))
}

/// Require a non-empty string of at most `max` characters (v1's
/// `requireString` for plain `&str` inputs).
pub fn require_str(value: &str, field: &str, max: usize) -> Result<String, WordlessError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(bad_request(&format!("Field \"{field}\" is required.")));
    }
    if value.len() > max {
        return Err(bad_request(&format!(
            "Field \"{field}\" must be at most {max} characters."
        )));
    }
    Ok(value.to_string())
}

/// Optional string with a length cap (v1's `optionalPayloadString` for
/// `&str` inputs).
pub fn optional_str(
    value: Option<&str>,
    field: &str,
    max: usize,
) -> Result<Option<String>, WordlessError> {
    match value {
        None => Ok(None),
        Some(v) => {
            if v.len() > max {
                return Err(bad_request(&format!(
                    "Field \"{field}\" must be at most {max} characters."
                )));
            }
            Ok(Some(v.to_string()))
        }
    }
}

/// Read a required string field out of a proposal payload object.
pub fn payload_string(
    payload: &serde_json::Value,
    field: &str,
    max: usize,
) -> Result<String, WordlessError> {
    match payload.get(field) {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => {
            if s.len() > max {
                return Err(bad_request(&format!(
                    "Proposal payload field \"{field}\" is too long."
                )));
            }
            Ok(s.clone())
        }
        _ => Err(bad_request(&format!(
            "Proposal payload is missing a valid \"{field}\"."
        ))),
    }
}

/// Read an optional string field out of a proposal payload object.
pub fn payload_optional_string(
    payload: &serde_json::Value,
    field: &str,
    max: usize,
) -> Result<Option<String>, WordlessError> {
    match payload.get(field) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => {
            if s.len() > max {
                return Err(bad_request(&format!(
                    "Proposal payload field \"{field}\" is too long."
                )));
            }
            Ok(Some(s.clone()))
        }
        Some(_) => Err(bad_request(&format!(
            "Proposal payload field \"{field}\" must be a string."
        ))),
    }
}

/// Read a required integer post id out of a proposal payload object.
pub fn payload_post_id(payload: &serde_json::Value) -> Result<i64, WordlessError> {
    match payload.get("postId") {
        Some(serde_json::Value::Number(n)) => match n.as_i64() {
            Some(id) if id >= 1 => Ok(id),
            _ => Err(bad_request(
                "Proposal payload is missing a valid \"postId\".",
            )),
        },
        _ => Err(bad_request(
            "Proposal payload is missing a valid \"postId\".",
        )),
    }
}
