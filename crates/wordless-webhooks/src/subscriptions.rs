//! Subscriptions + delivery log storage (`wl_webhook_subscriptions`,
//! `wl_webhook_deliveries`, migration 0011).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wordless_db::row::{row_int, row_opt_text, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::time::now_iso;
use wordless_kernel::{bad_request, WordlessError};

/// Events a subscription may ask for. Anything else is rejected at
/// subscribe time — a typo'd event name must never silently subscribe
/// to nothing.
pub const ALLOWED_EVENTS: &[&str] = &[
    "post.created",
    "post.updated",
    "post.published",
    "order.paid",
    "comment.created",
];

pub fn is_allowed_event(event: &str) -> bool {
    ALLOWED_EVENTS.contains(&event)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    pub id: String,
    pub url: String,
    pub events: Vec<String>,
    pub active: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Delivery {
    pub id: String,
    pub subscription_id: String,
    pub event: String,
    pub payload: Value,
    pub status: String,
    pub attempts: i64,
    pub next_retry_at: Option<String>,
    pub response_status: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

fn row_to_subscription(row: &Row) -> Subscription {
    let events: Vec<String> = serde_json::from_str(&row_text(row, "events")).unwrap_or_default();
    Subscription {
        id: row_text(row, "id"),
        url: row_text(row, "url"),
        events,
        active: row_int(row, "active") != 0,
        created_at: row_text(row, "created_at"),
    }
}

fn row_to_delivery(row: &Row) -> Delivery {
    let payload: Value = serde_json::from_str(&row_text(row, "payload")).unwrap_or(Value::Null);
    Delivery {
        id: row_text(row, "id"),
        subscription_id: row_text(row, "subscription_id"),
        event: row_text(row, "event"),
        payload,
        status: row_text(row, "status"),
        attempts: row_int(row, "attempts"),
        next_retry_at: row_opt_text(row, "next_retry_at"),
        response_status: {
            let v = row.get("response_status").cloned().unwrap_or(Value::Null);
            v.as_i64()
        },
        created_at: row_text(row, "created_at"),
        updated_at: row_text(row, "updated_at"),
    }
}

/// Validate a subscription URL: http(s) only, with a host. Anything
/// else (including `javascript:` and relative URLs) is rejected.
pub fn validate_url(url: &str) -> Result<(), WordlessError> {
    let lower = url.to_ascii_lowercase();
    let rest = if let Some(r) = lower.strip_prefix("https://") {
        r
    } else if let Some(r) = lower.strip_prefix("http://") {
        r
    } else {
        return Err(bad_request(
            "Webhook URL must start with http:// or https://.",
        ));
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() {
        return Err(bad_request("Webhook URL must have a host."));
    }
    Ok(())
}

/// Create a subscription. The secret is generated server-side (32
/// random hex chars) and returned once — it is the HMAC key for this
/// subscription's deliveries.
pub async fn create_subscription(
    db: &dyn Db,
    url: &str,
    events: &[String],
) -> Result<(Subscription, String), WordlessError> {
    validate_url(url)?;
    if events.is_empty() {
        return Err(bad_request("A subscription needs at least one event."));
    }
    for e in events {
        if !is_allowed_event(e) {
            return Err(bad_request(&format!("Unknown webhook event \"{e}\".")));
        }
    }
    let id = new_id();
    let secret = new_secret();
    let now = now_iso();
    db.prepare(
        "INSERT INTO wl_webhook_subscriptions (id, url, events, secret, active, created_at) VALUES (?, ?, ?, ?, 1, ?)",
    )
    .bind(vec![
        Param::Text(id.clone()),
        Param::Text(url.to_string()),
        Param::Text(serde_json::to_string(events).unwrap_or_else(|_| "[]".to_string())),
        Param::Text(secret.clone()),
        Param::Text(now.clone()),
    ])
    .run()
    .await?;
    Ok((
        Subscription {
            id,
            url: url.to_string(),
            events: events.to_vec(),
            active: true,
            created_at: now,
        },
        secret,
    ))
}

pub async fn list_subscriptions(db: &dyn Db) -> Result<Vec<Subscription>, WordlessError> {
    let rows = db
        .prepare("SELECT * FROM wl_webhook_subscriptions ORDER BY created_at DESC")
        .all::<Row>()
        .await?;
    Ok(rows.iter().map(row_to_subscription).collect())
}

pub async fn delete_subscription(db: &dyn Db, id: &str) -> Result<bool, WordlessError> {
    let meta = db
        .prepare("DELETE FROM wl_webhook_subscriptions WHERE id = ?")
        .bind(vec![Param::Text(id.to_string())])
        .run()
        .await?;
    Ok(meta.changes > 0)
}

pub async fn list_deliveries(
    db: &dyn Db,
    page: i64,
    per_page: i64,
) -> Result<(Vec<Delivery>, i64), WordlessError> {
    let page = page.max(1);
    let per_page = per_page.clamp(1, 100);
    let total_row = db
        .prepare("SELECT COUNT(*) AS n FROM wl_webhook_deliveries")
        .first::<Row>()
        .await?;
    let total = total_row
        .as_ref()
        .map(|r| wordless_db::row::row_int(r, "n"))
        .unwrap_or(0);
    let rows = db
        .prepare("SELECT * FROM wl_webhook_deliveries ORDER BY created_at DESC LIMIT ? OFFSET ?")
        .bind(vec![
            Param::Integer(per_page),
            Param::Integer((page - 1) * per_page),
        ])
        .all::<Row>()
        .await?;
    Ok((rows.iter().map(row_to_delivery).collect(), total))
}

/// The subscription secret — only the delivery code reads it, never
/// an API response.
pub async fn subscription_secret(db: &dyn Db, id: &str) -> Result<Option<String>, WordlessError> {
    let row = db
        .prepare("SELECT secret FROM wl_webhook_subscriptions WHERE id = ? LIMIT 1")
        .bind(vec![Param::Text(id.to_string())])
        .first::<Row>()
        .await?;
    Ok(row.map(|r| row_text(&r, "secret")))
}

fn new_id() -> String {
    // UUID-shaped id without a uuid crate: timestamp + 64 random bits.
    let ms = wordless_kernel::time::now_ms();
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).unwrap_or(());
    let rand = u64::from_be_bytes(bytes);
    format!("wh_{ms:x}_{rand:016x}")
}

fn new_secret() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).unwrap_or(());
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
