//! WORDLESS v2 commerce (P3).
//!
//! Port of the v1 module under `src/modules/commerce/` in the v1.1.0
//! TypeScript repo (read-only reference), plus
//! `src/modules/commerce/payments/` (Paystack, Flutterwave, Stripe,
//! mock). Same tables (migration `0008_m8.sql`: `wl_products`,
//! `wl_orders`, `wl_order_items`, `wl_carts`, `wl_webhook_log`), same
//! SQL, same validation messages, same webhook semantics.
//!
//! Modules: `types` (products / orders / cart shapes, camelCase wire
//! format), `money` (integer minor units only — never floats for
//! money; NGN default), `validate` (integer / slug validation and
//! `slugify`, byte-identical to v1), `ids` (UUID v4 ids from the
//! platform RNG), `products` (CRUD, unique slugs, delete refused when
//! the product appears in an order), `cart` (server-side cart keyed by
//! the `wordless_cart` cookie token, 7-day expiry), `orders` (create
//! from cart, status pipeline, stock decrement exactly once on paid /
//! restore on cancel), `payments` (HMAC helpers, the four provider
//! webhook verifiers, provider selection, the mock provider, and
//! `start_checkout`), `webhooks` (verified-webhook handling with
//! exactly-once paid transition and replay-as-duplicate), `shop`
//! (published-products data for `/shop` and `/shop/:slug`).
//!
//! Service functions take `db: &dyn Db` plus plain parameters and
//! return `Result<_, WordlessError>` (typed results); route handlers
//! live in `wordless-worker` and are wired by the P3 coordinator.
//! Webhook verification takes headers as `&[(&str, &str)]` pairs (the
//! worker converts its `Headers`); the Stripe verifier takes the
//! current Unix time explicitly so the 300 s tolerance is testable.
//!
//! Honesty notes (carried over from v1, same limits):
//! - The real providers' `createCheckout` HTTP calls (Paystack
//!   initialise, Flutterwave create payment, Stripe Checkout Session)
//!   need `fetch`, which only exists in the Worker. This crate defines
//!   the `CheckoutGateway` trait and implements it for the mock
//!   provider; the worker implements it for the real providers with
//!   `worker::Fetch`. Nothing here ever marks an order paid without a
//!   verified webhook signal.
//! - `start_checkout` returns the typed `payment_not_configured`
//!   error (HTTP 503) when no provider is selected or the selected
//!   one is unconfigured — never a fake success. The pending order
//!   created before resolution stays pending with an empty
//!   `payment_reference`, exactly as in v1 (an empty-reference webhook
//!   can never pay it — see `orders::get_order_by_reference`).
//! - The order-confirmation email v1 sends from the webhook handler
//!   lives in the integrations crate (`wordless-integrations`, P3):
//!   `handle_payment_webhook` marks the order paid and logs the
//!   attempt; the worker sends the mail best-effort afterwards. A mail
//!   failure never fails a webhook.
//! - `PaymentSecrets` holds secret *values* passed in by the worker
//!   from env/secrets bindings. Secrets are never stored in D1, never
//!   logged, and never appear in any `WebhookLogEntry` or error.
//! - WASM size: this crate adds only `hmac` + `sha2` (already in the
//!   workspace for other crates) and `unicode-normalization` (already
//!   pulled in by `wordless-cms`, so zero marginal cost in the worker
//!   binary). No regex, no HTTP client, no new crypto.

pub mod cart;
pub mod ids;
pub mod money;
pub mod orders;
pub mod payments;
pub mod products;
pub mod shop;
pub mod types;
pub mod validate;
pub(crate) mod variation;
pub mod webhooks;

#[cfg(test)]
mod test_support;

pub use orders::OrderTotals;
pub use types::{
    Cart, CartItem, CartLine, Order, OrderItem, OrderList, Product, ProductInput, ProductList,
};
