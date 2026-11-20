//! Typed errors. Handlers translate `WordlessError` into the JSON
//! envelope; stack traces are never sent to clients.
//! (Port of v1 `src/kernel/errors.ts` — same codes, same statuses.)

use thiserror::Error;

#[derive(Debug, Error)]
#[error("{message}")]
pub struct WordlessError {
    pub code: String,
    pub status: u16,
    pub message: String,
}

impl WordlessError {
    pub fn new(code: &str, status: u16, message: &str) -> Self {
        Self {
            code: code.to_string(),
            status,
            message: message.to_string(),
        }
    }
}

pub fn bad_request(message: &str) -> WordlessError {
    WordlessError::new("bad_request", 400, message)
}

pub fn unauthorized(message: &str) -> WordlessError {
    WordlessError::new("unauthorized", 401, message)
}

pub fn forbidden(message: &str) -> WordlessError {
    WordlessError::new("forbidden", 403, message)
}

pub fn not_found(message: &str) -> WordlessError {
    WordlessError::new("not_found", 404, message)
}

pub fn conflict(message: &str, code: &str) -> WordlessError {
    WordlessError::new(code, 409, message)
}

pub fn internal_error(message: &str) -> WordlessError {
    WordlessError::new("internal_error", 500, message)
}
