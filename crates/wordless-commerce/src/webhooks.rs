//! Payment webhooks (port of v1 `src/modules/commerce/webhooks.ts`).
//!
//! `POST /api/commerce/webhooks/:provider` — no session auth; the
//! provider signature IS the auth. Every attempt is logged in
//! `wl_webhook_log` (provider, event, reference, result) — never the
//! secret, never the raw body. A valid "paid" event transitions the
//! order pending → paid exactly once: a replayed webhook finds the
//! order already paid and is logged as a duplicate, never re-applied.
//! Unknown references and cancelled orders are answered 200 (so the
//! provider stops retrying) with a body that says what happened.
//!
//! The order-confirmation email v1 sends here is best-effort and lives
//! in the integrations crate: this handler marks the order paid and
//! logs the attempt; the worker sends the mail afterwards. A mail
//! failure never fails a webhook.

use serde::Serialize;
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::{bad_request, WordlessError};
use wordless_kernel::time::now_iso;

use crate::orders::{get_order_by_reference, transition_order};
use crate::payments::{verify_webhook, PaymentSecrets};
use crate::types::{ORDER_STATUS_CANCELLED, ORDER_STATUS_FULFILLED, ORDER_STATUS_PAID};

/// What happened to the order a webhook named (wire format is the
/// snake_case string, as in v1's `WebhookOutcome.order`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookOrderOutcome {
    Paid,
    Duplicate,
    NotFound,
    Cancelled,
    NotPaidEvent,
}

/// The outcome of handling a payment webhook (v1 `WebhookOutcome`).
/// `http_status` 401 means the signature was rejected — the worker
/// maps it to the 401 error envelope; every other outcome is 200.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookOutcome {
    pub http_status: u16,
    pub received: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<WebhookOrderOutcome>,
    pub detail: String,
}

/// One row of the webhook log (admin view — never a secret).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookLogEntry {
    pub id: i64,
    pub provider: String,
    pub event: String,
    pub reference: String,
    pub result: String,
    pub detail: String,
    pub created_at: String,
}

/// A paginated webhook-log list.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookLogList {
    pub entries: Vec<WebhookLogEntry>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

async fn log_webhook(
    db: &dyn Db,
    provider: &str,
    event: &str,
    reference: Option<&str>,
    result: &str,
    detail: &str,
) {
    // Webhook logging must never break the webhook response.
    let detail: String = detail.chars().take(500).collect();
    let _ = db
        .prepare(
            "INSERT INTO wl_webhook_log (provider, event, reference, result, detail, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(vec![
            Param::Text(provider.to_string()),
            Param::Text(event.to_string()),
            Param::Text(reference.unwrap_or("").to_string()),
            Param::Text(result.to_string()),
            Param::Text(detail),
            Param::Text(now_iso()),
        ])
        .run()
        .await;
}

/// Recent webhook attempts (admin only) — provider, event, reference,
/// result. Never a secret. Newest first; page ≥ 1, per-page clamped
/// 1–100, default 20.
pub async fn list_webhook_log(
    db: &dyn Db,
    page: i64,
    per_page: i64,
) -> Result<WebhookLogList, WordlessError> {
    let page = page.max(1);
    let per_page = per_page.clamp(1, 100);
    let offset = (page - 1) * per_page;
    let total_row: Option<Row> = db
        .prepare("SELECT COUNT(*) AS n FROM wl_webhook_log")
        .bind(vec![])
        .first()
        .await?;
    let total = total_row.as_ref().map(|r| row_int(r, "n")).unwrap_or(0);
    let rows = db
        .prepare("SELECT * FROM wl_webhook_log ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?")
        .bind(vec![Param::Integer(per_page), Param::Integer(offset)])
        .all::<Row>()
        .await?;
    Ok(WebhookLogList {
        entries: rows
            .iter()
            .map(|r| WebhookLogEntry {
                id: row_int(r, "id"),
                provider: row_text(r, "provider"),
                event: row_text(r, "event"),
                reference: row_text(r, "reference"),
                result: row_text(r, "result"),
                detail: row_text(r, "detail"),
                created_at: row_text(r, "created_at"),
            })
            .collect(),
        total,
        page,
        per_page,
        total_pages: ((total + per_page - 1) / per_page).max(1),
    })
}

/// Handle a payment webhook: verify the signature, then apply a paid
/// event to the matching order exactly once.
///
/// - Unknown provider id → 400 (`Unknown payment provider "…"`).
/// - Bad / missing signature → outcome `http_status` 401, order left
///   untouched (the worker maps this to the 401 envelope).
/// - Valid signature but not a paid event → 200, `not_paid_event`.
/// - Valid paid event, unknown / empty reference → 200, `not_found`.
/// - Valid paid event, order already paid/fulfilled → 200, `duplicate`
///   (replay ignored; stock is NOT decremented again).
/// - Valid paid event, order cancelled → 200, `cancelled`.
/// - Valid paid event, order pending → 200, `paid` (stock decremented
///   exactly once by the transition).
pub async fn handle_payment_webhook(
    db: &dyn Db,
    secrets: &PaymentSecrets,
    provider_id: &str,
    raw_body: &str,
    headers: &[(&str, &str)],
    now_unix_secs: i64,
) -> Result<WebhookOutcome, WordlessError> {
    if !crate::payments::is_payment_provider_id(provider_id) {
        return Err(bad_request(&format!(
            "Unknown payment provider \"{provider_id}\"."
        )));
    }
    let result = verify_webhook(provider_id, secrets, raw_body, headers, now_unix_secs)?;
    if !result.valid {
        log_webhook(
            db,
            provider_id,
            &result.event,
            result.reference.as_deref(),
            "rejected",
            &result.detail,
        )
        .await;
        return Ok(WebhookOutcome {
            http_status: 401,
            received: false,
            order: None,
            detail: result.detail,
        });
    }
    if !result.paid {
        log_webhook(
            db,
            provider_id,
            &result.event,
            result.reference.as_deref(),
            "ignored",
            &result.detail,
        )
        .await;
        return Ok(WebhookOutcome {
            http_status: 200,
            received: true,
            order: Some(WebhookOrderOutcome::NotPaidEvent),
            detail: result.detail,
        });
    }
    let Some(reference) = result.reference.clone().filter(|r| !r.is_empty()) else {
        log_webhook(
            db,
            provider_id,
            &result.event,
            None,
            "error",
            "Verified webhook carried no reference.",
        )
        .await;
        return Ok(WebhookOutcome {
            http_status: 200,
            received: true,
            order: Some(WebhookOrderOutcome::NotFound),
            detail: "Verified webhook carried no reference.".to_string(),
        });
    };
    let order = get_order_by_reference(db, &reference).await?;
    let Some(order) = order else {
        log_webhook(
            db,
            provider_id,
            &result.event,
            Some(&reference),
            "error",
            &format!("No order matches reference \"{reference}\"."),
        )
        .await;
        return Ok(WebhookOutcome {
            http_status: 200,
            received: true,
            order: Some(WebhookOrderOutcome::NotFound),
            detail: format!("No order matches reference \"{reference}\"."),
        });
    };
    if order.status == ORDER_STATUS_PAID || order.status == ORDER_STATUS_FULFILLED {
        log_webhook(
            db,
            provider_id,
            &result.event,
            Some(&reference),
            "duplicate",
            &format!(
                "Order {} is already {}; replay ignored.",
                order.id, order.status
            ),
        )
        .await;
        return Ok(WebhookOutcome {
            http_status: 200,
            received: true,
            order: Some(WebhookOrderOutcome::Duplicate),
            detail: format!("Order {} is already {}.", order.id, order.status),
        });
    }
    if order.status == ORDER_STATUS_CANCELLED {
        log_webhook(
            db,
            provider_id,
            &result.event,
            Some(&reference),
            "ignored",
            &format!("Order {} is cancelled; payment not applied.", order.id),
        )
        .await;
        return Ok(WebhookOutcome {
            http_status: 200,
            received: true,
            order: Some(WebhookOrderOutcome::Cancelled),
            detail: format!("Order {} is cancelled.", order.id),
        });
    }
    match transition_order(db, &order.id, ORDER_STATUS_PAID).await {
        Ok(paid) => {
            log_webhook(
                db,
                provider_id,
                &result.event,
                Some(&reference),
                "accepted",
                &format!("Order {} marked paid.", paid.id),
            )
            .await;
            Ok(WebhookOutcome {
                http_status: 200,
                received: true,
                order: Some(WebhookOrderOutcome::Paid),
                detail: format!("Order {} marked paid.", paid.id),
            })
        }
        Err(err) => {
            log_webhook(
                db,
                provider_id,
                &result.event,
                Some(&reference),
                "error",
                &err.message,
            )
            .await;
            Ok(WebhookOutcome {
                http_status: 200,
                received: true,
                order: Some(WebhookOrderOutcome::NotFound),
                detail: err.message,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cart::{add_to_cart, new_cart_token};
    use crate::orders::{
        create_order_from_cart, get_order, get_order_by_reference, set_order_payment,
    };
    use crate::payments::{hmac_hex_sha256, MockCheckoutGateway, PaymentSecrets};
    use crate::products::{create_product, get_product};
    use crate::test_support::SqliteDb;
    use crate::types::ProductInput;
    use futures_executor::block_on;
    use serde_json::json;

    const MOCK_SECRET: &str = "test-only-mock-webhook-secret";

    fn mock_secrets() -> PaymentSecrets {
        PaymentSecrets {
            mock_webhook_secret: Some(MOCK_SECRET.to_string()),
            ..Default::default()
        }
    }

    fn make_product(db: &SqliteDb, stock: i64) -> crate::types::Product {
        block_on(create_product(
            db,
            ProductInput {
                name: Some(json!("Pack Hoodie")),
                price_minor: Some(json!(125000)),
                stock: Some(json!(stock)),
                status: Some(json!("published")),
                ..Default::default()
            },
        ))
        .unwrap()
    }

    /// A pending order with a mock checkout reference, as
    /// `start_checkout` would leave it.
    fn make_paid_ready_order(db: &SqliteDb) -> (String, i64, String) {
        let product = make_product(db, 5);
        let token = new_cart_token().unwrap();
        block_on(add_to_cart(db, &token, product.id, &json!(2))).unwrap();
        let order = block_on(create_order_from_cart(
            db,
            &token,
            &json!("Ada"),
            &json!("ada@example.test"),
        ))
        .unwrap();
        let reference = format!("mock_{}", order.id);
        block_on(set_order_payment(db, &order.id, "mock", &reference)).unwrap();
        (order.id, product.id, reference)
    }

    fn post_mock_webhook(
        db: &SqliteDb,
        secrets: &PaymentSecrets,
        reference: &str,
        secret: &str,
    ) -> WebhookOutcome {
        let body = format!(r#"{{"event":"payment.paid","reference":"{reference}"}}"#);
        let signature = hmac_hex_sha256(secret, &body);
        block_on(handle_payment_webhook(
            db,
            secrets,
            "mock",
            &body,
            &[("x-mock-signature", signature.as_str())],
            1_795_200_000,
        ))
        .unwrap()
    }

    #[test]
    fn mock_webhook_marks_order_paid_with_valid_signature() {
        let db = SqliteDb::new();
        let (order_id, _product_id, reference) = make_paid_ready_order(&db);
        let outcome = post_mock_webhook(&db, &mock_secrets(), &reference, MOCK_SECRET);
        assert_eq!(outcome.http_status, 200);
        assert_eq!(outcome.order, Some(WebhookOrderOutcome::Paid));
        let order = block_on(get_order(&db, &order_id)).unwrap().unwrap();
        assert_eq!(order.status, "paid");
        assert!(order.stock_applied);
        // The mock's verifyPayment now reads the accepted webhook log.
        let verified = block_on(crate::payments::mock_verify_payment(&db, &reference)).unwrap();
        assert_eq!(verified.status, "paid");
    }

    #[test]
    fn bad_signature_is_401_and_leaves_order_pending() {
        let db = SqliteDb::new();
        let (order_id, _product_id, reference) = make_paid_ready_order(&db);
        let outcome = post_mock_webhook(&db, &mock_secrets(), &reference, "wrong-secret");
        assert_eq!(outcome.http_status, 401);
        assert!(!outcome.received);
        assert_eq!(
            block_on(get_order(&db, &order_id)).unwrap().unwrap().status,
            "pending"
        );
        // And with no mock secret configured at all: also 401.
        let outcome = post_mock_webhook(&db, &PaymentSecrets::empty(), &reference, MOCK_SECRET);
        assert_eq!(outcome.http_status, 401);
        assert_eq!(
            block_on(get_order(&db, &order_id)).unwrap().unwrap().status,
            "pending"
        );
    }

    #[test]
    fn replayed_webhook_is_duplicate_and_does_not_double_pay_or_decrement() {
        let db = SqliteDb::new();
        let (order_id, product_id, reference) = make_paid_ready_order(&db);
        let first = post_mock_webhook(&db, &mock_secrets(), &reference, MOCK_SECRET);
        assert_eq!(first.order, Some(WebhookOrderOutcome::Paid));
        let replay = post_mock_webhook(&db, &mock_secrets(), &reference, MOCK_SECRET);
        assert_eq!(replay.http_status, 200);
        assert_eq!(replay.order, Some(WebhookOrderOutcome::Duplicate));
        assert_eq!(
            block_on(get_order(&db, &order_id)).unwrap().unwrap().status,
            "paid"
        );
        assert_eq!(
            block_on(get_product(&db, product_id))
                .unwrap()
                .unwrap()
                .stock,
            3
        ); // 5 - 2, once
        let log = block_on(list_webhook_log(&db, 1, 20)).unwrap();
        assert_eq!(
            log.entries
                .iter()
                .filter(|e| e.result == "duplicate")
                .count(),
            1
        );
        assert_eq!(
            log.entries
                .iter()
                .filter(|e| e.result == "accepted")
                .count(),
            1
        );
    }

    #[test]
    fn unknown_reference_answers_200_not_found() {
        let db = SqliteDb::new();
        let outcome = post_mock_webhook(&db, &mock_secrets(), "mock_does-not-exist", MOCK_SECRET);
        assert_eq!(outcome.http_status, 200);
        assert_eq!(outcome.order, Some(WebhookOrderOutcome::NotFound));
    }

    #[test]
    fn empty_reference_webhook_cannot_pay_order_with_empty_reference() {
        let db = SqliteDb::new();
        let product = make_product(&db, 5);
        let token = new_cart_token().unwrap();
        block_on(add_to_cart(&db, &token, product.id, &json!(1))).unwrap();
        let pending = block_on(create_order_from_cart(
            &db,
            &token,
            &json!("No Provider"),
            &json!("np@example.test"),
        ))
        .unwrap();
        assert_eq!(pending.payment_reference, "");
        assert!(block_on(get_order_by_reference(&db, "")).unwrap().is_none());
        let outcome = post_mock_webhook(&db, &mock_secrets(), "", MOCK_SECRET);
        assert_eq!(outcome.http_status, 200);
        assert_eq!(outcome.order, Some(WebhookOrderOutcome::NotFound));
        assert_eq!(
            block_on(get_order(&db, &pending.id))
                .unwrap()
                .unwrap()
                .status,
            "pending"
        );
        assert_eq!(
            block_on(get_product(&db, product.id))
                .unwrap()
                .unwrap()
                .stock,
            5
        );
    }

    #[test]
    fn unknown_provider_is_400() {
        let db = SqliteDb::new();
        let err = block_on(handle_payment_webhook(
            &db,
            &mock_secrets(),
            "nope",
            "{}",
            &[],
            0,
        ))
        .unwrap_err();
        assert_eq!(err.status, 400);
    }

    #[test]
    fn mock_gateway_marks_paid_exactly_once_end_to_end() {
        // Full DONE-WHEN flow: mock checkout + correctly-signed webhook.
        let db = SqliteDb::new();
        block_on(crate::payments::set_selected_provider_id(&db, Some("mock"))).unwrap();
        let product = make_product(&db, 5);
        let token = new_cart_token().unwrap();
        block_on(add_to_cart(&db, &token, product.id, &json!(2))).unwrap();
        let gateway = MockCheckoutGateway;
        let checkout = block_on(crate::payments::start_checkout(
            &db,
            &mock_secrets(),
            &gateway,
            &token,
            &json!("Ada"),
            &json!("ada@example.test"),
            "http://localhost:8787",
            "http://localhost:8787/shop/checkout/success",
            "http://localhost:8787/shop/checkout/cancelled",
        ))
        .unwrap();
        let outcome = post_mock_webhook(&db, &mock_secrets(), &checkout.reference, MOCK_SECRET);
        assert_eq!(outcome.order, Some(WebhookOrderOutcome::Paid));
        assert_eq!(
            block_on(get_order(&db, &checkout.order.id))
                .unwrap()
                .unwrap()
                .status,
            "paid"
        );
        assert_eq!(
            block_on(get_product(&db, product.id))
                .unwrap()
                .unwrap()
                .stock,
            3
        );
    }
}
