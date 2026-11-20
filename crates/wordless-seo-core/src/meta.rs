//! Per-post SEO fields (port of v1 `src/modules/seo/meta.ts` +
//! `types.ts`).
//!
//! Stored as `wp_postmeta` so they travel with the post. Keys use the
//! `_wl_seo_` prefix; Yoast (`_yoast_wpseo_*`) and Rank Math
//! (`rank_math_*`) keys are only understood by the importer
//! ([`crate::import`]), never read as live fields.
//!
//! Validation limits (enforced on write, never silently truncated):
//! SEO title ≤ 70 chars, meta description ≤ 160, focus keyword ≤ 100,
//! OG title ≤ 70, OG description ≤ 160. Canonical must be an absolute
//! http(s) URL or empty. OG image must be an existing attachment post
//! ID (0 = none). Twitter card is `summary` or `summary_large_image`.
//!
//! Queries D1 directly (no `wordless-cms` dependency for this module,
//! per the P2 task); the attachment existence check is a direct
//! `wp_posts` query too.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wordless_db::row::row_text;
use wordless_db::{Db, Param};
use wordless_kernel::error::{bad_request, not_found, WordlessError};

/// Per-post SEO fields (v1 `PostSeo`). The SEO title is the page title
/// WITHOUT the site name — the renderer appends it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostSeo {
    pub title: String,
    pub description: String,
    pub focus_keyword: String,
    pub canonical: String,
    pub noindex: bool,
    pub og_title: String,
    pub og_description: String,
    /// Attachment post ID for the Open Graph image (0 = none).
    pub og_image_id: i64,
    pub twitter_card: String,
}

/// One FAQ pair (P10). Stored as JSON in `_wl_seo_faq` postmeta;
/// rendered as `FAQPage` JSON-LD by `head.rs`. Validation limits:
/// at most 20 pairs, question ≤ 200 chars, answer ≤ 2000 chars.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaqItem {
    pub question: String,
    pub answer: String,
}

pub const MAX_FAQ_ITEMS: usize = 20;

/// Partial input accepted by [`set_post_seo`]. Only the keys present
/// are changed; an empty string clears a text field.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PostSeoInput {
    pub title: Option<String>,
    pub description: Option<String>,
    pub focus_keyword: Option<String>,
    pub canonical: Option<String>,
    pub noindex: Option<bool>,
    pub og_title: Option<String>,
    pub og_description: Option<String>,
    pub og_image_id: Option<i64>,
    pub twitter_card: Option<String>,
    /// Replaces the whole FAQ list when present (empty = clear).
    pub faq: Option<Vec<FaqItem>>,
}

/// The empty field set, for callers that need a default without a DB read.
pub fn empty_seo() -> PostSeo {
    PostSeo {
        title: String::new(),
        description: String::new(),
        focus_keyword: String::new(),
        canonical: String::new(),
        noindex: false,
        og_title: String::new(),
        og_description: String::new(),
        og_image_id: 0,
        twitter_card: "summary".to_string(),
    }
}

fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

async fn get_meta(db: &dyn Db, site_id: i64, post_id: i64, key: &str) -> Result<String, WordlessError> {
    let row = db
        .query_one(
            "SELECT meta_value FROM wp_postmeta WHERE post_id = ? AND meta_key = ? AND site_id = ? LIMIT 1",
            &[Param::Integer(post_id), Param::Text(key.to_string()), Param::Integer(site_id)],
        )
        .await
        .map_err(WordlessError::from)?;
    Ok(row.map(|r| as_text(&r["meta_value"])).unwrap_or_default())
}

async fn set_meta(db: &dyn Db, site_id: i64, post_id: i64, key: &str, value: &str) -> Result<(), WordlessError> {
    let existing = db
        .query_one(
            "SELECT meta_id FROM wp_postmeta WHERE post_id = ? AND meta_key = ? AND site_id = ? LIMIT 1",
            &[Param::Integer(post_id), Param::Text(key.to_string()), Param::Integer(site_id)],
        )
        .await
        .map_err(WordlessError::from)?;
    if existing.is_some() {
        db.exec(
            "UPDATE wp_postmeta SET meta_value = ? WHERE post_id = ? AND meta_key = ? AND site_id = ?",
            &[
                Param::Text(value.to_string()),
                Param::Integer(post_id),
                Param::Text(key.to_string()),
                Param::Integer(site_id),
            ],
        )
        .await
        .map_err(WordlessError::from)?;
    } else {
        db.exec(
            "INSERT INTO wp_postmeta (site_id, post_id, meta_key, meta_value) VALUES (?, ?, ?, ?)",
            &[
                Param::Integer(site_id),
                Param::Integer(post_id),
                Param::Text(key.to_string()),
                Param::Text(value.to_string()),
            ],
        )
        .await
        .map_err(WordlessError::from)?;
    }
    Ok(())
}

async fn delete_meta(db: &dyn Db, site_id: i64, post_id: i64, key: &str) -> Result<(), WordlessError> {
    db.exec(
        "DELETE FROM wp_postmeta WHERE post_id = ? AND meta_key = ? AND site_id = ?",
        &[Param::Integer(post_id), Param::Text(key.to_string()), Param::Integer(site_id)],
    )
    .await
    .map_err(WordlessError::from)?;
    Ok(())
}

async fn post_exists(db: &dyn Db, site_id: i64, post_id: i64) -> Result<bool, WordlessError> {
    let row = db
        .query_one(
            "SELECT ID FROM wp_posts WHERE ID = ? AND site_id = ? LIMIT 1",
            &[Param::Integer(post_id), Param::Integer(site_id)],
        )
        .await
        .map_err(WordlessError::from)?;
    Ok(row.is_some())
}

/// JS `String.length` counts UTF-16 code units; count the same way so
/// the 70/160/100 limits behave identically for non-ASCII titles.
fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

fn require_seo_string(value: &Value, field: &str, max: usize) -> Result<String, WordlessError> {
    match value {
        Value::Null => Ok(String::new()),
        Value::String(s) => {
            if js_len(s) > max {
                return Err(bad_request(&format!(
                    "Field \"{field}\" is too long (max {max} characters)."
                )));
            }
            Ok(s.clone())
        }
        _ => Err(bad_request(&format!("Field \"{field}\" must be a string."))),
    }
}

fn require_canonical(value: &Value) -> Result<String, WordlessError> {
    match value {
        Value::Null => Ok(String::new()),
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Ok(String::new());
            }
            let lower = trimmed.to_ascii_lowercase();
            let rest = if let Some(r) = lower.strip_prefix("http://") {
                r
            } else if let Some(r) = lower.strip_prefix("https://") {
                r
            } else {
                return Err(bad_request(
                    "Field \"canonical\" must be a valid absolute http(s) URL or empty.",
                ));
            };
            // A host must follow the scheme (no spaces, not empty).
            let host = rest.split(['/', '?', '#']).next().unwrap_or("");
            if host.is_empty() || trimmed.contains(char::is_whitespace) {
                return Err(bad_request(
                    "Field \"canonical\" must be a valid absolute http(s) URL or empty.",
                ));
            }
            Ok(trimmed.to_string())
        }
        _ => Err(bad_request("Field \"canonical\" must be a string.")),
    }
}

async fn require_og_image_id(db: &dyn Db, site_id: i64, value: &Value) -> Result<i64, WordlessError> {
    let n: i64 = match value {
        Value::Null => return Ok(0),
        Value::Number(n) => {
            let Some(i) = n.as_i64() else {
                return Err(bad_request(
                    "Field \"ogImageId\" must be a media attachment ID or 0.",
                ));
            };
            i
        }
        Value::String(s) => {
            if s.is_empty() {
                return Ok(0);
            }
            // JS `Number("12")` / `Number("abc")` semantics: a numeric
            // string parses, anything else is NaN (not an integer).
            match s.trim().parse::<f64>() {
                Ok(f) if f.fract() == 0.0 && f >= 0.0 && f <= i64::MAX as f64 => f as i64,
                _ => {
                    return Err(bad_request(
                        "Field \"ogImageId\" must be a media attachment ID or 0.",
                    ));
                }
            }
        }
        _ => {
            return Err(bad_request(
                "Field \"ogImageId\" must be a media attachment ID or 0.",
            ));
        }
    };
    if n < 0 {
        return Err(bad_request(
            "Field \"ogImageId\" must be a media attachment ID or 0.",
        ));
    }
    if n == 0 {
        return Ok(0);
    }
    let row = db
        .query_one(
            "SELECT ID FROM wp_posts WHERE ID = ? AND post_type = 'attachment' AND site_id = ? LIMIT 1",
            &[Param::Integer(n), Param::Integer(site_id)],
        )
        .await
        .map_err(WordlessError::from)?;
    if row.is_none() {
        return Err(bad_request(&format!(
            "Field \"ogImageId\" does not match an existing media item ({n})."
        )));
    }
    Ok(n)
}

fn require_twitter_card(value: &Value) -> Result<String, WordlessError> {
    match value {
        Value::Null => Ok("summary".to_string()),
        Value::String(s) => {
            if s.is_empty() || s == "summary" || s == "summary_large_image" {
                Ok(if s.is_empty() {
                    "summary".to_string()
                } else {
                    s.clone()
                })
            } else {
                Err(bad_request(
                    "Field \"twitterCard\" must be \"summary\" or \"summary_large_image\".",
                ))
            }
        }
        _ => Err(bad_request(
            "Field \"twitterCard\" must be \"summary\" or \"summary_large_image\".",
        )),
    }
}

/// Validate FAQ pairs. At most 20, each with a non-empty question
/// (≤ 200 chars) and answer (≤ 2000 chars).
pub fn validate_faq(faqs: &[FaqItem]) -> Result<(), WordlessError> {
    if faqs.len() > MAX_FAQ_ITEMS {
        return Err(bad_request(&format!(
            "Field \"faq\" must have at most {MAX_FAQ_ITEMS} items."
        )));
    }
    for (i, item) in faqs.iter().enumerate() {
        if item.question.trim().is_empty() || item.question.chars().count() > 200 {
            return Err(bad_request(&format!(
                "FAQ item {} must have a question of 1-200 characters.",
                i + 1
            )));
        }
        if item.answer.trim().is_empty() || item.answer.chars().count() > 2000 {
            return Err(bad_request(&format!(
                "FAQ item {} must have an answer of 1-2000 characters.",
                i + 1
            )));
        }
    }
    Ok(())
}

/// Read a post's FAQ pairs (empty when none were set, or when the
/// stored JSON is corrupt — a bad meta value never 500s the page).
/// 404 when the post does not exist.
pub async fn get_post_faq(db: &dyn Db, site_id: i64, post_id: i64) -> Result<Vec<FaqItem>, WordlessError> {
    if !post_exists(db, site_id, post_id).await? {
        return Err(not_found(&format!("Post {post_id} not found.")));
    }
    let raw = get_meta(db, site_id, post_id, "_wl_seo_faq").await?;
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    Ok(serde_json::from_str::<Vec<FaqItem>>(&raw).unwrap_or_default())
}

/// Validate + save a post's FAQ pairs (empty list clears them).
/// 404 when the post does not exist.
pub async fn set_post_faq(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    faqs: &[FaqItem],
) -> Result<Vec<FaqItem>, WordlessError> {
    if !post_exists(db, site_id, post_id).await? {
        return Err(not_found(&format!("Post {post_id} not found.")));
    }
    validate_faq(faqs)?;
    if faqs.is_empty() {
        delete_meta(db, site_id, post_id, "_wl_seo_faq").await?;
    } else {
        set_meta(
            db,
            site_id,
            post_id,
            "_wl_seo_faq",
            &serde_json::to_string(faqs).unwrap_or_default(),
        )
        .await?;
    }
    Ok(faqs.to_vec())
}

/// Read a post's SEO fields (all empty/false when nothing was set).
/// 404 when the post does not exist.
pub async fn get_post_seo(db: &dyn Db, site_id: i64, post_id: i64) -> Result<PostSeo, WordlessError> {
    if !post_exists(db, site_id, post_id).await? {
        return Err(not_found(&format!("Post {post_id} not found.")));
    }
    // Same SQL as v1 (LIKE with the literal `_wl_seo_%` pattern).
    let rows = db
        .query_all(
            "SELECT meta_key, meta_value FROM wp_postmeta WHERE post_id = ? AND meta_key LIKE '_wl_seo_%' AND site_id = ?",
            &[Param::Integer(post_id), Param::Integer(site_id)],
        )
        .await
        .map_err(WordlessError::from)?;
    let mut by_key: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for row in &rows {
        by_key.insert(row_text(row, "meta_key"), as_text(&row["meta_value"]));
    }
    let og_raw = by_key.get("_wl_seo_og_image").cloned().unwrap_or_default();
    let og_parsed: i64 = og_raw.trim().parse().unwrap_or(0);
    Ok(PostSeo {
        title: by_key.get("_wl_seo_title").cloned().unwrap_or_default(),
        description: by_key
            .get("_wl_seo_description")
            .cloned()
            .unwrap_or_default(),
        focus_keyword: by_key
            .get("_wl_seo_focus_keyword")
            .cloned()
            .unwrap_or_default(),
        canonical: by_key.get("_wl_seo_canonical").cloned().unwrap_or_default(),
        noindex: by_key.get("_wl_seo_noindex").map(|v| v.as_str()) == Some("1"),
        og_title: by_key.get("_wl_seo_og_title").cloned().unwrap_or_default(),
        og_description: by_key
            .get("_wl_seo_og_description")
            .cloned()
            .unwrap_or_default(),
        og_image_id: if og_parsed > 0 { og_parsed } else { 0 },
        twitter_card: if by_key.get("_wl_seo_twitter_card").map(|v| v.as_str())
            == Some("summary_large_image")
        {
            "summary_large_image".to_string()
        } else {
            "summary".to_string()
        },
    })
}

/// Validate + save a post's SEO fields. Only the keys present in
/// `input` are changed; an empty string clears a text field. Returns
/// the full field set after saving. Everything is validated before
/// anything is written.
pub async fn set_post_seo(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    input: PostSeoInput,
) -> Result<PostSeo, WordlessError> {
    if !post_exists(db, site_id, post_id).await? {
        return Err(not_found(&format!("Post {post_id} not found.")));
    }
    // Validate everything before writing anything.
    let title = match &input.title {
        Some(v) => Some(require_seo_string(&Value::String(v.clone()), "title", 70)?),
        None => None,
    };
    let description = match &input.description {
        Some(v) => Some(require_seo_string(
            &Value::String(v.clone()),
            "description",
            160,
        )?),
        None => None,
    };
    let focus_keyword = match &input.focus_keyword {
        Some(v) => Some(require_seo_string(
            &Value::String(v.clone()),
            "focusKeyword",
            100,
        )?),
        None => None,
    };
    let canonical = match &input.canonical {
        Some(v) => Some(require_canonical(&Value::String(v.clone()))?),
        None => None,
    };
    let og_title = match &input.og_title {
        Some(v) => Some(require_seo_string(
            &Value::String(v.clone()),
            "ogTitle",
            70,
        )?),
        None => None,
    };
    let og_description = match &input.og_description {
        Some(v) => Some(require_seo_string(
            &Value::String(v.clone()),
            "ogDescription",
            160,
        )?),
        None => None,
    };
    let og_image_id = match input.og_image_id {
        Some(n) => Some(require_og_image_id(db, site_id, &Value::from(n)).await?),
        None => None,
    };
    let twitter_card = match &input.twitter_card {
        Some(v) => Some(require_twitter_card(&Value::String(v.clone()))?),
        None => None,
    };
    if let Some(faqs) = &input.faq {
        validate_faq(faqs)?;
    }

    async fn write_text(
        db: &dyn Db,
        site_id: i64,
        post_id: i64,
        key: &str,
        value: &str,
    ) -> Result<(), WordlessError> {
        if value.is_empty() {
            delete_meta(db, site_id, post_id, key).await
        } else {
            set_meta(db, site_id, post_id, key, value).await
        }
    }
    if let Some(v) = &title {
        write_text(db, site_id, post_id, "_wl_seo_title", v).await?;
    }
    if let Some(v) = &description {
        write_text(db, site_id, post_id, "_wl_seo_description", v).await?;
    }
    if let Some(v) = &focus_keyword {
        write_text(db, site_id, post_id, "_wl_seo_focus_keyword", v).await?;
    }
    if let Some(v) = &canonical {
        write_text(db, site_id, post_id, "_wl_seo_canonical", v).await?;
    }
    if let Some(noindex) = input.noindex {
        if noindex {
            set_meta(db, site_id, post_id, "_wl_seo_noindex", "1").await?;
        } else {
            delete_meta(db, site_id, post_id, "_wl_seo_noindex").await?;
        }
    }
    if let Some(v) = &og_title {
        write_text(db, site_id, post_id, "_wl_seo_og_title", v).await?;
    }
    if let Some(v) = &og_description {
        write_text(db, site_id, post_id, "_wl_seo_og_description", v).await?;
    }
    if let Some(n) = og_image_id {
        if n == 0 {
            delete_meta(db, site_id, post_id, "_wl_seo_og_image").await?;
        } else {
            set_meta(db, site_id, post_id, "_wl_seo_og_image", &n.to_string()).await?;
        }
    }
    if let Some(v) = &twitter_card {
        write_text(db, site_id, post_id, "_wl_seo_twitter_card", v).await?;
    }
    if let Some(faqs) = &input.faq {
        if faqs.is_empty() {
            delete_meta(db, site_id, post_id, "_wl_seo_faq").await?;
        } else {
            set_meta(
                db,
                site_id,
                post_id,
                "_wl_seo_faq",
                &serde_json::to_string(faqs).unwrap_or_default(),
            )
            .await?;
        }
    }
    get_post_seo(db, site_id, post_id).await
}

/// Validate + save from a JSON request body (the coordinator's PUT
/// `/api/posts/:id/seo` handler). Only the known keys are picked up;
/// unknown keys are ignored, exactly as v1's PUT handler does. A
/// non-string value for a text field, a non-boolean `noindex`, or a
/// non-numeric `ogImageId` is a 400 with v1's message.
pub async fn set_post_seo_from_json(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    body: &Value,
) -> Result<PostSeo, WordlessError> {
    let obj = body.as_object().cloned().unwrap_or_default();
    let mut input = PostSeoInput::default();
    if let Some(v) = obj.get("title") {
        input.title = Some(require_seo_string(v, "title", 70)?);
    }
    if let Some(v) = obj.get("description") {
        input.description = Some(require_seo_string(v, "description", 160)?);
    }
    if let Some(v) = obj.get("focusKeyword") {
        input.focus_keyword = Some(require_seo_string(v, "focusKeyword", 100)?);
    }
    if let Some(v) = obj.get("canonical") {
        input.canonical = Some(require_canonical(v)?);
    }
    if let Some(v) = obj.get("noindex") {
        match v {
            Value::Bool(b) => input.noindex = Some(*b),
            Value::Null => {}
            _ => {
                return Err(bad_request("Field \"noindex\" must be true or false."));
            }
        }
    }
    if let Some(v) = obj.get("ogTitle") {
        input.og_title = Some(require_seo_string(v, "ogTitle", 70)?);
    }
    if let Some(v) = obj.get("ogDescription") {
        input.og_description = Some(require_seo_string(v, "ogDescription", 160)?);
    }
    if let Some(v) = obj.get("ogImageId") {
        input.og_image_id = Some(require_og_image_id(db, site_id, v).await?);
    }
    if let Some(v) = obj.get("twitterCard") {
        input.twitter_card = Some(require_twitter_card(v)?);
    }
    if let Some(v) = obj.get("faq") {
        let arr = v.as_array().cloned().unwrap_or_default();
        let mut faqs: Vec<FaqItem> = Vec::new();
        for item in &arr {
            let question = item
                .get("question")
                .and_then(|q| q.as_str())
                .unwrap_or("")
                .to_string();
            let answer = item
                .get("answer")
                .and_then(|a| a.as_str())
                .unwrap_or("")
                .to_string();
            faqs.push(FaqItem { question, answer });
        }
        input.faq = Some(faqs);
    }
    set_post_seo(db, site_id, post_id, input).await
}

/// Read one raw meta value (used by the import path's diagnostics).
pub async fn get_raw_meta(db: &dyn Db, site_id: i64, post_id: i64, key: &str) -> Result<String, WordlessError> {
    get_meta(db, site_id, post_id, key).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SqliteDb;
    use futures_executor::block_on;
    use wordless_cms::posts::{create_post, CreatePostInput};

    fn post(db: &SqliteDb, title: &str) -> i64 {
        block_on(async {
            create_post(
                db,
                1,
                CreatePostInput {
                    title: title.to_string(),
                    author_id: 1,
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .id
        })
    }

    #[test]
    fn round_trips_fields_and_clears_on_empty_string() {
        let db = SqliteDb::new();
        let id = post(&db, "Hello");
        let saved = block_on(set_post_seo(
            &db, 1,
            id,
            PostSeoInput {
                title: Some("Hello — the pack".to_string()),
                description: Some("A short description of the post.".to_string()),
                focus_keyword: Some("ridge trail".to_string()),
                canonical: Some("https://example.test/hello".to_string()),
                noindex: Some(true),
                og_title: Some("Hello (social)".to_string()),
                og_description: Some("Social description.".to_string()),
                twitter_card: Some("summary_large_image".to_string()),
                ..Default::default()
            },
        ))
        .unwrap();
        assert_eq!(saved.title, "Hello — the pack");
        assert!(saved.noindex);
        assert_eq!(saved.twitter_card, "summary_large_image");
        let read_back = block_on(get_post_seo(&db, 1, id)).unwrap();
        assert_eq!(read_back.canonical, "https://example.test/hello");
        let cleared = block_on(set_post_seo(
            &db, 1,
            id,
            PostSeoInput {
                title: Some(String::new()),
                noindex: Some(false),
                ..Default::default()
            },
        ))
        .unwrap();
        assert_eq!(cleared.title, "");
        assert!(!cleared.noindex);
        assert_eq!(cleared.description, "A short description of the post.");
    }

    #[test]
    fn validates_limits_and_values() {
        let db = SqliteDb::new();
        let id = post(&db, "Hello");
        let err = |r: Result<PostSeo, WordlessError>| r.unwrap_err().message;
        assert!(err(block_on(set_post_seo(
            &db, 1,
            id,
            PostSeoInput {
                title: Some("x".repeat(71)),
                ..Default::default()
            }
        )))
        .contains("too long"));
        assert!(err(block_on(set_post_seo(
            &db, 1,
            id,
            PostSeoInput {
                description: Some("x".repeat(161)),
                ..Default::default()
            }
        )))
        .contains("too long"));
        assert!(err(block_on(set_post_seo(
            &db, 1,
            id,
            PostSeoInput {
                focus_keyword: Some("x".repeat(101)),
                ..Default::default()
            }
        )))
        .contains("too long"));
        assert!(err(block_on(set_post_seo(
            &db, 1,
            id,
            PostSeoInput {
                canonical: Some("not a url".to_string()),
                ..Default::default()
            }
        )))
        .contains("absolute http"));
        assert!(err(block_on(set_post_seo(
            &db, 1,
            id,
            PostSeoInput {
                canonical: Some("ftp://example.test/x".to_string()),
                ..Default::default()
            }
        )))
        .contains("absolute http"));
        assert!(err(block_on(set_post_seo(
            &db, 1,
            id,
            PostSeoInput {
                twitter_card: Some("player".to_string()),
                ..Default::default()
            }
        )))
        .contains("summary"));
        assert!(err(block_on(set_post_seo(
            &db, 1,
            id,
            PostSeoInput {
                og_image_id: Some(9999),
                ..Default::default()
            }
        )))
        .contains("existing media item"));
        assert!(block_on(get_post_seo(&db, 1, 9999)).is_err());
        // JSON path: non-boolean noindex is a 400.
        let body = serde_json::json!({ "noindex": "yes" });
        assert!(block_on(set_post_seo_from_json(&db, 1, id, &body))
            .unwrap_err()
            .message
            .contains("true or false"));
    }

    #[test]
    fn accepts_an_existing_attachment_as_the_og_image() {
        let db = SqliteDb::new();
        let id = post(&db, "Hello");
        let attachment = block_on(db.insert_post(
            "wolf.png",
            "",
            "",
            "inherit",
            "wolf-png",
            "attachment",
            "2026-11-19T00:00:00.000Z",
            1,
        ));
        let saved = block_on(set_post_seo(
            &db, 1,
            id,
            PostSeoInput {
                og_image_id: Some(attachment),
                ..Default::default()
            },
        ))
        .unwrap();
        assert_eq!(saved.og_image_id, attachment);
    }
}
