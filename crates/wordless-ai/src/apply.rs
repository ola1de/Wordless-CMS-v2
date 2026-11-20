//! Proposal applier — port of v1 `src/modules/ai/apply.ts`
//! (Milestone 6).
//!
//! Applies a proposed action through the EXISTING services — never
//! raw SQL writes to content tables. This is the single choke point
//! where an approved AI action touches the site, so the allow-lists
//! here are the safety boundary:
//! - `update_option` only touches a small allow-list of safe settings.
//! - `create_post` always creates a DRAFT (publishing is a separate,
//!   high-risk `publish_post` action that needs its own approval).
//! - No delete actions exist at all: the `ProposalActionType` enum
//!   has no delete variant, so a delete can never be proposed,
//!   approved, or applied.

use serde_json::{json, Value};
use wordless_db::Db;
use wordless_kernel::error::{bad_request, WordlessError};

use crate::types::{ProposalActionType, ProposedAction};
use crate::util::{payload_optional_string, payload_post_id, payload_string};

/// Option keys an approved AI action may change, with the value kind
/// each expects. Nothing else. (v1 `OPTION_ALLOWLIST`.)
fn option_kind(key: &str) -> Option<&'static str> {
    match key {
        "blogname" | "blogdescription" => Some("string"),
        "posts_per_page" => Some("int"),
        "default_comment_status" => Some("comment-status"),
        _ => None,
    }
}

/// Apply one proposed action as `user_id`. Returns a small result
/// object (ids of what was created/updated) for the task result and
/// audit trail.
pub async fn apply_proposal(
    db: &dyn Db,
    site_id: i64,
    action: &ProposedAction,
    user_id: i64,
) -> Result<Value, WordlessError> {
    let payload = &action.payload;
    match action.action_type {
        ProposalActionType::CreatePost => {
            let title = payload_string(payload, "title", 1000)?;
            let content =
                payload_optional_string(payload, "content", 1_000_000)?.unwrap_or_default();
            let excerpt = payload_optional_string(payload, "excerpt", 10000)?;
            // ALWAYS a draft, whatever the payload says: publishing is
            // the separate high-risk `publish_post` action.
            let post = wordless_cms::posts::create_post(
                db,
                site_id,
                wordless_cms::posts::CreatePostInput {
                    title,
                    content: Some(content),
                    excerpt,
                    slug: None,
                    status: Some("draft".to_string()),
                    post_type: Some("post".to_string()),
                    author_id: user_id,
                    parent_id: None,
                    mime_type: None,
                    featured_image_id: None,
                },
            )
            .await?;
            Ok(json!({"postId": post.id, "slug": post.slug}))
        }
        ProposalActionType::UpdatePost => {
            let post_id = payload_post_id(payload)?;
            let post = wordless_cms::posts::update_post(
                db,
                site_id,
                post_id,
                wordless_cms::posts::UpdatePostInput {
                    title: payload_optional_string(payload, "title", 1000)?,
                    content: payload_optional_string(payload, "content", 1_000_000)?,
                    excerpt: payload_optional_string(payload, "excerpt", 10000)?,
                    ..Default::default()
                },
            )
            .await?;
            Ok(json!({"postId": post.id}))
        }
        ProposalActionType::PublishPost => {
            let post_id = payload_post_id(payload)?;
            let post = wordless_cms::posts::update_post(
                db,
                site_id,
                post_id,
                wordless_cms::posts::UpdatePostInput {
                    status: Some("publish".to_string()),
                    ..Default::default()
                },
            )
            .await?;
            Ok(json!({"postId": post.id, "status": post.status.as_str()}))
        }
        ProposalActionType::UpdateOption => {
            let key = payload_string(payload, "key", 191)?;
            let kind = option_kind(&key).ok_or_else(|| {
                bad_request(&format!(
                    "Option \"{key}\" is not on the AI allow-list and cannot be changed by an agent."
                ))
            })?;
            match kind {
                "string" => {
                    let value = payload_string(payload, "value", 10000)?;
                    wordless_multisite::set_site_option(db, site_id, &key, &value).await?;
                }
                "int" => {
                    let value = payload
                        .get("value")
                        .and_then(|v| v.as_i64())
                        .filter(|v| (1..=100).contains(v))
                        .ok_or_else(|| {
                            bad_request(&format!(
                                "Proposal payload for option \"{key}\" must be an integer between 1 and 100."
                            ))
                        })?;
                    wordless_multisite::set_site_option(db, site_id, &key, &value).await?;
                }
                _ => {
                    let value = payload_string(payload, "value", 32)?;
                    if value != "open" && value != "closed" {
                        return Err(bad_request(&format!(
                            "Proposal payload for option \"{key}\" must be \"open\" or \"closed\"."
                        )));
                    }
                    wordless_multisite::set_site_option(db, site_id, &key, &value).await?;
                }
            }
            Ok(json!({"key": key}))
        }
        ProposalActionType::CreateTerm => {
            let term = wordless_cms::terms::create_term(
                db,
                site_id,
                wordless_cms::terms::CreateTermInput {
                    name: payload_string(payload, "name", 200)?,
                    slug: payload_optional_string(payload, "slug", 200)?,
                    taxonomy: payload_string(payload, "taxonomy", 32)?,
                    description: None,
                    parent_id: None,
                },
            )
            .await?;
            Ok(json!({"termTaxonomyId": term.term_taxonomy_id}))
        }
        ProposalActionType::UpdateThemeVariation => {
            let slug = payload_string(payload, "slug", 100)?;
            let settings = payload.get("settings").cloned().ok_or_else(|| {
                bad_request("Proposal payload is missing a valid \"settings\" object.")
            })?;
            if !settings.is_object() {
                return Err(bad_request(
                    "Proposal payload is missing a valid \"settings\" object.",
                ));
            }
            let variation =
                wordless_themes::save_variation(db, site_id, &slug, Some("AI suggestion"), &settings)
                    .await?;
            Ok(json!({"slug": slug, "updatedAt": variation.updated_at}))
        }
    }
}
