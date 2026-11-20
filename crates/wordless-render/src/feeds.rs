//! RSS feeds (P2).
//!
//! Port of v1 `src/modules/feeds/service.ts` (RSS only — the sitemap
//! and robots.txt moved to the SEO module in M7, see `wordless-seo`).
//! RSS 2.0 feed of the latest 20 published posts/pages. All XML is
//! built with escaping (never string-interpolated user content) and
//! uses absolute URLs from the `siteurl` option.

use wordless_db::{Db, Param, Row};
use wordless_kernel::error::WordlessError;
use wordless_kernel::time::{iso_from_ms, ms_from_iso, now_ms};

/// XML-escape a value. Port of v1 `escapeXml` (feeds/service.ts).
pub fn escape_xml(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Site base URL (the `siteurl` option, trailing slashes stripped).
/// Default `http://localhost:8787` — same as v1.
pub async fn site_url(db: &dyn Db, site_id: i64) -> Result<String, WordlessError> {
    let url: String = wordless_multisite::get_site_option(db, site_id, "siteurl", "http://localhost:8787".to_string())
        .await
        .map_err(wordless_kernel::error::WordlessError::from)?;
    Ok(url.trim_end_matches('/').to_string())
}

/// Site name (the `blogname` option). Default `Wordless`.
pub async fn site_name(db: &dyn Db, site_id: i64) -> Result<String, WordlessError> {
    wordless_multisite::get_site_option(db, site_id, "blogname", "Wordless".to_string())
        .await
        .map_err(wordless_kernel::error::WordlessError::from)
}

/// One RSS item (already-resolved absolute link).
#[derive(Debug, Clone)]
pub struct RssItem {
    pub title: String,
    pub link: String,
    /// RFC-1123 date, e.g. `Wed, 19 Nov 2026 00:00:00 GMT`.
    pub pub_date: String,
    pub description: String,
}

/// Pure RSS builder: same fields/order as v1 `rssFeed`.
pub fn build_rss_feed(name: &str, base: &str, items: &[RssItem]) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\">\n<channel>\n");
    out.push_str(&format!("  <title>{}</title>\n", escape_xml(name)));
    out.push_str(&format!("  <link>{}/</link>\n", escape_xml(base)));
    out.push_str(&format!(
        "  <description>{}</description>\n",
        escape_xml(name)
    ));
    out.push_str("  <generator>Wordless</generator>\n");
    let rendered: Vec<String> = items
        .iter()
        .map(|item| {
            format!(
                "  <item>\n    <title>{}</title>\n    <link>{}</link>\n    <guid isPermaLink=\"true\">{}</guid>\n    <pubDate>{}</pubDate>\n    <description>{}</description>\n  </item>",
                escape_xml(&item.title),
                escape_xml(&item.link),
                escape_xml(&item.link),
                escape_xml(&item.pub_date),
                escape_xml(&item.description),
            )
        })
        .collect();
    out.push_str(&rendered.join("\n"));
    if !rendered.is_empty() {
        out.push('\n');
    }
    out.push_str("</channel>\n</rss>");
    out
}

/// Parse a stored date (`2026-11-19T00:00:00.000Z` or
/// `2026-11-19 00:00:00`) to epoch ms. `ms_from_iso` only accepts the
/// `T` form, so the space form is normalised first.
pub fn parse_stored_date_ms(s: &str) -> Option<u64> {
    let t = s.trim();
    if t.len() >= 19 && t.as_bytes()[10] == b' ' {
        let mut owned = t.to_string();
        owned.replace_range(10..11, "T");
        let with_z = if owned.ends_with('Z') {
            owned
        } else {
            format!("{owned}Z")
        };
        return ms_from_iso(&with_z);
    }
    ms_from_iso(t)
}

/// Format epoch ms as an RFC-1123 UTC date (`Date.toUTCString()` in v1).
pub fn to_utc_string(ms: u64) -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let secs = ms / 1000;
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    // 1970-01-01 was a Thursday (weekday 4, Sunday = 0).
    let weekday = DAYS[((days + 4).rem_euclid(7)) as usize];
    let iso = iso_from_ms(ms);
    let year: i64 = iso[0..4].parse().unwrap_or(1970);
    let month: usize = iso[5..7].parse().unwrap_or(1);
    let day: u64 = iso[8..10].parse().unwrap_or(1);
    format!(
        "{weekday}, {day:02} {} {year} {:02}:{:02}:{:02} GMT",
        MONTHS[(month - 1).min(11)],
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

/// RSS 2.0 feed of the latest 20 published posts/pages.
/// v1 description fallback: `post.excerpt || post.content.slice(0, 500)`
/// (JS slices UTF-16 units; here it is the first 500 chars).
pub async fn rss_feed(db: &dyn Db, site_id: i64) -> Result<String, WordlessError> {
    let base = site_url(db, site_id).await?;
    let name = site_name(db, site_id).await?;
    let structure = crate::pages::get_permalink_structure(db, site_id).await?;
    let rows: Vec<Row> = db
        .prepare(
            "SELECT * FROM wp_posts WHERE site_id = ? AND post_status = 'publish' AND post_type IN ('post', 'page') \
             ORDER BY post_date DESC, ID DESC LIMIT ?",
        )
        .bind(vec![Param::Integer(site_id), Param::Integer(20)])
        .all::<Row>()
        .await
        .map_err(wordless_kernel::error::WordlessError::from)?;
    let mut items = Vec::new();
    for row in &rows {
        let post = crate::pages::row_to_post(row);
        let link = format!(
            "{}{}",
            base,
            crate::pages::post_url_with_structure(&post, &structure)
        );
        let pub_date = match parse_stored_date_ms(&post.date) {
            Some(ms) => to_utc_string(ms),
            None => to_utc_string(now_ms()),
        };
        let description = if !post.excerpt.is_empty() {
            post.excerpt.clone()
        } else {
            post.content.chars().take(500).collect()
        };
        items.push(RssItem {
            title: post.title.clone(),
            link,
            pub_date,
            description,
        });
    }
    Ok(build_rss_feed(&name, &base, &items))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::SqliteDb;
    use futures_executor::block_on;

    #[test]
    fn rss_feed_from_db_matches_v1_vector() {
        let db = SqliteDb::new();
        block_on(async {
            db.insert_post(
                "Fish & Chips <b>",
                "Body & more",
                "",
                "publish",
                "fish-chips",
                "post",
                "2026-11-19T00:00:00.000Z",
                1,
            )
            .await;
            db.insert_post(
                "Draft post",
                "Hidden",
                "",
                "draft",
                "draft-post",
                "post",
                "2026-11-19T00:00:00.000Z",
                1,
            )
            .await;
        });
        let xml = block_on(rss_feed(&db, 1)).unwrap();
        assert!(xml.contains("Fish &amp; Chips &lt;b&gt;"));
        assert!(!xml.contains("Draft post"));
        assert!(xml.contains("<generator>Wordless</generator>"));
        assert!(xml.contains("<link>http://localhost:8787/fish-chips</link>"));
        assert!(xml.contains("<pubDate>Thu, 19 Nov 2026 00:00:00 GMT</pubDate>"));
    }

    #[test]
    fn rss_shape_matches_v1() {
        let items = vec![RssItem {
            title: "Fish & Chips <b>".to_string(),
            link: "http://localhost:8787/fish-chips".to_string(),
            pub_date: "Wed, 19 Nov 2026 00:00:00 GMT".to_string(),
            description: "Body & more".to_string(),
        }];
        let xml = build_rss_feed("Wordless", "http://localhost:8787", &items);
        assert!(xml.contains("<rss version=\"2.0\">"));
        assert!(xml.contains("Fish &amp; Chips &lt;b&gt;"));
        assert!(xml.contains("<generator>Wordless</generator>"));
        assert!(xml.contains("<guid isPermaLink=\"true\">http://localhost:8787/fish-chips</guid>"));
        assert!(xml.contains("Body &amp; more"));
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(xml.contains("<link>http://localhost:8787/</link>"));
        let empty = build_rss_feed("Wordless", "http://localhost:8787", &[]);
        assert!(empty.contains("<channel>"));
        assert!(!empty.contains("<item>"));
    }

    #[test]
    fn utc_string_matches_js_toutcstring() {
        // 2026-11-19T00:00:00.000Z is a Thursday.
        assert_eq!(
            to_utc_string(1_795_046_400_000),
            "Thu, 19 Nov 2026 00:00:00 GMT"
        );
        assert_eq!(to_utc_string(0), "Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(
            parse_stored_date_ms("2026-11-19 00:00:00"),
            Some(1_795_046_400_000)
        );
        assert_eq!(
            parse_stored_date_ms("2026-11-19T00:00:00.000Z"),
            Some(1_795_046_400_000)
        );
        assert_eq!(parse_stored_date_ms("garbage"), None);
    }
}
