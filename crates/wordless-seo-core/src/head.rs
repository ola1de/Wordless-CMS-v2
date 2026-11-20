//! SEO head tags (port of v1 `src/modules/seo/head.ts`).
//!
//! `build_page_seo` returns what the two document builders (M2 layout
//! and themed document) need: an effective title + description (the
//! builders already emit `<title>` and `<meta name="description">`,
//! appending `" — <site name>"` to the title), plus the remaining head
//! tags (canonical, robots, Open Graph, Twitter Card, JSON-LD).
//!
//! Rules:
//! - The SEO title field is the page title WITHOUT the site name; the
//!   renderer appends the site name, exactly like WordPress does.
//! - Fallbacks, in order: SEO field → post title / excerpt → site
//!   title / tagline. A page is never given a title or description
//!   that was not stored somewhere on the site.
//! - Search and 404 pages are noindex and carry no canonical.
//! - All attribute values are escaped; JSON-LD is serialised with
//!   JSON string escaping and `<` `>` `&` escaped so a title can
//!   never break out of the `<script>` block.

use serde::{Deserialize, Serialize};
use wordless_cms::permalinks::post_url;
use wordless_cms::posts::Post;
use wordless_db::row::row_text;
use wordless_db::{Db, Param};
use wordless_kernel::error::WordlessError;
use wordless_media::service::get_media_record;

use crate::jsonld::{
    faq_json_ld, organization_json_ld, product_json_ld, validate_json_ld, website_json_ld,
};
use crate::meta::{get_post_faq, get_post_seo};

/// Page kind (v1 `PageKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PageKind {
    Home,
    Single,
    Page,
    Archive,
    Search,
    #[serde(rename = "404")]
    NotFound,
}

impl PageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PageKind::Home => "home",
            PageKind::Single => "single",
            PageKind::Page => "page",
            PageKind::Archive => "archive",
            PageKind::Search => "search",
            PageKind::NotFound => "404",
        }
    }
}

/// One breadcrumb after Home (label + absolute path).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Breadcrumb {
    pub name: String,
    pub path: String,
}

/// Product data for the `Product` JSON-LD node (P10). Carried on
/// [`PageSeoContext`] by the shop product page; `None` elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductSeo {
    pub name: String,
    pub description: String,
    /// Absolute product URL.
    pub url: String,
    pub price_minor: i64,
    pub currency: String,
    pub in_stock: bool,
    pub image_url: Option<String>,
}

/// Input to [`build_page_seo`] (v1 `PageSeoContext`).
#[derive(Debug, Clone, Default)]
pub struct PageSeoContext {
    pub kind: PageKind,
    /// Page title without the site name (e.g. post title,
    /// "Category: News").
    pub title: String,
    /// Fallback description (site tagline, term description).
    pub description: String,
    /// Canonical path (e.g. "/hello-world"); `None` = no canonical tag.
    pub canonical_path: Option<String>,
    /// The post being rendered (single/page only).
    pub post: Option<Post>,
    /// Breadcrumb trail after Home (label + absolute path).
    pub breadcrumbs: Vec<Breadcrumb>,
    /// Product data for the `Product` JSON-LD node (shop pages only).
    pub product: Option<ProductSeo>,
}

impl Default for PageKind {
    fn default() -> Self {
        PageKind::Home
    }
}

/// The page-level SEO data the renderers need (v1 `PageSeo`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageSeo {
    /// Effective `<title>` text WITHOUT the site name (renderers
    /// append it).
    pub title: String,
    pub description: String,
    /// Head tags AFTER title/description (canonical, robots, OG,
    /// Twitter, JSON-LD).
    pub head_tags: String,
    /// Visible breadcrumb navigation (`<nav aria-label="Breadcrumb">`),
    /// empty when the page has no trail beyond Home. Renderers place
    /// it at the top of `<main>`; block-theme templates do not get it
    /// (recorded limit — they render their own navigation).
    #[serde(default)]
    pub breadcrumbs_html: String,
}

/// Visible breadcrumb navigation for a trail after Home (P10).
/// Returns an empty string when the trail is empty (home page,
/// search, 404). The last crumb is plain text with
/// `aria-current="page"`; earlier crumbs are links.
pub fn render_breadcrumbs_html(site_url: &str, crumbs: &[Breadcrumb]) -> String {
    if crumbs.is_empty() {
        return String::new();
    }
    let mut items: Vec<String> = vec![format!(
        "<li><a href=\"{}/\">Home</a></li>",
        escape_html(site_url)
    )];
    for (i, c) in crumbs.iter().enumerate() {
        if i + 1 == crumbs.len() {
            items.push(format!(
                "<li aria-current=\"page\">{}</li>",
                escape_html(&c.name)
            ));
        } else {
            items.push(format!(
                "<li><a href=\"{}{}\">{}</a></li>",
                escape_html(site_url),
                escape_html(&c.path),
                escape_html(&c.name)
            ));
        }
    }
    format!(
        "<nav aria-label=\"Breadcrumb\"><ol>{}</ol></nav>",
        items.join("")
    )
}

/// HTML attribute escaping (same 5 replacements as v1 `escapeHtml`).
pub fn escape_html(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn escape_json_ld(json: &str) -> String {
    // Order matters: `<` → `\u003c`, `>` → `\u003e`, `&` → `\u0026`.
    json.replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
}

/// A JSON string literal, exactly as `JSON.stringify` produces it
/// (escapes `"`, `\`, and control characters < 0x20).
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub async fn build_page_seo(db: &dyn Db, site_id: i64, ctx: PageSeoContext) -> Result<PageSeo, WordlessError> {
    // P12: site name and URL are per-site options.
    let site_name: String = wordless_multisite::get_site_option(db, site_id, "blogname", "Wordless".to_string())
        .await
        .map_err(WordlessError::from)?;
    let site_url: String = wordless_multisite::get_site_option(db, site_id, "siteurl", "http://localhost:8787".to_string())
        .await
        .map_err(WordlessError::from)?;
    let site_url = site_url.trim_end_matches('/').to_string();
    let mut tags: Vec<String> = Vec::new();

    let mut title = ctx.title.clone();
    let mut description = ctx.description.clone();
    let mut noindex = ctx.kind == PageKind::Search || ctx.kind == PageKind::NotFound;
    let mut canonical: Option<String> = ctx
        .canonical_path
        .as_ref()
        .map(|p| format!("{site_url}{p}"));
    let mut og_title = title.clone();
    let mut og_description = description.clone();
    let mut og_image: Option<String> = None;
    let mut twitter_card = "summary".to_string();
    // Article JSON-LD fields, when the page is a post.
    let mut article: Option<(String, String, String, Option<String>)> = None;

    if let Some(post) = &ctx.post {
        let seo = get_post_seo(db, site_id, post.id).await?;
        if !seo.title.is_empty() {
            title = seo.title.clone();
        }
        if !seo.description.is_empty() {
            description = seo.description.clone();
        } else if !post.excerpt.is_empty() {
            description = post.excerpt.clone();
        }
        if !seo.canonical.is_empty() {
            canonical = Some(seo.canonical.clone());
        } else {
            canonical = Some(format!("{site_url}{}", post_url(db, site_id, post).await?));
        }
        if seo.noindex {
            noindex = true;
        }
        og_title = if !seo.og_title.is_empty() {
            seo.og_title.clone()
        } else {
            title.clone()
        };
        og_description = if !seo.og_description.is_empty() {
            seo.og_description.clone()
        } else {
            description.clone()
        };
        twitter_card = seo.twitter_card.clone();
        if seo.og_image_id > 0 {
            if let Some(record) = get_media_record(db, site_id, seo.og_image_id).await? {
                if !record.url.is_empty() {
                    og_image = Some(if record.url.starts_with("http") {
                        record.url.clone()
                    } else {
                        format!("{site_url}{}", record.url)
                    });
                }
            }
        }
        if post.post_type == "post" {
            let author_name: Option<String> = {
                let row = db
                    .query_one(
                        "SELECT display_name, user_login FROM wp_users WHERE ID = ? LIMIT 1",
                        &[Param::Integer(post.author_id)],
                    )
                    .await
                    .map_err(WordlessError::from)?;
                row.map(|r| {
                    let display = row_text(&r, "display_name");
                    if !display.is_empty() {
                        display
                    } else {
                        row_text(&r, "user_login")
                    }
                })
            };
            article = Some((
                title.clone(),
                post.date.clone(),
                post.modified.clone(),
                author_name,
            ));
        }
    }

    if let Some(canonical) = &canonical {
        tags.push(format!(
            "<link rel=\"canonical\" href=\"{}\">",
            escape_html(canonical)
        ));
    }
    if noindex {
        tags.push("<meta name=\"robots\" content=\"noindex, nofollow\">".to_string());
    }
    let og_type = if ctx.post.as_ref().is_some_and(|p| p.post_type == "post") {
        "article"
    } else {
        "website"
    };
    tags.push(format!("<meta property=\"og:type\" content=\"{og_type}\">"));
    tags.push(format!(
        "<meta property=\"og:title\" content=\"{}\">",
        escape_html(&og_title)
    ));
    if !og_description.is_empty() {
        tags.push(format!(
            "<meta property=\"og:description\" content=\"{}\">",
            escape_html(&og_description)
        ));
    }
    tags.push(format!(
        "<meta property=\"og:url\" content=\"{}\">",
        escape_html(canonical.as_deref().unwrap_or(&site_url))
    ));
    tags.push(format!(
        "<meta property=\"og:site_name\" content=\"{}\">",
        escape_html(&site_name)
    ));
    if let Some(img) = &og_image {
        tags.push(format!(
            "<meta property=\"og:image\" content=\"{}\">",
            escape_html(img)
        ));
    }
    tags.push(format!(
        "<meta name=\"twitter:card\" content=\"{twitter_card}\">"
    ));
    tags.push(format!(
        "<meta name=\"twitter:title\" content=\"{}\">",
        escape_html(&og_title)
    ));
    if !og_description.is_empty() {
        tags.push(format!(
            "<meta name=\"twitter:description\" content=\"{}\">",
            escape_html(&og_description)
        ));
    }
    if let Some(img) = &og_image {
        tags.push(format!(
            "<meta name=\"twitter:image\" content=\"{}\">",
            escape_html(img)
        ));
    }

    // JSON-LD, in v1's key order (serde_json's default map ordering
    // would sort keys, so the object is built as a string).
    // P10: WebSite carries a SearchAction; the home page also gets
    // an Organization node; posts get FAQPage when FAQs are set; shop
    // pages get a Product node from `ctx.product`.
    let mut graph: Vec<String> = vec![website_json_ld(&site_name, &site_url)];
    if ctx.kind == PageKind::Home {
        graph.push(organization_json_ld(&site_name, &site_url, None));
    }
    if let Some((headline, date_published, date_modified, author)) = &article {
        let mut fields = format!("\"@type\":\"Article\",\"headline\":{}", json_str(headline));
        if !date_published.is_empty() {
            fields.push_str(&format!(",\"datePublished\":{}", json_str(date_published)));
        }
        if !date_modified.is_empty() {
            fields.push_str(&format!(",\"dateModified\":{}", json_str(date_modified)));
        }
        if let Some(name) = author {
            fields.push_str(&format!(
                ",\"author\":{{\"@type\":\"Person\",\"name\":{}}}",
                json_str(name)
            ));
        }
        graph.push(format!("{{{fields}}}"));
    }
    if let Some(post) = &ctx.post {
        // FAQPage (best-effort: a corrupt FAQ meta value reads as
        // empty, never 500s the page).
        if let Ok(faqs) = get_post_faq(db, site_id, post.id).await {
            let pairs: Vec<(String, String)> =
                faqs.into_iter().map(|f| (f.question, f.answer)).collect();
            if let Some(faq_ld) = faq_json_ld(&pairs) {
                graph.push(faq_ld);
            }
        }
    }
    if let Some(product) = &ctx.product {
        graph.push(product_json_ld(
            &product.name,
            &product.description,
            &product.url,
            product.price_minor,
            &product.currency,
            product.in_stock,
            product.image_url.as_deref(),
        ));
    }
    let home = Breadcrumb {
        name: "Home".to_string(),
        path: "/".to_string(),
    };
    let mut all_crumbs: Vec<&Breadcrumb> = vec![&home];
    for c in &ctx.breadcrumbs {
        all_crumbs.push(c);
    }
    if all_crumbs.len() > 1 {
        let items: Vec<String> = all_crumbs
            .iter()
            .enumerate()
            .map(|(i, c)| {
                format!(
                    "{{\"@type\":\"ListItem\",\"position\":{},\"name\":{},\"item\":{}}}",
                    i + 1,
                    json_str(&c.name),
                    json_str(&format!("{site_url}{}", c.path))
                )
            })
            .collect();
        graph.push(format!(
            "{{\"@type\":\"BreadcrumbList\",\"itemListElement\":[{}]}}",
            items.join(",")
        ));
    }
    tags.push(format!(
        "<script type=\"application/ld+json\">{}</script>",
        escape_json_ld(&format!(
            "{{\"@context\":\"https://schema.org\",\"@graph\":[{}]}}",
            graph.join(",")
        ))
    ));

    Ok(PageSeo {
        title,
        description,
        head_tags: tags.join("\n"),
        breadcrumbs_html: render_breadcrumbs_html(&site_url, &ctx.breadcrumbs),
    })
}

/// The JSON-LD graph for one post, for the structured-data preview
/// (`GET /api/posts/:id/seo/json-ld`). Returns the raw graph JSON
/// (the `<script>` content, exactly as the page serves it) plus the
/// structural validation problems (empty = valid). The post is
/// rendered as a single post with its stored SEO fields.
pub async fn preview_post_json_ld(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
) -> Result<(String, Vec<String>), WordlessError> {
    use wordless_cms::posts::row_to_post;
    let row = db
        .query_one(
            "SELECT * FROM wp_posts WHERE ID = ? AND site_id = ? LIMIT 1",
            &[Param::Integer(post_id), Param::Integer(site_id)],
        )
        .await
        .map_err(WordlessError::from)?;
    let Some(row) = row else {
        return Err(wordless_kernel::error::not_found(&format!(
            "Post {post_id} not found."
        )));
    };
    let post = row_to_post(&row);
    let canonical = post_url(db, site_id, &post).await?;
    let crumbs = if post.post_type == "post" {
        vec![Breadcrumb {
            name: if post.title.is_empty() {
                "(untitled)".to_string()
            } else {
                post.title.clone()
            },
            path: canonical.clone(),
        }]
    } else {
        Vec::new()
    };
    let page = build_page_seo(
        db,
        site_id,
        PageSeoContext {
            kind: if post.post_type == "page" {
                PageKind::Page
            } else {
                PageKind::Single
            },
            title: post.title.clone(),
            description: post.excerpt.clone(),
            canonical_path: Some(canonical),
            post: Some(post),
            breadcrumbs: crumbs,
            product: None,
        },
    )
    .await?;
    let marker = "<script type=\"application/ld+json\">";
    let Some(start) = page.head_tags.find(marker) else {
        return Ok((
            String::new(),
            vec!["No JSON-LD script was emitted.".to_string()],
        ));
    };
    let rest = &page.head_tags[start + marker.len()..];
    let Some(end) = rest.find("</script>") else {
        return Ok((
            String::new(),
            vec!["JSON-LD script is not closed.".to_string()],
        ));
    };
    let graph = rest[..end].to_string();
    let problems = validate_json_ld(&graph);
    Ok((graph, problems))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::{set_post_seo, PostSeoInput};
    use crate::test_support::SqliteDb;
    use futures_executor::block_on;
    use wordless_cms::posts::{create_post, CreatePostInput};

    fn make_post(db: &SqliteDb, title: &str, excerpt: &str) -> Post {
        block_on(async {
            create_post(
                db,
                1,
                CreatePostInput {
                    title: title.to_string(),
                    content: Some("Body.".to_string()),
                    excerpt: Some(excerpt.to_string()),
                    status: Some("publish".to_string()),
                    author_id: 1,
                    ..Default::default()
                },
            )
            .await
            .unwrap()
        })
    }

    fn set_opts(db: &SqliteDb, name: &str, url: &str) {
        block_on(async {
            db.set_option_raw("blogname", &format!("\"{name}\"")).await;
            db.set_option_raw("siteurl", &format!("\"{url}\"")).await;
        });
    }

    #[test]
    fn emits_canonical_og_twitter_and_json_ld_for_a_post_with_seo_fields() {
        let db = SqliteDb::new();
        set_opts(&db, "Wolf Site", "https://example.test");
        let post = make_post(
            &db,
            "Walking the Ridge Trail at Dawn",
            "A plain walk along the ridge trail at dawn.",
        );
        block_on(set_post_seo(
            &db, 1,
            post.id,
            PostSeoInput {
                title: Some("Ridge Trail at Dawn".to_string()),
                description: Some("Walk the ridge trail at dawn with the pack.".to_string()),
                canonical: Some("https://example.test/ridge-trail".to_string()),
                twitter_card: Some("summary_large_image".to_string()),
                ..Default::default()
            },
        ))
        .unwrap();
        let page = block_on(build_page_seo(
            &db, 1,
            PageSeoContext {
                kind: PageKind::Single,
                title: post.title.clone(),
                description: post.excerpt.clone(),
                canonical_path: Some("/walking-the-ridge-trail-at-dawn".to_string()),
                post: Some(post.clone()),
                breadcrumbs: vec![Breadcrumb {
                    name: post.title.clone(),
                    path: "/walking-the-ridge-trail-at-dawn".to_string(),
                }],
                product: None,
            },
        ))
        .unwrap();
        assert_eq!(page.title, "Ridge Trail at Dawn");
        assert_eq!(
            page.description,
            "Walk the ridge trail at dawn with the pack."
        );
        assert!(page
            .head_tags
            .contains("<link rel=\"canonical\" href=\"https://example.test/ridge-trail\">"));
        assert!(page
            .head_tags
            .contains("<meta property=\"og:type\" content=\"article\">"));
        assert!(page
            .head_tags
            .contains("<meta name=\"twitter:card\" content=\"summary_large_image\">"));
        assert!(page.head_tags.contains("\"@type\":\"Article\""));
        assert!(page.head_tags.contains("\"@type\":\"BreadcrumbList\""));
        assert!(page.head_tags.contains("\"@type\":\"WebSite\""));
        assert!(!page.head_tags.contains("noindex"));
    }

    #[test]
    fn falls_back_to_post_title_and_excerpt_and_honours_noindex() {
        let db = SqliteDb::new();
        let post = make_post(&db, "Plain Post", "The excerpt.");
        block_on(set_post_seo(
            &db, 1,
            post.id,
            PostSeoInput {
                noindex: Some(true),
                ..Default::default()
            },
        ))
        .unwrap();
        let page = block_on(build_page_seo(
            &db, 1,
            PageSeoContext {
                kind: PageKind::Single,
                title: post.title.clone(),
                description: post.excerpt.clone(),
                canonical_path: Some("/plain-post".to_string()),
                post: Some(post),
                breadcrumbs: vec![],
                product: None,
            },
        ))
        .unwrap();
        assert_eq!(page.title, "Plain Post");
        assert_eq!(page.description, "The excerpt.");
        assert!(page
            .head_tags
            .contains("<meta name=\"robots\" content=\"noindex, nofollow\">"));
        assert!(page
            .head_tags
            .contains("<link rel=\"canonical\" href=\"http://localhost:8787/plain-post\">"));
    }

    #[test]
    fn marks_search_and_404_pages_noindex_with_no_canonical() {
        let db = SqliteDb::new();
        let search = block_on(build_page_seo(
            &db, 1,
            PageSeoContext {
                kind: PageKind::Search,
                title: "Search: wolves".to_string(),
                canonical_path: None,
                ..Default::default()
            },
        ))
        .unwrap();
        assert!(search.head_tags.contains("noindex"));
        assert!(!search.head_tags.contains("canonical"));
        let not_found = block_on(build_page_seo(
            &db, 1,
            PageSeoContext {
                kind: PageKind::NotFound,
                title: "Not found".to_string(),
                canonical_path: None,
                ..Default::default()
            },
        ))
        .unwrap();
        assert!(not_found.head_tags.contains("noindex"));
    }

    #[test]
    fn escapes_a_hostile_title_in_json_ld() {
        let db = SqliteDb::new();
        let post = make_post(&db, "Evil </script><script>alert(\"x\")</script>", "");
        let page = block_on(build_page_seo(
            &db, 1,
            PageSeoContext {
                kind: PageKind::Single,
                title: post.title.clone(),
                canonical_path: Some("/evil".to_string()),
                post: Some(post),
                ..Default::default()
            },
        ))
        .unwrap();
        assert!(!page.head_tags.contains("</script><script>"));
        assert!(page.head_tags.contains("\\u003c/script\\u003e"));
    }

    #[test]
    fn website_carries_search_action_and_home_carries_organization() {
        let db = SqliteDb::new();
        set_opts(&db, "Wolf Site", "https://example.test");
        let home = block_on(build_page_seo(
            &db, 1,
            PageSeoContext {
                kind: PageKind::Home,
                title: "Wolf Site".to_string(),
                ..Default::default()
            },
        ))
        .unwrap();
        assert!(home.head_tags.contains("\"@type\":\"SearchAction\""));
        assert!(home.head_tags.contains("/?s={search_term_string}"));
        assert!(home.head_tags.contains("\"@type\":\"Organization\""));
        assert!(home.breadcrumbs_html.is_empty());
        let post = make_post(&db, "Plain Post", "The excerpt.");
        let single = block_on(build_page_seo(
            &db, 1,
            PageSeoContext {
                kind: PageKind::Single,
                title: post.title.clone(),
                canonical_path: Some("/plain-post".to_string()),
                post: Some(post),
                ..Default::default()
            },
        ))
        .unwrap();
        assert!(single.head_tags.contains("\"@type\":\"SearchAction\""));
        assert!(!single.head_tags.contains("\"@type\":\"Organization\""));
    }

    #[test]
    fn faq_and_product_nodes_appear_only_when_data_exists() {
        use crate::meta::{set_post_faq, FaqItem};
        let db = SqliteDb::new();
        set_opts(&db, "Wolf Site", "https://example.test");
        let post = make_post(&db, "FAQ Post", "Answers within.");
        block_on(set_post_faq(
            &db, 1,
            post.id,
            &[FaqItem {
                question: "Is the pack real?".to_string(),
                answer: "Yes.".to_string(),
            }],
        ))
        .unwrap();
        let page = block_on(build_page_seo(
            &db, 1,
            PageSeoContext {
                kind: PageKind::Single,
                title: post.title.clone(),
                canonical_path: Some("/faq-post".to_string()),
                post: Some(post.clone()),
                breadcrumbs: vec![Breadcrumb {
                    name: post.title.clone(),
                    path: "/faq-post".to_string(),
                }],
                product: Some(ProductSeo {
                    name: "Pack Hoodie".to_string(),
                    description: "A warm hoodie.".to_string(),
                    url: "https://example.test/shop/pack-hoodie".to_string(),
                    price_minor: 329700,
                    currency: "NGN".to_string(),
                    in_stock: true,
                    image_url: None,
                }),
                ..Default::default()
            },
        ))
        .unwrap();
        assert!(page.head_tags.contains("\"@type\":\"FAQPage\""));
        assert!(page.head_tags.contains("\"@type\":\"Product\""));
        assert!(page.head_tags.contains("\"price\":\"3297.00\""));
        assert!(page
            .breadcrumbs_html
            .contains("<nav aria-label=\"Breadcrumb\">"));
        assert!(page.breadcrumbs_html.contains("aria-current=\"page\""));
        assert!(page.breadcrumbs_html.contains(">FAQ Post</li>"));
        // The preview helper returns the same graph, validated clean.
        let (graph, problems) = block_on(preview_post_json_ld(&db, 1, post.id)).unwrap();
        assert!(problems.is_empty(), "{problems:?}");
        assert!(graph.contains("\"@type\":\"FAQPage\""));
    }

    #[test]
    fn preview_returns_404_for_a_missing_post() {
        let db = SqliteDb::new();
        assert!(block_on(preview_post_json_ld(&db, 1, 9999)).is_err());
    }
}
