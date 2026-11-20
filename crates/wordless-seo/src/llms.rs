//! `llms.txt` + `llms-full.txt` (P10, doc §0.1 F).
//!
//! Generated live from published content — no stored copy to go
//! stale. Format follows the llms.txt convention: `# <site name>`,
//! a `> <tagline>` summary line, then `##` sections with
//! `- [Title](url): description` links. `llms-full.txt` adds the
//! full text of the newest posts (plain text, HTML stripped).
//!
//! Caps (documented, not hidden): 100 links per section in
//! `llms.txt`; `llms-full.txt` includes the full text of at most 50
//! posts. Products/courses are listed only when their tables exist
//! (a fresh install without commerce/school data lists nothing
//! there, rather than erroring).

use wordless_cms::permalinks::post_url;
use wordless_cms::posts::{row_to_post, Post};
use wordless_db::row::row_text;
use wordless_db::{Db, Param};
use wordless_kernel::error::WordlessError;

use wordless_seo_core::site::site_url;

const MAX_LINKS_PER_SECTION: usize = 100;
const MAX_FULL_POSTS: usize = 50;
const FULL_TEXT_CHARS: usize = 20_000;

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    // Collapse whitespace, and re-attach punctuation that the tag
    // boundaries separated ("pack ." → "pack.").
    let joined = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut fixed = joined;
    for (bad, good) in [
        (" .", "."),
        (" ,", ","),
        (" !", "!"),
        (" ?", "?"),
        (" ;", ";"),
        (" :", ":"),
    ] {
        fixed = fixed.replace(bad, good);
    }
    fixed
}

fn plain_excerpt(text: &str, max_chars: usize) -> String {
    let plain = strip_tags(text);
    if plain.chars().count() <= max_chars {
        return plain;
    }
    let truncated: String = plain.chars().take(max_chars).collect();
    format!("{}…", truncated.trim_end())
}

struct Link {
    title: String,
    url: String,
    description: String,
}

async fn published_posts(db: &dyn Db, site_id: i64, limit: i64) -> Result<Vec<Post>, WordlessError> {
    let rows = db
        .query_all(
            "SELECT * FROM wp_posts WHERE post_status = 'publish' AND post_type IN ('post', 'page') AND site_id = ? ORDER BY post_date DESC, ID DESC LIMIT ?",
            &[Param::Integer(site_id), Param::Integer(limit)],
        )
        .await
        .map_err(WordlessError::from)?;
    Ok(rows.iter().map(row_to_post).collect())
}

async fn shop_links(db: &dyn Db, base: &str) -> Vec<Link> {
    let Ok(rows) = db
        .query_all(
            "SELECT name, slug, description FROM wl_products WHERE status = 'published' ORDER BY created_at DESC, id DESC LIMIT ?",
            &[Param::Integer(MAX_LINKS_PER_SECTION as i64)],
        )
        .await
    else {
        return Vec::new();
    };
    rows.iter()
        .map(|r| Link {
            title: row_text(r, "name"),
            url: format!("{base}/shop/{}", row_text(r, "slug")),
            description: plain_excerpt(&row_text(r, "description"), 160),
        })
        .collect()
}

async fn course_links(db: &dyn Db, base: &str) -> Vec<Link> {
    let Ok(rows) = db
        .query_all(
            "SELECT title, slug, description FROM wl_courses WHERE status = 'published' ORDER BY created_at DESC, id DESC LIMIT ?",
            &[Param::Integer(MAX_LINKS_PER_SECTION as i64)],
        )
        .await
    else {
        return Vec::new();
    };
    rows.iter()
        .map(|r| Link {
            title: row_text(r, "title"),
            url: format!("{base}/school/courses/{}", row_text(r, "slug")),
            description: plain_excerpt(&row_text(r, "description"), 160),
        })
        .collect()
}

fn section(title: &str, links: &[Link]) -> String {
    if links.is_empty() {
        return String::new();
    }
    let mut out = format!("## {title}\n\n");
    for l in links {
        if l.description.is_empty() {
            out.push_str(&format!("- [{}]({})\n", l.title, l.url));
        } else {
            out.push_str(&format!("- [{}]({}): {}\n", l.title, l.url, l.description));
        }
    }
    out.push('\n');
    out
}

/// `llms.txt`: site summary + link lists (posts, pages, products,
/// courses). No full content — that is `llms-full.txt`.
pub async fn llms_txt(db: &dyn Db, site_id: i64) -> Result<String, WordlessError> {
    let base = site_url(db, site_id).await?;
    // P12: site name and tagline are per-site options.
    let site_name: String =
        wordless_multisite::get_site_option(db, site_id, "blogname", "Wordless".to_string())
            .await
            .map_err(WordlessError::from)?;
    let tagline: String = wordless_multisite::get_site_option(db, site_id, "blogdescription", String::new())
        .await
        .map_err(WordlessError::from)?;
    let posts = published_posts(db, site_id, MAX_LINKS_PER_SECTION as i64 * 2).await?;
    let mut post_links: Vec<Link> = Vec::new();
    let mut page_links: Vec<Link> = Vec::new();
    for post in &posts {
        let url = format!("{base}{}", post_url(db, site_id, post).await?);
        let desc = if post.excerpt.is_empty() {
            plain_excerpt(&post.content, 160)
        } else {
            post.excerpt.clone()
        };
        let link = Link {
            title: if post.title.is_empty() {
                "(untitled)".to_string()
            } else {
                post.title.clone()
            },
            url,
            description: desc,
        };
        if post.post_type == "page" {
            page_links.push(link);
        } else {
            post_links.push(link);
        }
        if post_links.len() >= MAX_LINKS_PER_SECTION && page_links.len() >= MAX_LINKS_PER_SECTION {
            break;
        }
    }
    post_links.truncate(MAX_LINKS_PER_SECTION);
    page_links.truncate(MAX_LINKS_PER_SECTION);
    let products = shop_links(db, &base).await;
    let courses = course_links(db, &base).await;

    let mut out = format!("# {site_name}\n\n");
    if !tagline.is_empty() {
        out.push_str(&format!("> {tagline}\n\n"));
    }
    out.push_str(&section("Pages", &page_links));
    out.push_str(&section("Posts", &post_links));
    out.push_str(&section("Products", &products));
    out.push_str(&section("Courses", &courses));
    Ok(out)
}

/// `llms-full.txt`: everything in `llms.txt` plus the full plain
/// text of the newest posts (at most 50, at most 20,000 chars each).
pub async fn llms_full_txt(db: &dyn Db, site_id: i64) -> Result<String, WordlessError> {
    let mut out = llms_txt(db, site_id).await?;
    let posts = published_posts(db, site_id, MAX_FULL_POSTS as i64).await?;
    let base = site_url(db, site_id).await?;
    let mut full_posts: Vec<&Post> = posts.iter().filter(|p| p.post_type == "post").collect();
    full_posts.truncate(MAX_FULL_POSTS);
    if !full_posts.is_empty() {
        out.push_str("## Full content\n\n");
        for post in full_posts {
            let url = format!("{base}{}", post_url(db, site_id, post).await?);
            out.push_str(&format!(
                "### [{}]({})\n\n{}\n\n",
                if post.title.is_empty() {
                    "(untitled)"
                } else {
                    &post.title
                },
                url,
                plain_excerpt(&post.content, FULL_TEXT_CHARS)
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SqliteDb;
    use futures_executor::block_on;
    use wordless_cms::posts::{create_post, CreatePostInput};

    fn make_post(db: &SqliteDb, title: &str, slug: &str, content: &str) {
        block_on(async {
            create_post(
                db,
                1,
                CreatePostInput {
                    title: title.to_string(),
                    slug: Some(slug.to_string()),
                    content: Some(content.to_string()),
                    excerpt: Some("A short excerpt.".to_string()),
                    status: Some("publish".to_string()),
                    author_id: 1,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        });
    }

    #[test]
    fn llms_lists_published_posts_and_full_has_content() {
        let db = SqliteDb::new();
        make_post(
            &db,
            "Hello Pack",
            "hello-pack",
            "<p>Welcome to the <strong>pack</strong>.</p>",
        );
        let txt = block_on(llms_txt(&db, 1)).unwrap();
        assert!(txt.starts_with("# Wordless\n\n"));
        assert!(txt.contains("## Posts"));
        assert!(txt.contains("[Hello Pack](http://localhost:8787/hello-pack)"));
        assert!(txt.contains("A short excerpt."));
        assert!(!txt.contains("<strong>"));
        let full = block_on(llms_full_txt(&db, 1)).unwrap();
        assert!(full.contains("## Full content"));
        assert!(full.contains("Welcome to the pack."));
        assert!(!full.contains("<p>"));
    }

    #[test]
    fn strip_tags_collapses_whitespace_and_drops_markup() {
        assert_eq!(strip_tags("<h1>Hi</h1><p>there  you</p>"), "Hi there you");
        assert_eq!(plain_excerpt("<p>short</p>", 160), "short");
    }

    #[test]
    fn empty_site_still_renders_a_valid_document() {
        let db = SqliteDb::new();
        let txt = block_on(llms_txt(&db, 1)).unwrap();
        assert!(txt.starts_with("# Wordless\n\n"));
        assert!(!txt.contains("## Posts"));
    }
}
