//! Webhook tests: exactly-once dispatch per event, verifiable
//! signature, retry with backoff.

use std::cell::RefCell;
use std::rc::Rc;

use async_trait::async_trait;
use futures_executor::block_on;
use serde_json::json;

use crate::deliver::{dispatch_event, retry_due, retry_one, sign_payload, HttpPost};
use crate::subscriptions::{
    create_subscription, delete_subscription, list_deliveries, list_subscriptions,
};
use crate::test_support::SqliteDb;

/// Fake HTTP client: records requests, answers with a configured status.
struct FakeHttp {
    status: u16,
    fail: bool,
    requests: Rc<RefCell<Vec<(String, Vec<(String, String)>, Vec<u8>)>>>,
}

impl FakeHttp {
    fn ok(
        status: u16,
    ) -> (
        Self,
        Rc<RefCell<Vec<(String, Vec<(String, String)>, Vec<u8>)>>>,
    ) {
        let requests = Rc::new(RefCell::new(Vec::new()));
        (
            Self {
                status,
                fail: false,
                requests: requests.clone(),
            },
            requests,
        )
    }

    fn failing() -> (
        Self,
        Rc<RefCell<Vec<(String, Vec<(String, String)>, Vec<u8>)>>>,
    ) {
        let requests = Rc::new(RefCell::new(Vec::new()));
        (
            Self {
                status: 0,
                fail: true,
                requests: requests.clone(),
            },
            requests,
        )
    }
}

#[async_trait(?Send)]
impl HttpPost for FakeHttp {
    async fn post(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<u16, String> {
        self.requests
            .borrow_mut()
            .push((url.to_string(), headers.to_vec(), body.to_vec()));
        if self.fail {
            Err("connection refused".to_string())
        } else {
            Ok(self.status)
        }
    }
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

#[test]
fn dispatch_fires_exactly_once_with_verifiable_signature() {
    let db = SqliteDb::new();
    let (sub, secret) = block_on(create_subscription(
        &db,
        "https://example.test/hook",
        &["post.published".to_string()],
    ))
    .unwrap();
    let (http, requests) = FakeHttp::ok(200);
    let ids = block_on(dispatch_event(
        &db,
        &http,
        "post.published",
        &json!({ "id": 7, "slug": "hello-world" }),
    ))
    .unwrap();
    assert_eq!(ids.len(), 1, "one delivery per subscription");
    assert_eq!(requests.borrow().len(), 1);
    let (url, headers, body) = &requests.borrow()[0];
    assert_eq!(url, "https://example.test/hook");
    assert_eq!(header(headers, "X-Wordless-Event"), Some("post.published"));
    // The signature the subscriber receives must verify against the
    // secret with the same scheme.
    let sig = header(headers, "X-Wordless-Signature").unwrap();
    assert_eq!(sig, sign_payload(&secret, body));
    assert_eq!(
        header(headers, "X-Wordless-Delivery"),
        Some(ids[0].as_str())
    );

    // The delivery log records it as sent.
    let (deliveries, total) = block_on(list_deliveries(&db, 1, 20)).unwrap();
    assert_eq!(total, 1);
    assert_eq!(deliveries[0].status, "sent");
    assert_eq!(deliveries[0].subscription_id, sub.id);
    assert_eq!(deliveries[0].attempts, 1);
}

#[test]
fn dispatch_skips_subscriptions_for_other_events() {
    let db = SqliteDb::new();
    block_on(create_subscription(
        &db,
        "https://example.test/hook",
        &["order.paid".to_string()],
    ))
    .unwrap();
    let (http, requests) = FakeHttp::ok(200);
    let ids = block_on(dispatch_event(
        &db,
        &http,
        "post.published",
        &json!({ "id": 1 }),
    ))
    .unwrap();
    assert!(ids.is_empty());
    assert!(requests.borrow().is_empty());
}

#[test]
fn failed_delivery_records_backoff_and_retries() {
    let db = SqliteDb::new();
    block_on(create_subscription(
        &db,
        "https://example.test/hook",
        &["post.created".to_string()],
    ))
    .unwrap();
    let (failing, _) = FakeHttp::failing();
    let ids = block_on(dispatch_event(
        &db,
        &failing,
        "post.created",
        &json!({ "id": 2 }),
    ))
    .unwrap();
    assert_eq!(ids.len(), 1);
    let (deliveries, _) = block_on(list_deliveries(&db, 1, 20)).unwrap();
    assert_eq!(deliveries[0].status, "failed");
    assert_eq!(deliveries[0].attempts, 1);
    assert!(deliveries[0].next_retry_at.is_some(), "backoff scheduled");

    // Not due yet (next_retry_at is in the future) — retry_due finds nothing.
    let (http, _) = FakeHttp::ok(200);
    let retried = block_on(retry_due(&db, &http)).unwrap();
    assert!(retried.is_empty());

    // Manual retry works and marks it sent.
    let ok = block_on(retry_one(&db, &http, &ids[0])).unwrap();
    assert!(ok);
    let (deliveries, _) = block_on(list_deliveries(&db, 1, 20)).unwrap();
    assert_eq!(deliveries[0].status, "sent");
    assert_eq!(deliveries[0].attempts, 2);
}

#[test]
fn subscription_validation_rejects_bad_urls_and_events() {
    let db = SqliteDb::new();
    assert!(block_on(create_subscription(
        &db,
        "javascript:alert(1)",
        &["post.created".to_string()]
    ))
    .is_err());
    assert!(block_on(create_subscription(&db, "https://example.test/h", &[])).is_err());
    assert!(block_on(create_subscription(
        &db,
        "https://example.test/h",
        &["post.exploded".to_string()]
    ))
    .is_err());
    let (sub, _) = block_on(create_subscription(
        &db,
        "https://example.test/h",
        &["comment.created".to_string()],
    ))
    .unwrap();
    let subs = block_on(list_subscriptions(&db)).unwrap();
    assert_eq!(subs.len(), 1);
    assert!(block_on(delete_subscription(&db, &sub.id)).unwrap());
    assert!(block_on(list_subscriptions(&db)).unwrap().is_empty());
}

#[test]
fn signature_is_stable_hmac_sha256() {
    // Known vector cross-checked against Python hmac.new(b"key",
    // b"hello", hashlib.sha256).hexdigest().
    let sig = sign_payload("key", b"hello");
    assert_eq!(
        sig,
        "sha256=9307b3b915efb5171ff14d8cb55fbcc798c6c0ef1456d66ded1a6aa723a58b7b"
    );
}
