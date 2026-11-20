//! Delivery: sign, POST, record, retry.
//!
//! The HTTP call goes through the `HttpPost` trait so tests use a
//! fake and the worker implements it with `worker::Fetch` (the only
//! HTTP client available in Workers — `reqwest` does not work there).

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use wordless_db::row::row_text;
use wordless_db::{Db, Param, Row};
use wordless_kernel::time::{iso_from_ms, now_iso, now_ms};
use wordless_kernel::WordlessError;

use crate::subscriptions::is_allowed_event;

/// One HTTP POST. Returns the response status code.
#[async_trait(?Send)]
pub trait HttpPost {
    async fn post(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<u16, String>;
}

type HmacSha256 = Hmac<Sha256>;

/// `sha256=<hex HMAC-SHA256(body)>` — the scheme subscribers verify.
pub fn sign_payload(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC key must be usable");
    mac.update(body);
    let out = mac.finalize().into_bytes();
    let hex: String = out.iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256={hex}")
}

const MAX_ATTEMPTS: i64 = 5;

/// Backoff: 60s * 2^attempts, capped at 1 hour.
pub fn next_retry_at(attempts: i64) -> String {
    let secs = 60u64.saturating_mul(1u64 << attempts.min(6));
    let secs = secs.min(3600);
    iso_from_ms(now_ms() + secs * 1000)
}

fn new_delivery_id() -> String {
    let ms = now_ms();
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).unwrap_or(());
    let rand = u64::from_be_bytes(bytes);
    format!("whd_{ms:x}_{rand:016x}")
}

struct PendingDelivery {
    id: String,
    subscription_id: String,
    url: String,
    secret: String,
}

/// Fire `event` to every active subscription for it. One delivery row
/// per subscription, attempted immediately. A delivery failure never
/// propagates — it is recorded and retried later.
pub async fn dispatch_event(
    db: &dyn Db,
    http: &dyn HttpPost,
    event: &str,
    payload: &Value,
) -> Result<Vec<String>, WordlessError> {
    if !is_allowed_event(event) {
        return Err(WordlessError::new(
            "bad_request",
            400,
            &format!("Unknown webhook event \"{event}\"."),
        ));
    }
    let body = serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string());
    let rows = db
        .prepare("SELECT id, url, secret, events FROM wl_webhook_subscriptions WHERE active = 1")
        .all::<Row>()
        .await?;
    let mut attempted = Vec::new();
    for row in &rows {
        let events: Vec<String> =
            serde_json::from_str(&row_text(row, "events")).unwrap_or_default();
        if !events.iter().any(|e| e == event) {
            continue;
        }
        let pending = PendingDelivery {
            id: new_delivery_id(),
            subscription_id: row_text(row, "id"),
            url: row_text(row, "url"),
            secret: row_text(row, "secret"),
        };
        let now = now_iso();
        db.prepare(
            "INSERT INTO wl_webhook_deliveries (id, subscription_id, event, payload, status, attempts, next_retry_at, response_status, created_at, updated_at) VALUES (?, ?, ?, ?, 'pending', 0, NULL, NULL, ?, ?)",
        )
        .bind(vec![
            Param::Text(pending.id.clone()),
            Param::Text(pending.subscription_id.clone()),
            Param::Text(event.to_string()),
            Param::Text(body.clone()),
            Param::Text(now.clone()),
            Param::Text(now),
        ])
        .run()
        .await?;
        attempt_delivery(db, http, &pending, event, body.as_bytes(), 0).await;
        attempted.push(pending.id);
    }
    Ok(attempted)
}

async fn attempt_delivery(
    db: &dyn Db,
    http: &dyn HttpPost,
    pending: &PendingDelivery,
    event: &str,
    body: &[u8],
    prior_attempts: i64,
) {
    let attempts = prior_attempts + 1;
    let signature = sign_payload(&pending.secret, body);
    let headers = vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        ("X-Wordless-Event".to_string(), event.to_string()),
        ("X-Wordless-Signature".to_string(), signature),
        ("X-Wordless-Delivery".to_string(), pending.id.clone()),
    ];
    let result = http.post(&pending.url, &headers, body).await;
    let now = now_iso();
    match result {
        Ok(status) if (200..300).contains(&status) => {
            let _ = db
                .prepare(
                    "UPDATE wl_webhook_deliveries SET status = 'sent', attempts = ?, response_status = ?, updated_at = ? WHERE id = ?",
                )
                .bind(vec![
                    Param::Integer(attempts),
                    Param::Integer(status as i64),
                    Param::Text(now),
                    Param::Text(pending.id.clone()),
                ])
                .run()
                .await;
        }
        Ok(status) => {
            record_failure(db, pending, attempts, Some(status)).await;
        }
        Err(_) => {
            record_failure(db, pending, attempts, None).await;
        }
    }
}

async fn record_failure(
    db: &dyn Db,
    pending: &PendingDelivery,
    attempts: i64,
    response_status: Option<u16>,
) {
    let now = now_iso();
    let (status, next) = if attempts >= MAX_ATTEMPTS {
        ("failed".to_string(), None)
    } else {
        ("failed".to_string(), Some(next_retry_at(attempts)))
    };
    let _ = db
        .prepare(
            "UPDATE wl_webhook_deliveries SET status = ?, attempts = ?, next_retry_at = ?, response_status = ?, updated_at = ? WHERE id = ?",
        )
        .bind(vec![
            Param::Text(status),
            Param::Integer(attempts),
            match next {
                Some(n) => Param::Text(n),
                None => Param::Null,
            },
            match response_status {
                Some(s) => Param::Integer(s as i64),
                None => Param::Null,
            },
            Param::Text(now),
            Param::Text(pending.id.clone()),
        ])
        .run()
        .await;
}

/// Re-attempt due failed deliveries (status failed, attempts < 5,
/// next_retry_at <= now). Returns the ids retried. Called from the
/// retry route; automatic sweeping is a P5 cron job, not claimed here.
pub async fn retry_due(db: &dyn Db, http: &dyn HttpPost) -> Result<Vec<String>, WordlessError> {
    let now = now_iso();
    let rows = db
        .prepare(
            "SELECT d.id, d.subscription_id, d.event, d.payload, d.attempts, s.url, s.secret FROM wl_webhook_deliveries d JOIN wl_webhook_subscriptions s ON s.id = d.subscription_id WHERE d.status = 'failed' AND d.attempts < ? AND d.next_retry_at IS NOT NULL AND d.next_retry_at <= ? ORDER BY d.next_retry_at ASC LIMIT 20",
        )
        .bind(vec![Param::Integer(MAX_ATTEMPTS), Param::Text(now)])
        .all::<Row>()
        .await?;
    let mut retried = Vec::new();
    for row in &rows {
        let pending = PendingDelivery {
            id: row_text(row, "id"),
            subscription_id: row_text(row, "subscription_id"),
            url: row_text(row, "url"),
            secret: row_text(row, "secret"),
        };
        let event = row_text(row, "event");
        let body = row_text(row, "payload");
        let attempts = wordless_db::row::row_int(row, "attempts");
        attempt_delivery(db, http, &pending, &event, body.as_bytes(), attempts).await;
        retried.push(pending.id);
    }
    Ok(retried)
}

/// Re-attempt one delivery by id (manual retry route). Returns true
/// when the delivery existed.
pub async fn retry_one(
    db: &dyn Db,
    http: &dyn HttpPost,
    delivery_id: &str,
) -> Result<bool, WordlessError> {
    let row = db
        .prepare(
            "SELECT d.id, d.subscription_id, d.event, d.payload, d.attempts, s.url, s.secret FROM wl_webhook_deliveries d JOIN wl_webhook_subscriptions s ON s.id = d.subscription_id WHERE d.id = ? LIMIT 1",
        )
        .bind(vec![Param::Text(delivery_id.to_string())])
        .first::<Row>()
        .await?;
    let Some(row) = row else {
        return Ok(false);
    };
    let pending = PendingDelivery {
        id: row_text(&row, "id"),
        subscription_id: row_text(&row, "subscription_id"),
        url: row_text(&row, "url"),
        secret: row_text(&row, "secret"),
    };
    let event = row_text(&row, "event");
    let body = row_text(&row, "payload");
    let attempts = wordless_db::row::row_int(&row, "attempts");
    attempt_delivery(db, http, &pending, &event, body.as_bytes(), attempts).await;
    Ok(true)
}
