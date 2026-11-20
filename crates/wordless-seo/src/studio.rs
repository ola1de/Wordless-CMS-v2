//! Content studio (port of v1 `src/modules/seo/studio.ts`) —
//! templates + editorial calendar.
//!
//! Templates are real Gutenberg block-comment HTML: creating from a
//! template inserts a genuine draft post through the posts service
//! (revisions, slug generation and all). The calendar groups stored
//! posts by date for the next 30 days — drafts with a future date are
//! the schedule, since Wordless has no "future" post status yet (the
//! publish date is the post_date). Ideas and AI drafts come from the
//! M6 AI task API — the studio UI calls it directly; there is no
//! separate ideas store and no fake idea is ever invented.

use serde::{Deserialize, Serialize};
use wordless_cms::posts::{create_post, CreatePostInput};
use wordless_db::row::{row_int, row_text};
use wordless_db::{Db, Param};
use wordless_kernel::error::{bad_request, WordlessError};
use wordless_kernel::time::{iso_from_ms, ms_from_iso, now_ms};

/// One content template (v1 `ContentTemplate`). Fields are `&'static
/// str` so the three templates can live in a `const` array.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentTemplate {
    pub slug: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// Suggested title (the admin may change it before creating).
    pub title: &'static str,
    /// Gutenberg block-comment HTML (real blocks, created as a draft).
    pub content: &'static str,
}

pub const CONTENT_TEMPLATES: [ContentTemplate; 3] = [
    ContentTemplate {
        slug: "announcement",
        name: "Announcement",
        description: "A short news-style announcement with a headline, intro and key details.",
        title: "Announcement: ",
        content: "<!-- wp:heading {\"level\":2} --><h2 class=\"wp-block-heading\">What is happening</h2><!-- /wp:heading -->\n<!-- wp:paragraph --><p>Write one short paragraph explaining the announcement in plain language.</p><!-- /wp:paragraph -->\n<!-- wp:heading {\"level\":2} --><h2 class=\"wp-block-heading\">Key details</h2><!-- /wp:heading -->\n<!-- wp:list --><ul class=\"wp-block-list\"><li>Detail one</li><li>Detail two</li><li>Detail three</li></ul><!-- /wp:list -->\n<!-- wp:paragraph --><p>End with what readers should do next.</p><!-- /wp:paragraph -->",
    },
    ContentTemplate {
        slug: "how-to",
        name: "How-to guide",
        description: "A step-by-step guide with an intro, numbered steps and a summary.",
        title: "How to ",
        content: "<!-- wp:paragraph --><p>One paragraph: what the reader will be able to do by the end, and who this guide is for.</p><!-- /wp:paragraph -->\n<!-- wp:heading {\"level\":2} --><h2 class=\"wp-block-heading\">Before you start</h2><!-- /wp:heading -->\n<!-- wp:list --><ul class=\"wp-block-list\"><li>What you need first</li><li>What you need second</li></ul><!-- /wp:list -->\n<!-- wp:heading {\"level\":2} --><h2 class=\"wp-block-heading\">Steps</h2><!-- /wp:heading -->\n<!-- wp:list {\"ordered\":true} --><ol class=\"wp-block-list\"><li>First step — be specific.</li><li>Second step — be specific.</li><li>Third step — be specific.</li></ol><!-- /wp:list -->\n<!-- wp:heading {\"level\":2} --><h2 class=\"wp-block-heading\">Summary</h2><!-- /wp:heading -->\n<!-- wp:paragraph --><p>One paragraph recapping the result and what to try next.</p><!-- /wp:paragraph -->",
    },
    ContentTemplate {
        slug: "news-update",
        name: "News update",
        description: "A dated news update: lead paragraph, background, and what happens next.",
        title: "News update: ",
        content: "<!-- wp:paragraph --><p>Lead paragraph: the news in one or two sentences — who, what, when.</p><!-- /wp:paragraph -->\n<!-- wp:heading {\"level\":2} --><h2 class=\"wp-block-heading\">Background</h2><!-- /wp:heading -->\n<!-- wp:paragraph --><p>Context readers need to understand why this matters.</p><!-- /wp:paragraph -->\n<!-- wp:heading {\"level\":2} --><h2 class=\"wp-block-heading\">What happens next</h2><!-- /wp:heading -->\n<!-- wp:paragraph --><p>What to expect, and when the next update will come.</p><!-- /wp:paragraph -->",
    },
];

pub fn list_templates() -> Vec<ContentTemplate> {
    CONTENT_TEMPLATES.to_vec()
}

pub fn get_template(slug: &str) -> Option<ContentTemplate> {
    CONTENT_TEMPLATES.iter().find(|t| t.slug == slug).cloned()
}

/// Create a draft post from a template (real post, real blocks).
pub async fn create_from_template(
    db: &dyn Db,
    site_id: i64,
    slug: &str,
    author_id: i64,
    title: Option<&str>,
) -> Result<wordless_cms::posts::Post, WordlessError> {
    let Some(template) = get_template(slug) else {
        return Err(bad_request(&format!(
            "Unknown content template \"{slug}\"."
        )));
    };
    let title = match title {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => template.title.to_string(),
    };
    create_post(
        db,
        site_id,
        CreatePostInput {
            title,
            content: Some(template.content.to_string()),
            status: Some("draft".to_string()),
            post_type: Some("post".to_string()),
            author_id,
            ..Default::default()
        },
    )
    .await
    .map_err(WordlessError::from)
}

/// One calendar day (v1 `CalendarDay`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarDay {
    pub date: String,
    pub posts: Vec<CalendarPost>,
}

/// One post on a calendar day.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarPost {
    pub id: i64,
    pub title: String,
    pub status: String,
    #[serde(rename = "type")]
    pub post_type: String,
    pub slug: String,
}

/// Editorial calendar (v1 `ContentCalendar`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentCalendar {
    pub from: String,
    pub to: String,
    pub days: Vec<CalendarDay>,
    pub total: i64,
}

/// Editorial calendar: stored posts (any status except trash) grouped
/// by post date, from today through `days` days ahead (clamped 1–90,
/// default 30). A draft with a future date is the scheduled item.
pub async fn content_calendar(db: &dyn Db, site_id: i64, days: i64) -> Result<ContentCalendar, WordlessError> {
    let safe_days = days.clamp(1, 90);
    let now = now_ms();
    let from_ms = now - (now % 86_400_000);
    let from = iso_from_ms(from_ms)[..10].to_string();
    let to = iso_from_ms(from_ms + ((safe_days - 1) as u64) * 86_400_000)[..10].to_string();
    let rows = db
        .query_all(
            "SELECT ID, post_title, post_status, post_type, post_name, post_date FROM wp_posts WHERE post_status != 'trash' AND post_type IN ('post', 'page') AND site_id = ? AND substr(post_date, 1, 10) >= ? AND substr(post_date, 1, 10) <= ? ORDER BY post_date ASC, ID ASC",
            &[Param::Integer(site_id), Param::Text(from.clone()), Param::Text(to.clone())],
        )
        .await
        .map_err(WordlessError::from)?;
    let mut by_date: std::collections::HashMap<String, CalendarDay> =
        std::collections::HashMap::new();
    for row in &rows {
        let date = row_text(row, "post_date")
            .chars()
            .take(10)
            .collect::<String>();
        if date.is_empty() {
            continue;
        }
        let day = by_date.entry(date.clone()).or_insert(CalendarDay {
            date,
            posts: vec![],
        });
        day.posts.push(CalendarPost {
            id: row_int(row, "ID"),
            title: row_text(row, "post_title"),
            status: row_text(row, "post_status"),
            post_type: row_text(row, "post_type"),
            slug: row_text(row, "post_name"),
        });
    }
    let mut days_out: Vec<CalendarDay> = Vec::new();
    for i in 0..safe_days {
        let d = iso_from_ms(from_ms + (i as u64) * 86_400_000)[..10].to_string();
        days_out.push(by_date.remove(&d).unwrap_or(CalendarDay {
            date: d,
            posts: vec![],
        }));
    }
    Ok(ContentCalendar {
        from,
        to,
        days: days_out,
        total: rows.len() as i64,
    })
}

/// Re-exported for tests: today's UTC date (`YYYY-MM-DD`).
pub fn today_utc() -> String {
    let now = now_ms();
    iso_from_ms(now - (now % 86_400_000))[..10].to_string()
}

/// Add whole days to a `YYYY-MM-DD` date (UTC).
pub fn add_days_utc(date: &str, days: i64) -> Option<String> {
    let ms = ms_from_iso(&format!("{date}T00:00:00.000Z"))?;
    let shifted = (ms as i64 + days * 86_400_000) as u64;
    Some(iso_from_ms(shifted)[..10].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SqliteDb;
    use futures_executor::block_on;

    #[test]
    fn lists_the_three_templates_and_creates_real_drafts_from_them() {
        let db = SqliteDb::new();
        assert_eq!(list_templates().len(), 3);
        assert_eq!(get_template("how-to").unwrap().name, "How-to guide");
        assert!(get_template("nope").is_none());
        for template in list_templates() {
            let post = block_on(create_from_template(&db, 1, &template.slug, 1, None)).unwrap();
            assert_eq!(post.status.as_str(), "draft");
            assert!(post.content.contains("<!-- wp:"));
            assert!(post.content.contains("wp-block-heading"));
        }
        assert!(block_on(create_from_template(&db, 1, "nope", 1, None))
            .unwrap_err()
            .message
            .contains("Unknown content template"));
    }

    #[test]
    fn groups_posts_by_date_for_the_next_30_days() {
        let db = SqliteDb::new();
        let today = today_utc();
        let future = add_days_utc(&today, 5).unwrap();
        let far = add_days_utc(&today, 60).unwrap();
        block_on(async {
            db.exec(
                &format!(
                    "INSERT INTO wp_posts (post_author, post_date, post_date_gmt, post_content, post_title, post_excerpt, post_status, post_name, post_modified, post_modified_gmt, post_parent, post_type, post_mime_type, comment_status) VALUES (1, '{today} 09:00:00', '{today} 09:00:00', 'Body', 'Today post', '', 'publish', 'today-post', '{today} 09:00:00', '{today} 09:00:00', 0, 'post', '', 'open'), (1, '{future} 09:00:00', '{future} 09:00:00', 'Body', 'Future draft', '', 'draft', 'future-draft', '{future} 09:00:00', '{future} 09:00:00', 0, 'post', '', 'open'), (1, '{far} 09:00:00', '{far} 09:00:00', 'Body', 'Too far', '', 'draft', 'too-far', '{far} 09:00:00', '{far} 09:00:00', 0, 'post', '', 'open')"
                ),
                &[],
            )
            .await
            .unwrap();
        });
        let calendar = block_on(content_calendar(&db, 1, 30)).unwrap();
        assert_eq!(calendar.days.len(), 30);
        assert_eq!(calendar.total, 2);
        let today_day = calendar.days.iter().find(|d| d.date == today).unwrap();
        assert_eq!(today_day.posts.len(), 1);
        let future_day = calendar.days.iter().find(|d| d.date == future).unwrap();
        assert_eq!(future_day.posts[0].title, "Future draft");
    }
}
