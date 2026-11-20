//! WORDLESS v2 GraphQL — a query endpoint over the same content model
//! as REST (posts, terms, media, users, options), authenticated the
//! same way, with the same capability guards.
//!
//! NEW in P3 (no v1 equivalent — see doc §0.2 module #10, §3 P3).
//! Queries only; there are no mutations and no subscriptions. The
//! parser is hand-rolled (`parser.rs`) — no GraphQL dependency, both
//! for WASM size and because no GraphQL crate is available offline in
//! this VM. Unsupported constructs (mutations, subscriptions,
//! directives) are rejected with a clear error, never silently
//! ignored. There is no introspection query; the schema is the
//! `SCHEMA` constant in `executor.rs`.
//!
//! Done-when (doc §3 P3): a GraphQL query returns the same post a REST
//! call returns, under the same capability guard. Guards mirror the
//! REST routes exactly — see `executor.rs`.

pub mod executor;
pub mod parser;

pub use executor::{execute, GraphqlOutcome, SCHEMA};

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests;
