//! Email (port of v1 `email.ts`).
//!
//! Two providers, one interface:
//! * `"http"`: a generic Resend/SendGrid-shaped HTTP endpoint — POST
//!   JSON `{ from, to, subject, text, html }` with
//!   `Authorization: Bearer <EMAIL_API_KEY>` to `EMAIL_API_URL`
//!   (the key comes only from env/secrets; the URL is non-secret
//!   config `wl_integration_email.apiUrl`). Outcomes (sent/failed)
//!   are recorded in `wl_mail_log`.
//! * `"mock"`: no provider is configured — the message is recorded
//!   in `wl_mail_log` with status `"logged"` and `delivered: false`.
//!   It is NEVER presented as delivered; the admin UI says "test
//!   mode — messages are logged, not delivered".
//!
//! `send_mail` never throws for a mail failure: it returns
//! `{ delivered, provider, error? }` and the caller decides. A mail
//! outage must never take down a webhook or a checkout.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use wordless_db::{exec_run, query_all, query_one, row::row_int, Db, Param, Row};
use wordless_kernel::{time::now_iso, WordlessError};

use crate::{
    config::get_integration_config,
    env::EnvSecrets,
    platform::{HttpClient, HttpError},
    util::{site_url, url_host},
};

/// Which provider handled a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MailProvider {
    Http,
    Mock,
}

/// Input to [`send_mail`].
#[derive(Debug, Clone)]
pub struct SendMailInput {
    pub to: String,
    pub subject: String,
    pub text: String,
    pub html: Option<String>,
}

/// Outcome of [`send_mail`]. `delivered == false` with provider
/// `mock` means "logged, not delivered" — never presented as sent.
#[derive(Debug, Clone, Serialize)]
pub struct SendMailResult {
    pub delivered: bool,
    pub provider: MailProvider,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One `wl_mail_log` row (wire shape: camelCase, v1 `MailLogEntry`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailLogEntry {
    pub id: i64,
    pub provider: String,
    pub to_email: String,
    pub subject: String,
    pub body: String,
    pub status: String,
    pub created_at: String,
}

/// One page of the mail log.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailLogPage {
    pub entries: Vec<MailLogEntry>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

async fn record_mail_async(
    db: &dyn Db,
    provider: &str,
    input: &SendMailInput,
    status: &str,
) -> Result<(), WordlessError> {
    let body: String = input.text.chars().take(10_000).collect();
    exec_run(
        db,
        "INSERT INTO wl_mail_log (provider, to_email, subject, body, status, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        &[
            Param::Text(provider.to_string()),
            Param::Text(input.to.clone()),
            Param::Text(input.subject.clone()),
            Param::Text(body),
            Param::Text(status.to_string()),
            Param::Text(now_iso()),
        ],
    )
    .await
    .map_err(WordlessError::from)?;
    Ok(())
}

/// The sender address: the configured `from`, else
/// `noreply@<site host>`, else `noreply@localhost` (v1 `getMailFrom`).
///
/// # Errors
/// Returns `Err` only on a database failure.
pub async fn get_mail_from(db: &dyn Db) -> Result<String, WordlessError> {
    let config = get_integration_config(db, "email").await?;
    if let Some(from) = config.get("from").and_then(|v| v.as_str()) {
        if !from.trim().is_empty() {
            return Ok(from.trim().to_string());
        }
    }
    let base = site_url(db).await?;
    let host = url_host(&base);
    if host.is_empty() {
        Ok("noreply@localhost".to_string())
    } else {
        Ok(format!("noreply@{host}"))
    }
}

/// Send one message via the configured provider, or log it locally
/// when none is configured. Never throws for a mail failure: a
/// provider error is returned as `delivered: false` with `error`
/// set, after recording `failed` in the mail log.
///
/// # Errors
/// Returns `Err` only on a database failure (the mail-log write).
pub async fn send_mail(
    db: &dyn Db,
    secrets: &EnvSecrets,
    http: &dyn HttpClient,
    input: &SendMailInput,
) -> Result<SendMailResult, WordlessError> {
    let config = get_integration_config(db, "email").await?;
    let api_url = config
        .get("apiUrl")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let api_key = secrets.email_api_key.clone().unwrap_or_default();
    if !api_url.is_empty() && !api_key.is_empty() {
        let from = get_mail_from(db).await?;
        let body = json!({
            "from": from,
            "to": input.to,
            "subject": input.subject,
            "text": input.text,
            "html": input.html,
        });
        match http.post_json(&api_url, &api_key, &body).await {
            Ok(res) if res.is_ok() => {
                record_mail_async(db, "http", input, "sent").await?;
                Ok(SendMailResult {
                    delivered: true,
                    provider: MailProvider::Http,
                    error: None,
                })
            }
            Ok(res) => {
                let detail = res.body.trim();
                let suffix = if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {}", detail.chars().take(200).collect::<String>())
                };
                record_mail_async(db, "http", input, "failed").await?;
                Ok(SendMailResult {
                    delivered: false,
                    provider: MailProvider::Http,
                    error: Some(format!(
                        "Email provider returned HTTP {}{}.",
                        res.status, suffix
                    )),
                })
            }
            Err(HttpError(msg)) => {
                record_mail_async(db, "http", input, "failed").await?;
                Ok(SendMailResult {
                    delivered: false,
                    provider: MailProvider::Http,
                    error: Some(msg),
                })
            }
        }
    } else {
        // No provider configured: log locally, never claim delivery.
        record_mail_async(db, "mock", input, "logged").await?;
        Ok(SendMailResult {
            delivered: false,
            provider: MailProvider::Mock,
            error: Some(
                "No email provider is configured — the message was logged, not delivered."
                    .to_string(),
            ),
        })
    }
}

/// List the mail log, newest first. `page` is clamped to ≥ 1,
/// `per_page` to 1..=100 (default 20), as in v1.
///
/// # Errors
/// Returns `Err` only on a database failure.
pub async fn list_mail_log(
    db: &dyn Db,
    page: i64,
    per_page: i64,
) -> Result<MailLogPage, WordlessError> {
    let page = page.max(1);
    let per_page = per_page.clamp(1, 100);
    let offset = (page - 1) * per_page;
    let total_row: Option<Row> = query_one(db, "SELECT COUNT(*) AS n FROM wl_mail_log", &[])
        .await
        .map_err(WordlessError::from)?;
    let total = total_row.as_ref().map(|r| row_int(r, "n")).unwrap_or(0);
    let rows: Vec<Row> = query_all(
        db,
        "SELECT * FROM wl_mail_log ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?",
        &[Param::Integer(per_page), Param::Integer(offset)],
    )
    .await
    .map_err(WordlessError::from)?;
    let entries = rows
        .iter()
        .map(|r| MailLogEntry {
            id: row_int(r, "id"),
            provider: r
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            to_email: r
                .get("to_email")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            subject: r
                .get("subject")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            body: r
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            status: r
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            created_at: r
                .get("created_at")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        })
        .collect();
    Ok(MailLogPage {
        entries,
        total,
        page,
        per_page,
        total_pages: ((total + per_page - 1) / per_page).max(1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{block_on, FakeHttpClient, SqliteDb};
    use wordless_db::options::set_option;

    fn input() -> SendMailInput {
        SendMailInput {
            to: "user@example.com".to_string(),
            subject: "Hello".to_string(),
            text: "Hi there".to_string(),
            html: None,
        }
    }

    #[test]
    fn logs_instead_of_delivering_when_no_provider_is_configured() {
        let db = SqliteDb::new();
        let fake = FakeHttpClient::ok();
        let result = block_on(send_mail(&db, &EnvSecrets::default(), &fake, &input())).unwrap();
        assert!(!result.delivered);
        assert_eq!(result.provider, MailProvider::Mock);
        assert!(result.error.unwrap().contains("logged, not delivered"));
        assert_eq!(fake.calls(), 0, "mock must never call HTTP");
        let page = block_on(list_mail_log(&db, 1, 20)).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.entries[0].status, "logged");
        assert_eq!(page.entries[0].provider, "mock");
        assert_eq!(page.entries[0].to_email, "user@example.com");
    }

    #[test]
    fn http_provider_delivers_and_logs_sent() {
        let db = SqliteDb::new();
        block_on(crate::config::set_integration_config(
            &db,
            "email",
            &json!({ "apiUrl": "https://api.example.com/send" })
                .as_object()
                .unwrap()
                .clone(),
        ))
        .unwrap();
        let secrets = EnvSecrets {
            email_api_key: Some("key_123".to_string()),
            ..Default::default()
        };
        let fake = FakeHttpClient::ok();
        let result = block_on(send_mail(&db, &secrets, &fake, &input())).unwrap();
        assert!(result.delivered);
        assert_eq!(result.provider, MailProvider::Http);
        assert_eq!(fake.calls(), 1);
        let last = fake.last_request().unwrap();
        assert_eq!(last.url, "https://api.example.com/send");
        assert_eq!(last.bearer, "key_123");
        assert_eq!(last.body["to"], json!("user@example.com"));
        let page = block_on(list_mail_log(&db, 1, 20)).unwrap();
        assert_eq!(page.entries[0].status, "sent");
    }

    #[test]
    fn http_provider_failure_is_logged_not_thrown() {
        let db = SqliteDb::new();
        block_on(crate::config::set_integration_config(
            &db,
            "email",
            &json!({ "apiUrl": "https://api.example.com/send" })
                .as_object()
                .unwrap()
                .clone(),
        ))
        .unwrap();
        let secrets = EnvSecrets {
            email_api_key: Some("key_123".to_string()),
            ..Default::default()
        };
        let fake = FakeHttpClient::failing(500, "provider exploded");
        let result = block_on(send_mail(&db, &secrets, &fake, &input())).unwrap();
        assert!(!result.delivered);
        assert!(result.error.unwrap().contains("HTTP 500"));
        let page = block_on(list_mail_log(&db, 1, 20)).unwrap();
        assert_eq!(page.entries[0].status, "failed");
    }

    #[test]
    fn derives_from_address_from_site_host() {
        let db = SqliteDb::new();
        block_on(set_option(
            &db,
            "siteurl",
            &"https://mysite.example.com".to_string(),
        ))
        .unwrap();
        assert_eq!(
            block_on(get_mail_from(&db)).unwrap(),
            "noreply@mysite.example.com"
        );
        block_on(crate::config::set_integration_config(
            &db,
            "email",
            &json!({ "from": "hello@custom.example" })
                .as_object()
                .unwrap()
                .clone(),
        ))
        .unwrap();
        assert_eq!(
            block_on(get_mail_from(&db)).unwrap(),
            "hello@custom.example"
        );
    }
}
