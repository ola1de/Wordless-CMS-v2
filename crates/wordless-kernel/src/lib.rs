//! WORDLESS v2 kernel — pure Rust, no Worker types.
//!
//! Router, JSON envelope, typed errors, validation, roles/capabilities,
//! base64url, time, platform crypto, and security headers. Everything
//! here runs under plain `cargo test` on native targets; the only
//! platform-dependent pieces are `crypto` (WebCrypto on wasm32, pure
//! Rust elsewhere) and `time` (platform clock), both behind tiny
//! functions with identical signatures on every target.

pub mod base64url;
pub mod capabilities;
pub mod converter;
pub mod crypto;
pub mod envelope;
pub mod error;
pub mod html;
pub mod router;
pub mod security;
pub mod storage;
pub mod time;
pub mod validate;
pub mod woo_client;

pub use capabilities::{capabilities_for, is_role, user_can, AuthUser, Role, ALL_ROLES};
pub use envelope::RouteResponse;
pub use error::{
    bad_request, conflict, forbidden, internal_error, not_found, unauthorized, WordlessError,
};
pub use router::{RequestContext, RouteOptions, Router};
