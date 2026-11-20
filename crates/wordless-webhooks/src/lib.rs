//! WORDLESS v2 outbound webhooks — event subscriptions, HMAC-signed
//! deliveries, retry with backoff, delivery log.
//!
//! NEW in P3 (no v1 equivalent — see doc §0.2 module #10, §3 P3).
//! v1 only had *inbound* payment-provider webhooks; this is the
//! outbound event system: a subscriber registers a URL + the events
//! it wants, and the worker POSTs a signed JSON payload on each event.
//!
//! Signature: `X-Wordless-Signature: sha256=<hex HMAC-SHA256>` over
//! the raw request body, keyed by the subscription secret (same scheme
//! as the mock payment provider). A subscriber verifies the signature
//! and replays are detectable via the delivery id.
//!
//! Delivery: one attempt per event per subscription, synchronously in
//! the request that fired the event (best-effort — a delivery failure
//! never breaks the response that caused it). Failures are recorded
//! with `next_retry_at = now + 60s * 2^attempts` (capped at 1 h) and
//! up to 5 attempts; `retry_due` re-attempts due deliveries (called
//! from the retry route; automatic sweeping needs the P5 cron wiring
//! and is documented as such, not claimed).
//!
//! Events (allowlist — anything else is rejected at subscribe time):
//! `post.created`, `post.updated`, `post.published`, `order.paid`,
//! `comment.created`.

pub mod deliver;
pub mod subscriptions;

pub use deliver::{dispatch_event, retry_due, retry_one, sign_payload, HttpPost};
pub use subscriptions::{
    create_subscription, delete_subscription, list_deliveries, list_subscriptions, Delivery,
    Subscription, ALLOWED_EVENTS,
};

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests;
