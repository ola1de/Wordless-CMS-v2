//! SEO metadata import — Yoast SEO + Rank Math (port of v1
//! `src/modules/seo/import.ts`).
//!
//! Accepts a list of rows, each with either a post ID or a slug plus
//! a `meta` object whose keys may be:
//! - Wordless: `_wl_seo_title`, `_wl_seo_description`,
//!   `_wl_seo_focus_keyword`, `_wl_seo_canonical`, `_wl_seo_noindex`,
//!   `_wl_seo_og_title`, `_wl_seo_og_description`, `_wl_seo_og_image`,
//!   `_wl_seo_twitter_card`
//! - Yoast SEO: `_yoast_wpseo_title`, `_yoast_wpseo_metadesc`,
//!   `_yoast_wpseo_focuskw`, `_yoast_wpseo_canonical`,
//!   `_yoast_wpseo_meta-robots-noindex`, `_yoast_wpseo_opengraph-title`,
//!   `_yoast_wpseo_opengraph-description`,
//!   `_yoast_wpseo_opengraph-image-id`, `_yoast_wpseo_twitter-card`
//! - Rank Math: `rank_math_title`, `rank_math_description`,
//!   `rank_math_focus_keyword`, `rank_math_canonical_url`,
//!   `rank_math_robots` (array or string; "noindex" is honoured),
//!   `rank_math_facebook_title`, `rank_math_facebook_description`,
//!   `rank_math_facebook_image_id`, `rank_math_twitter_card_type`
//!
//! Keys that are not recognised are reported as skipped (never
//! silently dropped or mis-mapped). Every value goes through the same
//! validation as the SEO editor ([`wordless_seo_core::meta::set_post_seo`]), so an
//! invalid row is skipped with its reason instead of aborting the
//! whole import. At most 500 rows.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use wordless_cms::posts::{get_post_by_id, get_post_by_slug};
use wordless_db::Db;
use wordless_kernel::error::{bad_request, WordlessError};

use wordless_seo_core::meta::set_post_seo_from_json;

pub const MAX_IMPORT_ROWS: usize = 500;

/// One row of a Yoast / Rank Math / Wordless SEO metadata import
/// (v1 `SeoImportRow`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeoImportRow {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    /// Meta keys: Wordless `_wl_seo_*`, Yoast `_yoast_wpseo_*`, or
    /// Rank Math `rank_math_*`. `None` = the row has no meta object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

/// One skipped row (v1 `SeoImportResult.skipped` entry).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeoImportSkipped {
    pub post: String,
    pub reason: String,
}

/// Import result (v1 `SeoImportResult`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeoImportResult {
    pub applied: i64,
    pub skipped: Vec<SeoImportSkipped>,
}

const KNOWN_KEYS: [&str; 27] = [
    "_wl_seo_title",
    "_wl_seo_description",
    "_wl_seo_focus_keyword",
    "_wl_seo_canonical",
    "_wl_seo_noindex",
    "_wl_seo_og_title",
    "_wl_seo_og_description",
    "_wl_seo_og_image",
    "_wl_seo_twitter_card",
    "_yoast_wpseo_title",
    "_yoast_wpseo_metadesc",
    "_yoast_wpseo_focuskw",
    "_yoast_wpseo_canonical",
    "_yoast_wpseo_meta-robots-noindex",
    "_yoast_wpseo_opengraph-title",
    "_yoast_wpseo_opengraph-description",
    "_yoast_wpseo_opengraph-image-id",
    "_yoast_wpseo_twitter-card",
    "rank_math_title",
    "rank_math_description",
    "rank_math_focus_keyword",
    "rank_math_canonical_url",
    "rank_math_robots",
    "rank_math_facebook_title",
    "rank_math_facebook_description",
    "rank_math_facebook_image_id",
    "rank_math_twitter_card_type",
];

/// JS `String(value)`: strings as-is, numbers in JSON form, booleans
/// as `true`/`false`, null as `null`.
fn js_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        Value::Array(_) | Value::Object(_) => v.to_string(),
    }
}

fn take<'a>(meta: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| meta.get(*k))
}

/// Normalise one meta object to Wordless field names (camelCase JSON,
/// as accepted by [`set_post_seo_from_json`]). Unknown keys are
/// returned separately.
pub fn normalize_meta(meta: &Map<String, Value>) -> (Value, Vec<String>) {
    let mut input = Map::new();
    let mut put_str = |key: &str, keys: &[&str]| {
        if let Some(v) = take(meta, keys) {
            input.insert(key.to_string(), Value::String(js_string(v)));
        }
    };
    put_str(
        "title",
        &["_wl_seo_title", "_yoast_wpseo_title", "rank_math_title"],
    );
    put_str(
        "description",
        &[
            "_wl_seo_description",
            "_yoast_wpseo_metadesc",
            "rank_math_description",
        ],
    );
    put_str(
        "focusKeyword",
        &[
            "_wl_seo_focus_keyword",
            "_yoast_wpseo_focuskw",
            "rank_math_focus_keyword",
        ],
    );
    put_str(
        "canonical",
        &[
            "_wl_seo_canonical",
            "_yoast_wpseo_canonical",
            "rank_math_canonical_url",
        ],
    );
    put_str(
        "ogTitle",
        &[
            "_wl_seo_og_title",
            "_yoast_wpseo_opengraph-title",
            "rank_math_facebook_title",
        ],
    );
    put_str(
        "ogDescription",
        &[
            "_wl_seo_og_description",
            "_yoast_wpseo_opengraph-description",
            "rank_math_facebook_description",
        ],
    );
    if let Some(v) = take(
        meta,
        &[
            "_wl_seo_og_image",
            "_yoast_wpseo_opengraph-image-id",
            "rank_math_facebook_image_id",
        ],
    ) {
        // v1: number as-is, otherwise `Number(value)` (a numeric
        // string parses; anything else becomes NaN and is rejected
        // by the editor validation with its usual message).
        let num = match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.trim().parse::<f64>().ok(),
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        };
        match num {
            Some(f) => {
                input.insert(
                    "ogImageId".to_string(),
                    serde_json::Number::from_f64(f)
                        .map(Value::Number)
                        .unwrap_or(Value::Null),
                );
            }
            None => {
                input.insert("ogImageId".to_string(), Value::String(js_string(v)));
            }
        }
    }
    if let Some(v) = take(
        meta,
        &[
            "_wl_seo_twitter_card",
            "_yoast_wpseo_twitter-card",
            "rank_math_twitter_card_type",
        ],
    ) {
        let t = js_string(v);
        input.insert(
            "twitterCard".to_string(),
            Value::String(if t == "summary_large_image" {
                "summary_large_image".to_string()
            } else {
                "summary".to_string()
            }),
        );
    }
    // noindex: Yoast "1"/"0" or true/false; Rank Math robots
    // array/string containing "noindex". Precedence: Wordless >
    // Yoast > Rank Math.
    let wl_noindex = meta.get("_wl_seo_noindex");
    let yoast_noindex = meta.get("_yoast_wpseo_meta-robots-noindex");
    let rank_robots = meta.get("rank_math_robots");
    let noindex = if let Some(v) = wl_noindex {
        Some(
            v == &Value::Bool(true) || v == &Value::String("1".to_string()) || v == &Value::from(1),
        )
    } else if let Some(v) = yoast_noindex {
        Some(
            v == &Value::Bool(true) || v == &Value::String("1".to_string()) || v == &Value::from(1),
        )
    } else if let Some(v) = rank_robots {
        let list: Vec<String> = match v {
            Value::Array(items) => items.iter().map(js_string).collect(),
            other => vec![js_string(other)],
        };
        Some(list.iter().any(|s| s.to_lowercase().contains("noindex")))
    } else {
        None
    };
    if let Some(b) = noindex {
        input.insert("noindex".to_string(), Value::Bool(b));
    }
    let unknown_keys: Vec<String> = meta
        .keys()
        .filter(|k| !KNOWN_KEYS.contains(&k.as_str()))
        .cloned()
        .collect();
    (Value::Object(input), unknown_keys)
}

pub async fn import_seo_metadata(
    db: &dyn Db,
    site_id: i64,
    rows: &[SeoImportRow],
) -> Result<SeoImportResult, WordlessError> {
    if rows.len() > MAX_IMPORT_ROWS {
        return Err(bad_request(&format!(
            "Import is limited to {MAX_IMPORT_ROWS} posts per request."
        )));
    }
    let mut applied: i64 = 0;
    let mut skipped: Vec<SeoImportSkipped> = Vec::new();
    for row in rows {
        let label = if let Some(id) = row.post_id {
            format!("post {id}")
        } else if let Some(slug) = &row.slug {
            format!("slug \"{slug}\"")
        } else {
            "row".to_string()
        };
        // The row must carry a meta object.
        let meta_obj = match &row.meta {
            Some(Value::Object(map)) => Some(map.clone()),
            _ => {
                skipped.push(SeoImportSkipped {
                    post: label,
                    reason: "Row has no \"meta\" object.".to_string(),
                });
                continue;
            }
        };
        let meta_obj = meta_obj.unwrap();
        // Resolve the post.
        let post_id: i64 = if let Some(id) = row.post_id {
            match get_post_by_id(db, site_id, id).await {
                Ok(Some(post)) => post.id,
                Ok(None) => {
                    skipped.push(SeoImportSkipped {
                        post: label,
                        reason: format!("Post {id} does not exist."),
                    });
                    continue;
                }
                Err(e) => {
                    skipped.push(SeoImportSkipped {
                        post: label,
                        reason: e.message,
                    });
                    continue;
                }
            }
        } else if let Some(slug) = &row.slug {
            match get_post_by_slug(db, site_id, slug).await {
                Ok(Some(post)) => post.id,
                Ok(None) => {
                    skipped.push(SeoImportSkipped {
                        post: label,
                        reason: format!("No post with slug \"{slug}\"."),
                    });
                    continue;
                }
                Err(e) => {
                    skipped.push(SeoImportSkipped {
                        post: label,
                        reason: e.message,
                    });
                    continue;
                }
            }
        } else {
            skipped.push(SeoImportSkipped {
                post: label,
                reason: "Row has neither postId nor slug.".to_string(),
            });
            continue;
        };
        let (input, unknown_keys) = normalize_meta(&meta_obj);
        if input.as_object().map(|o| o.is_empty()).unwrap_or(true) {
            skipped.push(SeoImportSkipped {
                post: label,
                reason: "No recognised SEO fields in meta.".to_string(),
            });
            continue;
        }
        match set_post_seo_from_json(db, site_id, post_id, &input).await {
            Ok(_) => {
                applied += 1;
                if !unknown_keys.is_empty() {
                    skipped.push(SeoImportSkipped {
                        post: label,
                        reason: format!(
                            "Applied, but ignored unknown keys: {}.",
                            unknown_keys.join(", ")
                        ),
                    });
                }
            }
            Err(e) => {
                skipped.push(SeoImportSkipped {
                    post: label,
                    reason: e.message,
                });
            }
        }
    }
    Ok(SeoImportResult { applied, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wordless_seo_core::meta::get_post_seo;
    use crate::test_support::SqliteDb;
    use futures_executor::block_on;
    use serde_json::json;
    use wordless_cms::posts::{create_post, CreatePostInput};

    fn make_post(db: &SqliteDb, title: &str) -> (i64, String) {
        block_on(async {
            let post = create_post(
                db,
                1,
                CreatePostInput {
                    title: title.to_string(),
                    content: Some("Body.".to_string()),
                    author_id: 1,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            (post.id, post.slug)
        })
    }

    #[test]
    fn applies_yoast_keys_by_slug_and_rank_math_keys_by_id() {
        let db = SqliteDb::new();
        let (a_id, a_slug) = make_post(&db, "Yoast Post");
        let (b_id, _) = make_post(&db, "Rank Post");
        let result = block_on(import_seo_metadata(
            &db, 1,
            &[
                SeoImportRow {
                    post_id: None,
                    slug: Some(a_slug),
                    meta: Some(json!({
                        "_yoast_wpseo_title": "Yoast Title",
                        "_yoast_wpseo_metadesc": "Yoast description here.",
                        "_yoast_wpseo_focuskw": "yoast",
                        "_yoast_wpseo_canonical": "https://example.test/yoast",
                        "_yoast_wpseo_meta-robots-noindex": "1",
                    })),
                },
                SeoImportRow {
                    post_id: Some(b_id),
                    slug: None,
                    meta: Some(json!({
                        "rank_math_title": "Rank Title",
                        "rank_math_description": "Rank description here.",
                        "rank_math_focus_keyword": "rank",
                        "rank_math_robots": ["index", "noindex"],
                    })),
                },
            ],
        ))
        .unwrap();
        assert_eq!(result.applied, 2);
        assert!(result.skipped.is_empty());
        let seo_a = block_on(get_post_seo(&db, 1, a_id)).unwrap();
        assert_eq!(seo_a.title, "Yoast Title");
        assert_eq!(seo_a.focus_keyword, "yoast");
        assert!(seo_a.noindex);
        let seo_b = block_on(get_post_seo(&db, 1, b_id)).unwrap();
        assert_eq!(seo_b.title, "Rank Title");
        assert!(seo_b.noindex);
    }

    #[test]
    fn skips_unknown_posts_invalid_values_and_unknown_keys_honestly() {
        let db = SqliteDb::new();
        let (post_id, _) = make_post(&db, "Kept");
        let result = block_on(import_seo_metadata(
            &db, 1,
            &[
                SeoImportRow {
                    post_id: None,
                    slug: Some("no-such-slug".to_string()),
                    meta: Some(json!({ "_yoast_wpseo_title": "X" })),
                },
                SeoImportRow {
                    post_id: Some(post_id),
                    slug: None,
                    meta: Some(json!({ "_yoast_wpseo_canonical": "not a url" })),
                },
                SeoImportRow {
                    post_id: Some(post_id),
                    slug: None,
                    meta: Some(
                        json!({ "_yoast_wpseo_title": "Fine", "some_plugin_key": "ignored" }),
                    ),
                },
                SeoImportRow {
                    post_id: None,
                    slug: None,
                    meta: Some(json!({ "_yoast_wpseo_title": "No identity" })),
                },
            ],
        ))
        .unwrap();
        assert_eq!(result.applied, 1);
        // 4 skipped entries: unknown slug, invalid canonical, no
        // identity — plus the applied row's own note about its ignored
        // unknown key.
        assert_eq!(result.skipped.len(), 4);
        assert!(result
            .skipped
            .iter()
            .any(|s| s.reason.contains("unknown keys")));
        assert!(result
            .skipped
            .iter()
            .any(|s| s.reason.contains("No post with slug")));
        assert_eq!(block_on(get_post_seo(&db, 1, post_id)).unwrap().title, "Fine");
    }
}
