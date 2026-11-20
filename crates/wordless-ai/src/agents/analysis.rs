//! Deterministic analysis agents — port of v1
//! `src/ai/agents/analysis.ts` (Milestone 6, with Milestone 9
//! updates).
//!
//! These agents do NOT need a provider: they measure the site's real
//! data (posts, comments, users, media, themes, conversion jobs) and
//! report only what they measured. Nothing here is invented — a check
//! that finds nothing says so.
//!
//! The agents call the owning P3 crates' service functions
//! (`wordless-security`, `wordless-backup`, `wordless-conversion`,
//! `wordless-integrations`, `wordless-commerce`) wherever they exist,
//! exactly as v1 calls the corresponding TypeScript services. Two
//! honest limits remain, both documented where they occur:
//! - R2 object counts and image-derivative binding state come from
//!   the worker, not from inside this crate: the agent context
//!   carries an optional `object_count` (`None` = unmeasured, never
//!   invented), and derivatives are reported unbound because the
//!   Images binding is not declared in `wrangler.toml` (same honest
//!   behaviour as the P2 media crate).
//! - The performance analyst measures D1 directly (post/revision
//!   counts, content sizes, derivative rows): there is no P3
//!   performance service to call yet, so the numbers it reports are
//!   the same queries v1's performance service runs.

use async_trait::async_trait;
use serde_json::{json, Value};
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::WordlessError;

use crate::agents::util::{contrast_ratio, count_h1, luminance};
use crate::agents::{Agent, AgentContext};
use crate::types::{AgentResult, ProposalRisk};

/// Posts sampled per agent run (v1 `SAMPLE`).
const SAMPLE: i64 = 50;

/// Site base URL (the `siteurl` option, trailing slashes stripped).
/// Default `http://localhost:8787` — same as v1 (`wordless-render`
/// `feeds::site_url`); duplicated here so this crate does not depend
/// on the whole render crate for one option read.
async fn site_url(db: &dyn Db, site_id: i64) -> Result<String, WordlessError> {
    let url: String = wordless_multisite::get_site_option(
        db,
        site_id,
        "siteurl",
        "http://localhost:8787".to_string(),
    )
    .await?;
    Ok(url.trim_end_matches('/').to_string())
}

async fn count_rows(db: &dyn Db, sql: &str, params: Vec<Param>) -> Result<i64, WordlessError> {
    let row: Option<Row> = db.prepare(sql).bind(params).first::<Row>().await?;
    Ok(row.as_ref().map(|r| row_int(r, "n")).unwrap_or(0))
}

fn list_posts_opts(
    status: Option<&str>,
    post_type: Option<&str>,
    per_page: i64,
) -> wordless_cms::posts::ListPostsOptions {
    wordless_cms::posts::ListPostsOptions {
        status: status.map(str::to_string),
        post_type: post_type.map(str::to_string),
        author_id: None,
        page: Some(1),
        per_page: Some(per_page),
        include_trash: false,
    }
}

pub struct SeoAuditor;

#[async_trait(?Send)]
impl Agent for SeoAuditor {
    fn id(&self) -> &'static str {
        "seo-auditor"
    }
    fn name(&self) -> &'static str {
        "SEO Auditor"
    }
    fn description(&self) -> &'static str {
        "Audits published posts with the Milestone 7 SEO engine: per-post scores (title, description, keyword placement, headings, image alt text, links, word count, readability estimate, canonical) plus the basic hygiene checks (excerpts, categories, titles, site title/tagline, permalinks). Findings only — automated fixes are proposed as approvals, never applied silently."
    }
    fn capabilities(&self) -> &[&'static str] {
        &[
            "Score posts with the SEO engine",
            "Audit excerpts, categories and titles",
            "Check site title, tagline and permalinks",
        ]
    }
    fn risk(&self) -> ProposalRisk {
        ProposalRisk::Low
    }
    fn requires_provider(&self) -> bool {
        false
    }
    async fn run(&self, ctx: &AgentContext<'_>) -> Result<AgentResult, WordlessError> {
        let db = ctx.db;
        let base = site_url(db, ctx.site_id).await?;
        let posts = wordless_cms::posts::list_posts(
            db,
            ctx.site_id,
            list_posts_opts(Some("publish"), Some("post"), SAMPLE),
        )
        .await?;
        let mut missing_excerpt = 0i64;
        let mut missing_terms = 0i64;
        let mut empty_title = 0i64;
        let mut scored: Vec<Value> = Vec::new();
        let mut score_sum = 0i64;
        for post in &posts.posts {
            if post.excerpt.trim().is_empty() {
                missing_excerpt += 1;
            }
            if post.title.trim().is_empty() {
                empty_title += 1;
            }
            let terms = wordless_cms::terms::get_post_terms(db, ctx.site_id, post.id, Some("category")).await?;
            if terms.is_empty() {
                missing_terms += 1;
            }
            let analysis = wordless_seo::analyze::analyze_post_seo(db, ctx.site_id, post.id, &base).await?;
            scored.push(json!({"id": post.id, "title": post.title, "score": analysis.score}));
            score_sum += analysis.score;
        }
        let avg_score: Option<i64> = if scored.is_empty() {
            None
        } else {
            Some(score_sum / scored.len() as i64)
        };
        let mut worst = scored.clone();
        worst.sort_by_key(|s| s.get("score").and_then(|v| v.as_i64()).unwrap_or(0));
        worst.truncate(3);
        let title: String =
            wordless_multisite::get_site_option(db, ctx.site_id, "blogname", String::new()).await?;
        let tagline: String = wordless_multisite::get_site_option(
            db,
            ctx.site_id,
            "blogdescription",
            String::new(),
        )
        .await?;
        let structure = wordless_cms::permalinks::get_permalink_structure(db, ctx.site_id).await?;
        let mut findings = vec![
            format!("{} published posts checked (sample of {}).", posts.total, posts.posts.len()),
            match avg_score {
                None => "No posts to score.".to_string(),
                Some(avg) => format!("Average SEO score: {avg}/100 (deterministic engine — title, description, keyword, headings, alt text, links, word count, readability estimate, canonical)."),
            },
        ];
        for w in &worst {
            let wtitle = w.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let wid = w.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
            let wscore = w.get("score").and_then(|v| v.as_i64()).unwrap_or(0);
            findings.push(format!(
                "Lowest score: \"{}\" (post {wid}) — {wscore}/100.",
                if wtitle.is_empty() {
                    "(untitled)"
                } else {
                    wtitle
                }
            ));
        }
        findings.push(format!("{missing_excerpt} missing an excerpt."));
        findings.push(format!("{missing_terms} have no category."));
        findings.push(format!("{empty_title} have an empty title."));
        findings.push(if title.is_empty() {
            "Site title is not set.".to_string()
        } else {
            format!("Site title is set (\"{title}\").")
        });
        findings.push(if tagline.is_empty() {
            "Tagline is not set.".to_string()
        } else {
            "Tagline is set.".to_string()
        });
        findings.push(format!("Permalink structure: {}.", structure.as_str()));
        Ok(AgentResult {
            summary: match avg_score {
                None => format!(
                    "SEO audit: no published posts to score; {missing_excerpt} missing excerpts, {missing_terms} without a category, {empty_title} with empty titles."
                ),
                Some(avg) => format!(
                    "SEO audit: average score {avg}/100 across {} post(s); {missing_excerpt} missing excerpts, {missing_terms} without a category, {empty_title} with empty titles.",
                    scored.len()
                ),
            },
            output: Some(findings.join("\n")),
            data: Some(json!({
                "checked": posts.posts.len(),
                "total": posts.total,
                "missingExcerpt": missing_excerpt,
                "missingTerms": missing_terms,
                "emptyTitle": empty_title,
                "titleSet": !title.is_empty(),
                "taglineSet": !tagline.is_empty(),
                "permalinkStructure": structure.as_str(),
                "averageScore": avg_score,
                "scores": scored,
                "worstPosts": worst,
            })),
            proposals: None,
            usage: None,
        })
    }
}

pub struct SecurityScanner;

#[async_trait(?Send)]
impl Agent for SecurityScanner {
    fn id(&self) -> &'static str {
        "security-scanner"
    }
    fn name(&self) -> &'static str {
        "Security Scanner"
    }
    fn description(&self) -> &'static str {
        "Runs the real Milestone 9 security checks: the security-headers policy applied to every response, login lockout configuration and currently locked keys, two-factor status per administrator, the upload allowlist in force, and whether a user with the default login name \"admin\" exists (login name only — passwords are never tested). It does not guess at password strength (hashes are opaque) and does not invent vulnerabilities."
    }
    fn capabilities(&self) -> &[&'static str] {
        &[
            "Check security headers, lockout and 2FA status",
            "Check the upload allowlist and the default admin login",
            "Count administrators, active sessions, pending and spam comments",
        ]
    }
    fn risk(&self) -> ProposalRisk {
        ProposalRisk::Low
    }
    fn requires_provider(&self) -> bool {
        false
    }
    async fn run(&self, ctx: &AgentContext<'_>) -> Result<AgentResult, WordlessError> {
        let db = ctx.db;
        // The M9 security overview (wordless-security) is the owning
        // service for lockout state, sessions, headers and the
        // default-admin flag; per-admin 2FA still comes from
        // wordless-auth, which owns users and TOTP.
        let overview = wordless_security::security_overview(db).await?;
        let config = overview.lockout.config;
        let locked_keys = overview.lockout.locked_keys.len() as i64;
        let header_names: Vec<String> = wordless_kernel::security::security_headers()
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        let csp_len = overview.headers.policy.len();
        let admins_with_2fa = overview.two_factor.iter().filter(|u| u.enabled).count() as i64;
        let admins = overview.two_factor.len() as i64;
        // Total user count still comes from the user list (the
        // overview carries 2FA coverage, not the user total).
        let users = wordless_auth::list_users(db, 1, 1).await?;
        let sessions = overview.sessions.active;
        let pending = wordless_cms::comments::list_comments(
            db,
            ctx.site_id,
            wordless_cms::comments::ListCommentsOptions {
                post_id: None,
                status: Some("pending".to_string()),
                page: Some(1),
                per_page: Some(1),
            },
        )
        .await?;
        let spam = wordless_cms::comments::list_comments(
            db,
            ctx.site_id,
            wordless_cms::comments::ListCommentsOptions {
                post_id: None,
                status: Some("spam".to_string()),
                page: Some(1),
                per_page: Some(1),
            },
        )
        .await?;
        let default_admin = overview.default_admin_login_exists;
        let findings = vec![
            format!(
                "Security headers applied to every response: {} (CSP is {csp_len} characters; the policy allows inline styles/scripts the admin and analytics need — see docs/security/security.md).",
                header_names.join(", ")
            ),
            format!(
                "Login lockout: {} failed attempts in {} minutes locks a login name/IP for {} minutes (doubling on consecutive lockouts, capped at 60). {locked_keys} key(s) currently locked.",
                config.max_attempts, config.window_minutes, config.lockout_minutes
            ),
            format!(
                "Two-factor authentication: {admins_with_2fa} of {} administrator(s) have TOTP enabled.",
                admins
            ),
            format!(
                "Upload allowlist in force: {} (max {} MB) plus magic-byte sniffing.",
                wordless_media::service::ALLOWED_MIME_TYPES.join(", "),
                wordless_media::service::MAX_UPLOAD_BYTES / 1024 / 1024
            ),
            if default_admin {
                "A user with the default login name \"admin\" exists — rename it (passwords are never tested by this check).".to_string()
            } else {
                "No user with the default login name \"admin\" exists.".to_string()
            },
            format!(
                "{} users in total; {} administrator{}.",
                users.total,
                admins,
                if admins == 1 { "" } else { "s" }
            ),
            format!("{sessions} active sessions."),
            format!("{} comments awaiting moderation; {} marked spam.", pending.total, spam.total),
            if admins > 0 && admins_with_2fa < admins {
                "Not every administrator has two-factor authentication enabled.".to_string()
            } else {
                "Every administrator has two-factor authentication enabled, or there are no administrators.".to_string()
            },
        ];
        Ok(AgentResult {
            summary: format!(
                "Security scan: headers on, lockout {}/{}min, 2FA {}/{} admins, {sessions} active sessions, {} comments pending, {} spam.",
                config.max_attempts,
                config.window_minutes,
                admins_with_2fa,
                admins,
                pending.total,
                spam.total
            ),
            output: Some(findings.join("\n")),
            data: Some(json!({
                "users": users.total,
                "administrators": admins,
                "administratorsWith2fa": admins_with_2fa,
                "activeSessions": sessions,
                "pendingComments": pending.total,
                "spamComments": spam.total,
                "headers": header_names,
                "lockout": {
                    "maxAttempts": config.max_attempts,
                    "windowMinutes": config.window_minutes,
                    "lockoutMinutes": config.lockout_minutes,
                },
                "lockedKeys": locked_keys,
                "uploadAllowlist": wordless_media::service::ALLOWED_MIME_TYPES,
                "maxUploadBytes": wordless_media::service::MAX_UPLOAD_BYTES,
                "defaultAdminLoginExists": default_admin,
            })),
            proposals: None,
            usage: None,
        })
    }
}

pub struct PerformanceAnalyst;

#[async_trait(?Send)]
impl Agent for PerformanceAnalyst {
    fn id(&self) -> &'static str {
        "performance-analyst"
    }
    fn name(&self) -> &'static str {
        "Performance Analyst"
    }
    fn description(&self) -> &'static str {
        "Measures real numbers from D1 and R2: post and revision counts, media files and total media bytes, comments, average and largest post content size, public-page cache configuration, image-derivative status, and R2 object count. Cache hit rates are NOT observable from inside the Worker, so the agent says so instead of inventing a number. Core Web Vitals (real-user timing) are not measured by the Worker."
    }
    fn capabilities(&self) -> &[&'static str] {
        &[
            "Count posts, revisions, media and comments",
            "Total media bytes and R2 object count",
            "Cache configuration and image-derivative status",
            "Average and largest post content size",
        ]
    }
    fn risk(&self) -> ProposalRisk {
        ProposalRisk::Low
    }
    fn requires_provider(&self) -> bool {
        false
    }
    async fn run(&self, ctx: &AgentContext<'_>) -> Result<AgentResult, WordlessError> {
        let db = ctx.db;
        let posts =
            wordless_cms::posts::list_posts(db, ctx.site_id, list_posts_opts(Some("publish"), Some("post"), 1))
                .await?;
        let revisions = count_rows(
            db,
            "SELECT COUNT(*) AS n FROM wp_posts WHERE post_type = 'revision' AND site_id = ?",
            vec![Param::Integer(ctx.site_id)],
        )
        .await?;
        let size_row: Option<Row> = db
            .prepare("SELECT AVG(LENGTH(post_content)) AS avg, MAX(LENGTH(post_content)) AS max FROM wp_posts WHERE post_type IN ('post', 'page') AND post_status = 'publish' AND site_id = ?")
            .bind(vec![Param::Integer(ctx.site_id)])
            .first::<Row>()
            .await?;
        let avg_chars: Option<i64> = size_row.as_ref().and_then(|r| {
            r.get("avg")
                .and_then(|v| v.as_f64())
                .map(|f| f.round() as i64)
        });
        let max_chars: i64 = size_row.as_ref().map(|r| row_int(r, "max")).unwrap_or(0);
        let media = wordless_media::service::list_media(
            db,
            ctx.site_id,
            wordless_media::service::ListMediaOpts {
                page: Some(1),
                per_page: Some(100),
                mime_type: None,
            },
        )
        .await?;
        let media_bytes: i64 = media.media.iter().map(|m| m.size).sum();
        let comments = wordless_cms::comments::list_comments(
            db,
            ctx.site_id,
            wordless_cms::comments::ListCommentsOptions {
                post_id: None,
                status: None,
                page: Some(1),
                per_page: Some(1),
            },
        )
        .await?;
        let derivatives =
            count_rows(db, "SELECT COUNT(*) AS n FROM wl_media_derivatives", vec![]).await?;
        // Cache config mirrors wordless-render's PUBLIC_CACHE_MAX_AGE /
        // PUBLIC_CACHE_STALE_WHILE_REVALIDATE (60 s / 60 s); the Images
        // binding is not declared in wrangler.toml, so derivatives are
        // honestly reported unbound (P2 media behaviour).
        let cache_max_age: u32 = 60;
        let cache_swr: u32 = 60;
        let r2_line = match ctx.object_count {
            Some(n) => format!("R2 holds {n} object(s)."),
            None => {
                "R2 object count is not measured from inside this crate — no number is reported."
                    .to_string()
            }
        };
        let findings = [
            format!("{} published posts; {revisions} revisions stored.", posts.total),
            format!(
                "{} media files ({} sampled), {media_bytes} bytes in the sample; {r2_line}",
                media.total,
                media.media.len()
            ),
            format!("{} comments in total.", comments.total),
            match avg_chars {
                None => "No published posts to measure content size.".to_string(),
                Some(avg) => format!(
                    "Published post/page content: average {avg} characters, largest {max_chars} characters."
                ),
            },
            format!(
                "Public-page cache: anonymous GETs cached {cache_max_age}s with stale-while-revalidate {cache_swr}s; publishing purges the post URL, home and affected term pages."
            ),
            format!(
                "Image derivatives: Cloudflare Images is NOT bound — the media endpoint serves originals; {derivatives} derivative(s) stored."
            ),
            "Cache hit rate is not observable from inside the Worker — no hit/miss figure is reported.".to_string(),
            if revisions > 100 {
                "Over 100 revisions — pruning old revisions would shrink the database.".to_string()
            } else {
                "Revision count is modest.".to_string()
            },
        ];
        Ok(AgentResult {
            summary: format!(
                "Performance snapshot: {} posts, {revisions} revisions, {} media files ({media_bytes} bytes sampled), {} comments; cache {cache_max_age}s; derivatives unbound.",
                posts.total,
                media.total,
                comments.total
            ),
            output: Some(findings.join("\n")),
            data: Some(json!({
                "posts": posts.total,
                "revisions": revisions,
                "mediaFiles": media.total,
                "mediaBytesSampled": media_bytes,
                "r2Objects": ctx.object_count,
                "comments": comments.total,
                "averageContentChars": avg_chars,
                "largestContentChars": max_chars,
                "cache": {"maxAgeSeconds": cache_max_age, "staleWhileRevalidateSeconds": cache_swr},
                "derivatives": {"bound": false, "stored": derivatives},
            })),
            proposals: None,
            usage: None,
        })
    }
}

pub struct AccessibilityChecker;

#[async_trait(?Send)]
impl Agent for AccessibilityChecker {
    fn id(&self) -> &'static str {
        "accessibility-checker"
    }
    fn name(&self) -> &'static str {
        "Accessibility Checker"
    }
    fn description(&self) -> &'static str {
        "Checks measurable accessibility basics: media items missing alt text, posts with empty titles, and heading use in recent posts. It does not render pages, so colour contrast on the live site is checked by the design advisor from the theme instead."
    }
    fn capabilities(&self) -> &[&'static str] {
        &[
            "Media alt-text coverage",
            "Empty titles",
            "Heading use in recent posts",
        ]
    }
    fn risk(&self) -> ProposalRisk {
        ProposalRisk::Low
    }
    fn requires_provider(&self) -> bool {
        false
    }
    async fn run(&self, ctx: &AgentContext<'_>) -> Result<AgentResult, WordlessError> {
        let db = ctx.db;
        let media = wordless_media::service::list_media(
            db,
            ctx.site_id,
            wordless_media::service::ListMediaOpts {
                page: Some(1),
                per_page: Some(100),
                mime_type: None,
            },
        )
        .await?;
        let missing_alt = media
            .media
            .iter()
            .filter(|m| m.alt.trim().is_empty())
            .count() as i64;
        let posts =
            wordless_cms::posts::list_posts(db, ctx.site_id, list_posts_opts(Some("publish"), Some("post"), 20))
                .await?;
        let empty_title = posts
            .posts
            .iter()
            .filter(|p| p.title.trim().is_empty())
            .count() as i64;
        let h1_count: usize = posts.posts.iter().map(|p| count_h1(&p.content)).sum();
        let findings = [
            format!(
                "{} media files ({} sampled); {missing_alt} missing alt text.",
                media.total,
                media.media.len()
            ),
            format!(
                "{} recent posts checked; {empty_title} with an empty title.",
                posts.posts.len()
            ),
            format!("{h1_count} <h1> headings across the sampled posts (more than one per post is usually a mistake)."),
        ];
        Ok(AgentResult {
            summary: format!(
                "Accessibility check: {missing_alt} media items missing alt text, {empty_title} posts with empty titles, {h1_count} <h1> headings in the sample."
            ),
            output: Some(findings.join("\n")),
            data: Some(json!({
                "mediaChecked": media.media.len(),
                "mediaTotal": media.total,
                "missingAlt": missing_alt,
                "postsChecked": posts.posts.len(),
                "emptyTitle": empty_title,
                "h1Count": h1_count,
            })),
            proposals: None,
            usage: None,
        })
    }
}

pub struct AnalyticsReporter;

#[async_trait(?Send)]
impl Agent for AnalyticsReporter {
    fn id(&self) -> &'static str {
        "analytics-reporter"
    }
    fn name(&self) -> &'static str {
        "Analytics Reporter"
    }
    fn description(&self) -> &'static str {
        "Reports the site's own counts: posts by status, comments by status, users by role. This is on-site activity, not visitor analytics — external analytics integrations arrive in Milestone 8."
    }
    fn capabilities(&self) -> &[&'static str] {
        &["Posts by status", "Comments by status", "Users by role"]
    }
    fn risk(&self) -> ProposalRisk {
        ProposalRisk::Low
    }
    fn requires_provider(&self) -> bool {
        false
    }
    async fn run(&self, ctx: &AgentContext<'_>) -> Result<AgentResult, WordlessError> {
        let db = ctx.db;
        let mut by_status = serde_json::Map::new();
        for status in ["publish", "draft", "private", "trash"] {
            let list =
                wordless_cms::posts::list_posts(db, ctx.site_id, list_posts_opts(Some(status), Some("post"), 1))
                    .await?;
            by_status.insert(status.to_string(), json!(list.total));
        }
        let mut comments_by_status = serde_json::Map::new();
        for status in ["approved", "pending", "spam", "trash"] {
            let list = wordless_cms::comments::list_comments(
                db,
            ctx.site_id,
                wordless_cms::comments::ListCommentsOptions {
                    post_id: None,
                    status: Some(status.to_string()),
                    page: Some(1),
                    per_page: Some(1),
                },
            )
            .await?;
            comments_by_status.insert(status.to_string(), json!(list.total));
        }
        let users = wordless_auth::list_users(db, 1, 100).await?;
        let mut users_by_role = serde_json::Map::new();
        for u in &users.users {
            let key = u.role.as_str().to_string();
            let n = users_by_role
                .get(&key)
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            users_by_role.insert(key, json!(n + 1));
        }
        let published = by_status
            .get("publish")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let approved = comments_by_status
            .get("approved")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let lines = [
            format!(
                "Posts: {}.",
                by_status
                    .iter()
                    .map(|(k, v)| format!("{k} {v}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            format!(
                "Comments: {}.",
                comments_by_status
                    .iter()
                    .map(|(k, v)| format!("{k} {v}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            format!(
                "Users: {} ({} total).",
                users_by_role
                    .iter()
                    .map(|(k, v)| format!("{k} {v}"))
                    .collect::<Vec<_>>()
                    .join(", "),
                users.total
            ),
        ];
        Ok(AgentResult {
            summary: format!(
                "Site report: {published} published posts, {approved} approved comments, {} users.",
                users.total
            ),
            output: Some(lines.join("\n")),
            data: Some(json!({
                "postsByStatus": by_status,
                "commentsByStatus": comments_by_status,
                "usersByRole": users_by_role,
                "usersTotal": users.total,
            })),
            proposals: None,
            usage: None,
        })
    }
}

pub struct BackupAdvisor;

#[async_trait(?Send)]
impl Agent for BackupAdvisor {
    fn id(&self) -> &'static str {
        "backup-advisor"
    }
    fn name(&self) -> &'static str {
        "Backup Advisor"
    }
    fn description(&self) -> &'static str {
        "Sizes what a backup would need to cover (posts, media files and bytes, users, comments), reports the real backup registry (how many backups exist, when the latest was verified), and recommends a cadence. Backup export, verification and restore are built in (Milestone 9) — this agent advises, it does not back anything up."
    }
    fn capabilities(&self) -> &[&'static str] {
        &[
            "Size the site's content",
            "Report backup registry state",
            "Recommend a backup cadence",
        ]
    }
    fn risk(&self) -> ProposalRisk {
        ProposalRisk::Low
    }
    fn requires_provider(&self) -> bool {
        false
    }
    async fn run(&self, ctx: &AgentContext<'_>) -> Result<AgentResult, WordlessError> {
        let db = ctx.db;
        let posts = wordless_cms::posts::list_posts(
            db,
            ctx.site_id,
            wordless_cms::posts::ListPostsOptions {
                status: None,
                post_type: None,
                author_id: None,
                page: Some(1),
                per_page: Some(1),
                include_trash: false,
            },
        )
        .await?;
        let media = wordless_media::service::list_media(
            db,
            ctx.site_id,
            wordless_media::service::ListMediaOpts {
                page: Some(1),
                per_page: Some(100),
                mime_type: None,
            },
        )
        .await?;
        let media_bytes: i64 = media.media.iter().map(|m| m.size).sum();
        let users = wordless_auth::list_users(db, 1, 1).await?;
        let comments = wordless_cms::comments::list_comments(
            db,
            ctx.site_id,
            wordless_cms::comments::ListCommentsOptions {
                post_id: None,
                status: None,
                page: Some(1),
                per_page: Some(1),
            },
        )
        .await?;
        // The backup registry lives in wordless-backup; newest first.
        let backup_rows = wordless_backup::list_backups(db).await?;
        let verified = backup_rows.iter().filter(|b| b.verified).count() as i64;
        let latest = backup_rows.first();
        let cadence = if posts.total > 50 || media.total > 100 {
            "daily"
        } else {
            "weekly"
        };
        let mut findings = vec![
            format!(
                "Content to protect: {} posts/pages, {} media files ({media_bytes} bytes sampled), {} users, {} comments.",
                posts.total, media.total, users.total, comments.total
            ),
            format!("Backups on record: {} ({verified} verified).", backup_rows.len()),
        ];
        match latest {
            None => findings.push(
                "No backup has been created yet — create one from the admin Backup page."
                    .to_string(),
            ),
            Some(backup) => {
                findings.push(format!(
                    "Latest backup: {} ({} bytes, {} tables, {} media files) created {}{}.",
                    backup.id,
                    backup.size,
                    backup.tables.len(),
                    backup.media_count,
                    backup.created_at,
                    if backup.verified {
                        format!(
                            ", verified {}",
                            backup.verified_at.as_deref().unwrap_or("unknown")
                        )
                    } else {
                        " — NOT verified yet; run verify before relying on it".to_string()
                    }
                ));
            }
        }
        findings.push(format!("Recommended backup cadence: {cadence}."));
        Ok(AgentResult {
            summary: format!(
                "Backup advice: {} posts/pages and {} media files to protect; {} backup(s) on record ({verified} verified); {cadence} backups recommended.",
                posts.total,
                media.total,
                backup_rows.len()
            ),
            output: Some(findings.join("\n")),
            data: Some(json!({
                "posts": posts.total,
                "mediaFiles": media.total,
                "mediaBytesSampled": media_bytes,
                "users": users.total,
                "comments": comments.total,
                "recommendedCadence": cadence,
                "backups": backup_rows.len(),
                "verifiedBackups": verified,
                "latestBackupId": latest.map(|b| b.id.clone()),
                "latestBackupVerified": latest.map(|b| b.verified).unwrap_or(false),
            })),
            proposals: None,
            usage: None,
        })
    }
}

pub struct MigrationAssistant;

#[async_trait(?Send)]
impl Agent for MigrationAssistant {
    fn id(&self) -> &'static str {
        "migration-assistant"
    }
    fn name(&self) -> &'static str {
        "Migration Assistant"
    }
    fn description(&self) -> &'static str {
        "Reports migration readiness: how much content a WordPress import would need to carry, and what the conversion engine has already converted. The WordPress importer (WXR + database dump) is built in — see the admin Import page and docs/import/wordpress-import.md."
    }
    fn capabilities(&self) -> &[&'static str] {
        &["Count importable content", "Summarise past conversions"]
    }
    fn risk(&self) -> ProposalRisk {
        ProposalRisk::Low
    }
    fn requires_provider(&self) -> bool {
        false
    }
    async fn run(&self, ctx: &AgentContext<'_>) -> Result<AgentResult, WordlessError> {
        let db = ctx.db;
        let posts = wordless_cms::posts::list_posts(
            db,
            ctx.site_id,
            wordless_cms::posts::ListPostsOptions {
                status: None,
                post_type: None,
                author_id: None,
                page: Some(1),
                per_page: Some(1),
                include_trash: false,
            },
        )
        .await?;
        let comments = wordless_cms::comments::list_comments(
            db,
            ctx.site_id,
            wordless_cms::comments::ListCommentsOptions {
                post_id: None,
                status: None,
                page: Some(1),
                per_page: Some(1),
            },
        )
        .await?;
        let users = wordless_auth::list_users(db, 1, 1).await?;
        let media = wordless_media::service::list_media(
            db,
            ctx.site_id,
            wordless_media::service::ListMediaOpts {
                page: Some(1),
                per_page: Some(1),
                mime_type: None,
            },
        )
        .await?;
        let jobs = wordless_conversion_jobs::list_conversion_jobs(db, 1, 100).await?;
        let total_jobs = jobs.total as i64;
        let done = jobs
            .jobs
            .iter()
            .filter(|j| j.status == wordless_conversion_jobs::ConversionJobStatus::Done)
            .count() as i64;
        let failed = jobs
            .jobs
            .iter()
            .filter(|j| j.status == wordless_conversion_jobs::ConversionJobStatus::Failed)
            .count() as i64;
        let lines = [
            format!(
                "Content on this site: {} posts/pages, {} comments, {} users, {} media files.",
                posts.total, comments.total, users.total, media.total
            ),
            format!("Conversion engine history: {total_jobs} jobs ({done} done, {failed} failed)."),
            "Readiness: export a WordPress WXR file (or a mysqldump of the 12 wp_* tables) from the source site and upload it on the admin Import page; plugin/theme conversion is handled by the conversion engine (Milestone 4) first. Imported users get a forced password reset — WordPress phpass hashes are not portable.".to_string(),
        ];
        Ok(AgentResult {
            summary: format!(
                "Migration readiness: {} posts/pages, {} comments, {} users, {} media files; {done} past conversions done.",
                posts.total, comments.total, users.total, media.total
            ),
            output: Some(lines.join("\n")),
            data: Some(json!({
                "posts": posts.total,
                "comments": comments.total,
                "users": users.total,
                "media": media.total,
                "conversionJobs": total_jobs,
                "conversionsDone": done,
                "conversionsFailed": failed,
            })),
            proposals: None,
            usage: None,
        })
    }
}

pub struct CodeReviewer;

#[async_trait(?Send)]
impl Agent for CodeReviewer {
    fn id(&self) -> &'static str {
        "code-reviewer"
    }
    fn name(&self) -> &'static str {
        "Code Reviewer"
    }
    fn description(&self) -> &'static str {
        "Reviews the site's installed extensions and conversion reports: what is active, and what the converter flagged as unconverted or warned about. It reads reports — it does not execute code."
    }
    fn capabilities(&self) -> &[&'static str] {
        &["List installed extensions", "Summarise conversion reports"]
    }
    fn risk(&self) -> ProposalRisk {
        ProposalRisk::Low
    }
    fn requires_provider(&self) -> bool {
        false
    }
    async fn run(&self, ctx: &AgentContext<'_>) -> Result<AgentResult, WordlessError> {
        let db = ctx.db;
        let extensions = wordless_integrations::list_extensions(db).await?;
        let active = extensions.iter().filter(|e| e.status == "active").count() as i64;
        let jobs = wordless_conversion_jobs::list_conversion_jobs(db, 1, 20).await?;
        let total_jobs = jobs.total as i64;
        let mut lines = vec![format!(
            "{} installed extensions ({active} active).",
            extensions.len()
        )];
        for e in &extensions {
            lines.push(format!(
                "- {} ({}) v{} — {}.",
                e.name, e.slug, e.version, e.status
            ));
        }
        lines.push(format!("{total_jobs} conversion jobs on record."));
        for job in jobs
            .jobs
            .iter()
            .filter(|j| j.status == wordless_conversion_jobs::ConversionJobStatus::Done)
            .take(3)
        {
            // Split (2026-11-20): the job report is the converter's
            // JSON (a Value), not the pipeline's struct — read the
            // counts off the JSON.
            if let Some(report) = &job.report {
                let functions = report
                    .get("converted")
                    .and_then(|c| c.get("functions"))
                    .and_then(|f| f.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let unconverted = report
                    .get("unconverted")
                    .and_then(|u| u.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let warnings = report
                    .get("warnings")
                    .and_then(|w| w.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                lines.push(format!(
                    "- {}: {} functions converted, {} unconverted, {} warnings.",
                    job.filename, functions, unconverted, warnings
                ));
            }
        }
        if extensions.is_empty() {
            lines.push("No extensions installed yet.".to_string());
        }
        Ok(AgentResult {
            summary: format!(
                "Code review: {} extensions ({active} active), {total_jobs} conversion jobs on record.",
                extensions.len()
            ),
            output: Some(lines.join("\n")),
            data: Some(json!({
                "extensions": extensions.len(),
                "active": active,
                "conversionJobs": total_jobs,
            })),
            proposals: None,
            usage: None,
        })
    }
}

pub struct DesignAdvisor;

#[async_trait(?Send)]
impl Agent for DesignAdvisor {
    fn id(&self) -> &'static str {
        "design-advisor"
    }
    fn name(&self) -> &'static str {
        "Design Advisor"
    }
    fn description(&self) -> &'static str {
        "Reads the active theme's colour palette and checks text/background contrast ratios (WCAG 4.5:1 for normal text). It reports real ratios from the theme — it does not render pages."
    }
    fn capabilities(&self) -> &[&'static str] {
        &[
            "Read the active theme palette",
            "Compute WCAG contrast ratios",
        ]
    }
    fn risk(&self) -> ProposalRisk {
        ProposalRisk::Low
    }
    fn requires_provider(&self) -> bool {
        false
    }
    async fn run(&self, ctx: &AgentContext<'_>) -> Result<AgentResult, WordlessError> {
        let db = ctx.db;
        let slug = wordless_themes::get_active_theme_slug(db, ctx.site_id).await?;
        // The palette lives in the cached theme.json on the wl_themes
        // row (migration 0005) — no R2 read is needed.
        let row: Option<Row> = db
            .prepare("SELECT theme_json FROM wl_themes WHERE slug = ? LIMIT 1")
            .bind(vec![Param::Text(slug.clone())])
            .first::<Row>()
            .await?;
        let palette: Vec<(String, String)> = row
            .as_ref()
            .and_then(|r| {
                let raw = row_text(r, "theme_json");
                if raw.is_empty() {
                    return None;
                }
                serde_json::from_str::<Value>(&raw).ok()
            })
            .and_then(|v| {
                v.get("settings")?
                    .get("color")?
                    .get("palette")?
                    .as_array()
                    .cloned()
            })
            .map(|arr| {
                arr.iter()
                    .filter_map(|p| {
                        let s = p.get("slug")?.as_str()?.to_string();
                        let c = p.get("color")?.as_str()?.to_string();
                        Some((s, c))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if palette.is_empty() {
            return Ok(AgentResult {
                summary: format!("Design check: the active theme \"{slug}\" has no colour palette to check."),
                output: Some(format!(
                    "The active theme \"{slug}\" defines no colour palette in theme.json, so there is nothing to measure."
                )),
                data: Some(json!({"theme": slug, "paletteSize": 0, "pairsChecked": 0, "failures": 0})),
                proposals: None,
                usage: None,
            });
        }
        let base = palette
            .iter()
            .find(|(s, _)| s == "base")
            .map(|(_, c)| c.clone());
        let contrast = palette
            .iter()
            .find(|(s, _)| s == "contrast")
            .map(|(_, c)| c.clone());
        let mut failures: Vec<String> = Vec::new();
        let mut pairs_checked = 0i64;
        if let (Some(base), Some(contrast)) = (&base, &contrast) {
            if let Some(ratio) = contrast_ratio(base, contrast) {
                pairs_checked += 1;
                if ratio < 4.5 {
                    failures.push(format!(
                        "base ({base}) on contrast ({contrast}): {ratio:.2}:1 — below 4.5:1."
                    ));
                }
            }
        }
        if let Some(base) = &base {
            for (entry_slug, color) in &palette {
                if entry_slug == "base" {
                    continue;
                }
                if let Some(ratio) = contrast_ratio(color, base) {
                    pairs_checked += 1;
                    if ratio < 4.5 {
                        failures.push(format!(
                            "{entry_slug} ({color}) on base ({base}): {ratio:.2}:1 — below 4.5:1."
                        ));
                    }
                }
            }
        }
        // Keep luminance import used for the palette sanity check: a
        // palette entry that is not a parseable colour is reported.
        let unparseable = palette
            .iter()
            .filter(|(_, c)| luminance(c).is_none())
            .count();
        let mut lines = vec![format!(
            "Active theme \"{slug}\" — palette of {} colours, {pairs_checked} contrast pairs checked.",
            palette.len()
        )];
        if unparseable > 0 {
            lines.push(format!(
                "{unparseable} palette colour(s) are not #rrggbb and were skipped."
            ));
        }
        if failures.is_empty() {
            lines.push("All checked pairs meet 4.5:1 for normal text.".to_string());
        } else {
            lines.push(format!(
                "{} pair(s) below 4.5:1:\n{}",
                failures.len(),
                failures
                    .iter()
                    .map(|f| format!("- {f}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        Ok(AgentResult {
            summary: if failures.is_empty() {
                format!(
                    "Design check: all {pairs_checked} contrast pairs in \"{slug}\" meet 4.5:1."
                )
            } else {
                format!(
                    "Design check: {} of {pairs_checked} contrast pairs in \"{slug}\" are below 4.5:1.",
                    failures.len()
                )
            },
            output: Some(lines.join("\n")),
            data: Some(json!({
                "theme": slug,
                "paletteSize": palette.len(),
                "pairsChecked": pairs_checked,
                "failures": failures.len(),
            })),
            proposals: None,
            usage: None,
        })
    }
}
