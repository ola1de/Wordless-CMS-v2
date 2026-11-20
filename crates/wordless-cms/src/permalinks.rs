//! Permalinks — URL building and path resolution.
//! Port of v1 `src/modules/permalinks/` (service.ts, repo.ts, types.ts).
//!
//! Structures (stored in option `permalink_structure`, default `postname`):
//!
//! - plain:      /?p=123
//! - postname:   /hello-world
//! - day-name:   /2026/11/19/hello-world
//! - month-name: /2026/11/hello-world
//!
//! Pages always use /slug. Terms use /category/slug and /tag/slug.
//!
//! Resolution order for a path: exact redirect → home → term archive →
//! date archive → dated post/page → plain slug → redirect by trailing slug
//! → not-found. Redirects are checked before 404, so renamed slugs 301.

use serde::Serialize;
use wordless_db::row::row_text;
use wordless_db::{Db, Param, Row};
use wordless_kernel::error::WordlessError;
use wordless_kernel::time::now_iso;

use crate::posts::{find_post_by_id, find_post_by_slug, row_to_post, Post};
use crate::terms::{find_term_by_slug, Term};

pub const PERMALINK_OPTION: &str = "permalink_structure";

/// Permalink structure. Serialises to the v1 strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermalinkStructure {
    Plain,
    Postname,
    #[serde(rename = "day-name")]
    DayName,
    #[serde(rename = "month-name")]
    MonthName,
}

impl PermalinkStructure {
    pub fn as_str(&self) -> &'static str {
        match self {
            PermalinkStructure::Plain => "plain",
            PermalinkStructure::Postname => "postname",
            PermalinkStructure::DayName => "day-name",
            PermalinkStructure::MonthName => "month-name",
        }
    }

    /// See `PostStatus::from_str` for why this is not `std::str::FromStr`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<PermalinkStructure> {
        match s {
            "plain" => Some(PermalinkStructure::Plain),
            "postname" => Some(PermalinkStructure::Postname),
            "day-name" => Some(PermalinkStructure::DayName),
            "month-name" => Some(PermalinkStructure::MonthName),
            _ => None,
        }
    }
}

impl<'de> serde::Deserialize<'de> for PermalinkStructure {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        PermalinkStructure::from_str(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown permalink structure \"{s}\"")))
    }
}

pub fn is_permalink_structure(value: &str) -> bool {
    PermalinkStructure::from_str(value).is_some()
}

/// P12: the permalink structure is a per-site option.
pub async fn get_permalink_structure(db: &dyn Db, site_id: i64) -> Result<PermalinkStructure, WordlessError> {
    let raw = wordless_multisite::get_site_option::<String>(db, site_id, PERMALINK_OPTION, "postname".to_string()).await?;
    Ok(PermalinkStructure::from_str(&raw).unwrap_or(PermalinkStructure::Postname))
}

/// v1 `setPermalinkStructure` throws a plain `Error` for an unknown
/// structure, which the v1 router turns into a 500 `internal_error`
/// with the generic message "An unexpected error occurred." — the
/// thrown message never reaches the client. This returns exactly that.
pub async fn set_permalink_structure(
    db: &dyn Db,
    site_id: i64,
    structure: &str,
) -> Result<PermalinkStructure, WordlessError> {
    let s = PermalinkStructure::from_str(structure).ok_or_else(|| {
        WordlessError::new("internal_error", 500, "An unexpected error occurred.")
    })?;
    wordless_multisite::set_site_option(db, site_id, PERMALINK_OPTION, &s.as_str()).await?;
    Ok(s)
}

fn date_parts(iso: &str) -> (String, String, String) {
    // Dates are ISO-8601 UTC text (e.g. 2026-11-19T00:00:00.000Z).
    (
        iso.get(0..4).unwrap_or("").to_string(),
        iso.get(5..7).unwrap_or("").to_string(),
        iso.get(8..10).unwrap_or("").to_string(),
    )
}

/// Public URL path for a post or page under the current structure.
/// A post of a registered custom post type uses the type's rewrite
/// rule (`/{rewriteSlug}/{slug}`) regardless of the permalink
/// structure; an unregistered type falls back to the v1 behaviour
/// (`/{slug}` under postname, `/?p=ID` under plain).
pub async fn post_url(db: &dyn Db, site_id: i64, post: &Post) -> Result<String, WordlessError> {
    if post.post_type == "page" {
        return Ok(format!("/{}", post.slug));
    }
    if post.post_type != "post" {
        if let Some(def) = crate::post_types::find_post_type(db, site_id, &post.post_type).await? {
            if !def.builtin {
                return Ok(crate::post_types::post_type_single_url(&def, post));
            }
        }
    }
    let structure = get_permalink_structure(db, site_id).await?;
    if structure == PermalinkStructure::Plain {
        return Ok(format!("/?p={}", post.id));
    }
    let (year, month, day) = date_parts(if post.date.is_empty() {
        &post.date_gmt
    } else {
        &post.date
    });
    match structure {
        PermalinkStructure::DayName => Ok(format!("/{year}/{month}/{day}/{}", post.slug)),
        PermalinkStructure::MonthName => Ok(format!("/{year}/{month}/{}", post.slug)),
        _ => Ok(format!("/{}", post.slug)),
    }
}

/// Public URL path for a term.
pub fn term_url(taxonomy: &str, slug: &str) -> String {
    if taxonomy == "category" {
        format!("/category/{slug}")
    } else if taxonomy == "tag" {
        format!("/tag/{slug}")
    } else {
        format!("/{taxonomy}/{slug}")
    }
}

/// Record a 301 for a published post whose slug changed. No redirect is
/// recorded for the `plain` structure (the URL is /?p=ID and never changes).
pub async fn record_slug_change(
    db: &dyn Db,
    site_id: i64,
    post: &Post,
    new_slug: &str,
) -> Result<(), WordlessError> {
    let old_path = post_url(db, site_id, post).await?;
    let new_post = Post {
        slug: new_slug.to_string(),
        ..post.clone()
    };
    let new_path = post_url(db, site_id, &new_post).await?;
    if old_path == new_path {
        return Ok(());
    }
    insert_redirect(db, site_id, &old_path, &new_path, post.id).await
}

fn normalize_path(path: &str) -> String {
    // v1: decodeURIComponent(path.replace(/^\/+|\/+$/g, ""))
    let trimmed = path.trim_matches('/');
    percent_decode(trimmed)
}

/// Minimal percent-decoding (v1 `decodeURIComponent`). Invalid `%`
/// sequences are left as-is rather than throwing, because the Rust
/// port has no `decodeURIComponent` failure mode to mirror.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| -> Option<u8> {
                match b {
                    b'0'..=b'9' => Some(b - b'0'),
                    b'a'..=b'f' => Some(b - b'a' + 10),
                    b'A'..=b'F' => Some(b - b'A' + 10),
                    _ => None,
                }
            };
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn is_digits(s: &str, len: usize) -> bool {
    s.len() == len && s.chars().all(|c| c.is_ascii_digit())
}

/// The result of resolving a public path.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ResolvedPath {
    /// 301 to the redirect target path.
    Redirect {
        target: String,
    },
    Home,
    Post {
        post: Post,
    },
    Page {
        post: Post,
    },
    Term {
        term: Term,
    },
    DateArchive {
        year: i64,
        month: Option<i64>,
    },
    /// Archive of a registered custom post type (`/{rewriteSlug}`).
    /// NEW in v2 (v1 had no CPTs); the render crate lists the type's
    /// published posts for this kind.
    PostTypeArchive {
        post_type: String,
    },
    NotFound,
}

pub async fn resolve_path(db: &dyn Db, site_id: i64, raw_path: &str) -> Result<ResolvedPath, WordlessError> {
    let path = normalize_path(raw_path);
    // 1. Exact redirect match (old permalink path).
    if let Some(target) = find_redirect_by_path(db, site_id, &format!("/{path}")).await? {
        return Ok(ResolvedPath::Redirect { target });
    }
    if path.is_empty() {
        return Ok(ResolvedPath::Home);
    }

    let segments: Vec<&str> = path.split('/').collect();
    let first = segments.first().copied().unwrap_or("");

    // 2. Term archives: /category/slug, /tag/slug.
    if (first == "category" || first == "tag") && segments.len() == 2 {
        let term = find_term_by_slug(db, site_id, first, segments[1]).await?;
        return Ok(match term {
            Some(term) => ResolvedPath::Term { term },
            None => ResolvedPath::NotFound,
        });
    }

    // 2b. Custom-taxonomy archives: /{rewriteSlug}/slug.
    if segments.len() == 2 {
        if let Some(def) = crate::taxonomies::find_taxonomy_by_rewrite(db, site_id, first).await? {
            let term = find_term_by_slug(db, site_id, &def.slug, segments[1]).await?;
            return Ok(match term {
                Some(term) => ResolvedPath::Term { term },
                None => ResolvedPath::NotFound,
            });
        }
    }

    // 3. Date archives: /YYYY/ and /YYYY/MM/.
    if is_digits(first, 4) && segments.len() <= 2 {
        let year: i64 = first.parse().unwrap_or(0);
        let month = if segments.len() == 2 && is_digits(segments[1], 2) {
            Some(segments[1].parse::<i64>().unwrap_or(0))
        } else {
            None
        };
        if month.is_none_or(|m| (1..=12).contains(&m)) {
            return Ok(ResolvedPath::DateArchive { year, month });
        }
        return Ok(ResolvedPath::NotFound);
    }

    // 4. Dated post: /YYYY/MM/DD/slug or /YYYY/MM/slug — verify the date
    //    matches the post's date so a slug cannot be hijacked by any date.
    let slug = segments.last().copied().unwrap_or("");
    if segments.len() == 3 || segments.len() == 4 {
        let dated = if segments.len() == 4 {
            is_digits(segments[0], 4) && is_digits(segments[1], 2) && is_digits(segments[2], 2)
        } else {
            is_digits(segments[0], 4) && is_digits(segments[1], 2)
        };
        if dated {
            let post = find_post_by_slug(db, site_id, slug).await?;
            if let Some(post) = post {
                if post.status == crate::posts::PostStatus::Publish {
                    let (year, month, day) = date_parts(if post.date.is_empty() {
                        &post.date_gmt
                    } else {
                        &post.date
                    });
                    let date_matches = if segments.len() == 4 {
                        segments[0] == year && segments[1] == month && segments[2] == day
                    } else {
                        segments[0] == year && segments[1] == month
                    };
                    if date_matches {
                        return Ok(if post.post_type == "page" {
                            ResolvedPath::Page { post }
                        } else {
                            ResolvedPath::Post { post }
                        });
                    }
                }
            }
            return Ok(ResolvedPath::NotFound);
        }
    }

    // 4b. Custom post type single: /{rewriteSlug}/slug.
    // (NEW in v2 — v1 had no CPTs. Checked before the plain-slug
    // fallback; a rewrite rule wins over a colliding post slug.)
    if segments.len() == 2 {
        if let Some(def) = crate::post_types::find_post_type_by_rewrite(db, site_id, first).await? {
            let post = crate::posts::find_post_by_slug_and_type(db, site_id, segments[1], &def.slug).await?;
            return Ok(match post {
                Some(post) if post.status == crate::posts::PostStatus::Publish => {
                    ResolvedPath::Post { post }
                }
                _ => ResolvedPath::NotFound,
            });
        }
    }

    // 4c. Custom post type archive: /{rewriteSlug}.
    // (NEW in v2 — v1 had no CPTs. Checked before the plain-slug
    // fallback; a rewrite rule wins over a colliding post slug.)
    if segments.len() == 1 {
        if let Some(def) = crate::post_types::find_post_type_by_rewrite(db, site_id, first).await? {
            if def.has_archive {
                return Ok(ResolvedPath::PostTypeArchive {
                    post_type: def.slug,
                });
            }
        }
    }

    // 5. Plain slug: /hello-world (post or page).
    if segments.len() == 1 {
        let post = find_post_by_slug(db, site_id, slug).await?;
        if let Some(post) = post {
            if post.status == crate::posts::PostStatus::Publish {
                return Ok(if post.post_type == "page" {
                    ResolvedPath::Page { post }
                } else {
                    ResolvedPath::Post { post }
                });
            }
        }
        // 6. Redirect by trailing slug (covers renamed slugs under any structure).
        if let Some(target) = find_redirect_by_slug(db, site_id, slug).await? {
            return Ok(ResolvedPath::Redirect { target });
        }
        return Ok(ResolvedPath::NotFound);
    }

    Ok(ResolvedPath::NotFound)
}

/// Resolve /?p=ID (plain structure) to a published post or page.
pub async fn resolve_post_by_id(db: &dyn Db, site_id: i64, id: i64) -> Result<Option<Post>, WordlessError> {
    let post = find_post_by_id(db, site_id, id).await?;
    match post {
        Some(p)
            if p.status == crate::posts::PostStatus::Publish
                && (p.post_type == "post" || p.post_type == "page") =>
        {
            Ok(Some(p))
        }
        _ => Ok(None),
    }
}

/// Published posts whose post_date starts with the given prefix (YYYY or YYYY-MM).
pub async fn list_posts_by_date_prefix(
    db: &dyn Db,
    site_id: i64,
    prefix: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<Post>, WordlessError> {
    let rows: Vec<Row> = db
        .prepare(
            "SELECT * FROM wp_posts
              WHERE post_type = 'post' AND post_status = 'publish' AND post_date LIKE ? AND site_id = ?
              ORDER BY post_date DESC, ID DESC LIMIT ? OFFSET ?",
        )
        .bind(vec![
            Param::Text(format!("{prefix}%")),
            Param::Integer(site_id),
            Param::Integer(limit),
            Param::Integer(offset),
        ])
        .all()
        .await?;
    Ok(rows.iter().map(row_to_post).collect())
}

// ---------------------------------------------------------------- redirects repo

pub async fn insert_redirect(
    db: &dyn Db,
    site_id: i64,
    old_path: &str,
    new_path: &str,
    post_id: i64,
) -> Result<(), WordlessError> {
    db.prepare(
        "INSERT OR IGNORE INTO wl_redirects (site_id, old_path, new_path, post_id, created_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(vec![
        Param::Integer(site_id),
        Param::Text(old_path.to_string()),
        Param::Text(new_path.to_string()),
        Param::Integer(post_id),
        Param::Text(now_iso()),
    ])
    .run()
    .await?;
    Ok(())
}

pub async fn find_redirect_by_path(
    db: &dyn Db,
    site_id: i64,
    path: &str,
) -> Result<Option<String>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT new_path FROM wl_redirects WHERE old_path = ? AND site_id = ? LIMIT 1")
        .bind(vec![Param::Text(path.to_string()), Param::Integer(site_id)])
        .first()
        .await?;
    Ok(row.map(|r| row_text(&r, "new_path")))
}

/// Match a redirect whose old path ends with the given slug segment.
pub async fn find_redirect_by_slug(
    db: &dyn Db,
    site_id: i64,
    slug: &str,
) -> Result<Option<String>, WordlessError> {
    let row: Option<Row> = db
        .prepare("SELECT new_path FROM wl_redirects WHERE old_path LIKE ? AND site_id = ? LIMIT 1")
        .bind(vec![Param::Text(format!("%/{slug}")), Param::Integer(site_id)])
        .first()
        .await?;
    Ok(row.map(|r| row_text(&r, "new_path")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structure_round_trip() {
        assert_eq!(
            PermalinkStructure::from_str("day-name"),
            Some(PermalinkStructure::DayName)
        );
        assert_eq!(PermalinkStructure::from_str("bogus"), None);
        assert!(is_permalink_structure("month-name"));
        assert_eq!(PermalinkStructure::Plain.as_str(), "plain");
        let v = serde_json::to_value(PermalinkStructure::MonthName).unwrap();
        assert_eq!(v, serde_json::json!("month-name"));
    }

    #[test]
    fn percent_decode_shapes() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("caf%C3%A9"), "café");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("100%zz"), "100%zz");
    }

    #[test]
    fn normalize_strips_slashes() {
        assert_eq!(normalize_path("///hello-world//"), "hello-world");
        assert_eq!(normalize_path("/"), "");
    }
}
