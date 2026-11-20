//! Media service — port of v1 `src/modules/media/service.ts`
//! (Milestone 3) plus the derivatives half of v1
//! `src/modules/performance/derivatives.ts` (Milestone 9).
//!
//! Media records live in `wp_posts` with `post_type = 'attachment'`
//! (the WordPress model). Object bytes live in R2 behind the kernel
//! `ObjectStore` trait. Postmeta keys:
//! - `_wp_attached_file`: the R2 object key
//! - `_wp_attachment_metadata`: JSON `{ filename, size, alt }`
//!
//! KEY SHAPE — read this before wiring routes. v1's object keys are
//! FLAT, not `media/`-prefixed: `objectKeyFor` in v1 service.ts
//! returns `${uuid8}-${safeFilename}` (e.g.
//! `a1b2c3d4-photo.jpg`), served at `/media/:key`. The architecture
//! doc §1.3 summarises R2 keys as `media/<…>`, but the v1 source is
//! flat, and the v1 file-manager tests list media files at the bucket
//! root. This port follows the v1 source (flat keys) so a v1-created
//! attachment keeps its URL. Only *derivative* keys carry a prefix:
//! `media/derivatives/<mediaId>-<width>.<format>` (v1
//! derivatives.ts). If the coordinator ever wants a `media/` prefix
//! for new uploads, it is a one-line change in [`object_key_for`];
//! the lookup is by postmeta value, so serving still works — but the
//! file-manager root listing would then show a `media` folder instead
//! of the files, and v1 URLs would change. That decision belongs to
//! the parent, not to this crate.
//!
//! DERIVATIVES — the Cloudflare Images binding is NOT available to
//! this crate (no binding type exists here, and it is not declared in
//! `wrangler.toml`). [`resolve_media_request`] therefore never fakes a
//! derivative: when Images is unbound and a variant is requested it
//! returns `MediaResolution::Unavailable`; when bound but no variant
//! is stored yet it returns `MediaResolution::NeedsDerivation` and the
//! coordinator must perform the Images call and then persist the
//! result with [`store_derivative`]. See also [`crate::optimize`].
//!
//! RANGE — v1's `GET /media/:key` serves the whole object (HTTP 200);
//! no `Range` handling exists in v1 (`grep Range src/` finds none).
//! [`parse_range_header`] / [`slice_for_range`] are a NEW, tested
//! helper the coordinator MAY use to answer `Range` with 206 for
//! video seeking; they are not v1 behaviour and must not be presented
//! as such.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use wordless_db::{
    row::{row_int, row_text},
    Db, Param, Row,
};
use wordless_kernel::{
    bad_request,
    crypto::random_bytes,
    error::{internal_error, not_found, WordlessError},
    storage::ObjectStore,
    time::now_iso,
    validate::require_string,
};

use crate::magic::verify_upload_bytes;

/// Maximum upload size — v1 constant `MAX_UPLOAD_BYTES` (service.ts).
pub const MAX_UPLOAD_BYTES: usize = 50 * 1024 * 1024; // 50 MB

/// The v1 MIME allowlist (`ALLOWED_MIME_TYPES` in service.ts).
pub const ALLOWED_MIME_TYPES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/webp",
    "image/avif",
    "image/svg+xml",
    "application/pdf",
];

pub fn is_allowed_mime_type(mime: &str) -> bool {
    ALLOWED_MIME_TYPES.contains(&mime)
}

const ATTACHED_FILE_KEY: &str = "_wp_attached_file";
const METADATA_KEY: &str = "_wp_attachment_metadata";

/// Cache-Control v1 sends for served media objects (index.ts
/// `GET /media/:key`): keys are UUID-prefixed and immutable.
pub const MEDIA_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

/// Header v1 sends when a derivative was requested but the Images
/// binding is absent: the original is served and the response says so.
pub const DERIVATIVES_UNAVAILABLE_HEADER: (&str, &str) = ("X-Wordless-Derivatives", "unavailable");

/// Derivative widths v1 pre-generates (`DERIVATIVE_WIDTHS`).
pub const DERIVATIVE_WIDTHS: &[i64] = &[320, 640, 1024, 1600];
/// Derivative formats v1 accepts (`DERIVATIVE_FORMATS`).
pub const DERIVATIVE_FORMATS: &[&str] = &["webp", "jpeg", "png"];

pub fn is_valid_derivative_width(width: i64) -> bool {
    (32..=4096).contains(&width)
}

pub fn is_valid_derivative_format(format: &str) -> bool {
    DERIVATIVE_FORMATS.contains(&format)
}

/// R2 key for a stored derivative (v1 derivatives.ts).
pub fn derivative_key(media_id: i64, width: i64, format: &str) -> String {
    format!("media/derivatives/{media_id}-{width}.{format}")
}

// ---------------------------------------------------------------- types

/// One media-library record — v1 `MediaRecord`, same JSON shape
/// (`id, title, slug, mimeType, objectKey, authorId, date, url,
/// filename, size, alt`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaRecord {
    pub id: i64,
    pub title: String,
    pub slug: String,
    pub mime_type: String,
    pub object_key: String,
    pub author_id: i64,
    pub date: String,
    pub url: String,
    pub filename: String,
    pub size: i64,
    pub alt: String,
}

impl MediaRecord {
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaListResult {
    pub media: Vec<MediaRecord>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

#[derive(Debug, Clone, Default)]
pub struct ListMediaOpts {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UploadMediaInput {
    pub title: Option<String>,
    pub filename: String,
    pub mime_type: String,
    pub data: Vec<u8>,
    pub author_id: i64,
    pub alt: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct AttachmentMeta {
    filename: String,
    size: i64,
    alt: String,
}

/// Minimal `wp_posts` row for an attachment (the fields media needs).
#[derive(Debug, Clone)]
struct PostRow {
    id: i64,
    author_id: i64,
    date: String,
    title: String,
    slug: String,
    mime_type: String,
}

fn row_to_post(row: &Row) -> PostRow {
    PostRow {
        id: row_int(row, "ID"),
        author_id: row_int(row, "post_author"),
        date: row_text(row, "post_date"),
        title: row_text(row, "post_title"),
        slug: row_text(row, "post_name"),
        mime_type: row_text(row, "post_mime_type"),
    }
}

/// One stored derivative — v1 `DerivativeRecord`, raw D1 row shape
/// (snake_case, as `listDerivatives` returns it).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DerivativeRecord {
    pub id: i64,
    pub media_id: i64,
    pub width: i64,
    pub format: String,
    pub object_key: String,
    pub size: i64,
    pub created_at: String,
}

/// What `GET /media/:key` should serve. The coordinator matches on
/// this; the crate never invents a derivative.
#[derive(Debug, Clone)]
pub enum MediaResolution {
    /// Serve the original object at this key.
    Original {
        object_key: String,
        /// True when a variant was requested but Images is unbound —
        /// the coordinator must add the `X-Wordless-Derivatives:
        /// unavailable` header (v1 index.ts).
        derivatives_unavailable: bool,
    },
    /// A stored derivative exists — serve it at this key.
    Derivative { object_key: String, format: String },
    /// Images is bound but no variant is stored yet. The coordinator
    /// must call the Images binding, then persist the bytes with
    /// [`store_derivative`], then serve the derived key.
    NeedsDerivation {
        media_id: i64,
        object_key: String,
        width: i64,
        format: String,
    },
}

/// A stored object fetched for serving, with v1's response headers.
#[derive(Debug, Clone)]
pub struct ServedMedia {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub cache_control: &'static str,
    /// `Some("unavailable")` when a variant was requested but Images
    /// is unbound (the `X-Wordless-Derivatives` header value).
    pub derivatives_header: Option<&'static str>,
}

/// An inclusive byte range parsed from a `Range: bytes=a-b` header.
/// NEW — v1 has no Range support; see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64, // inclusive
}

// ------------------------------------------------------------ validation

/// Validate an upload before any bytes are stored (v1
/// `validateUpload`). Messages are v1's, byte for byte.
pub fn validate_upload(mime_type: &str, size: usize) -> Result<(), WordlessError> {
    if !is_allowed_mime_type(mime_type) {
        return Err(bad_request(&format!(
            "File type \"{mime_type}\" is not allowed. Allowed types: JPEG, PNG, GIF, WebP, AVIF, SVG, PDF."
        )));
    }
    if size > MAX_UPLOAD_BYTES {
        return Err(bad_request(&format!(
            "File is too large ({size} bytes). Maximum upload size is {MAX_UPLOAD_BYTES} bytes."
        )));
    }
    if size == 0 {
        return Err(bad_request("Uploaded file is empty."));
    }
    Ok(())
}

/// v1 `safeFilename` (service.ts): keep the basename, replace runs of
/// disallowed characters with `-`, strip leading/trailing `-`, fall
/// back to `"file"`, cap at 180 characters.
pub fn safe_filename(filename: &str) -> String {
    let base = filename.rsplit(['/', '\\']).next().unwrap_or("file");
    let mut cleaned = String::new();
    let mut last_dash = false;
    for c in base.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
            cleaned.push(c);
            last_dash = false;
        } else if !last_dash {
            cleaned.push('-');
            last_dash = true;
        }
    }
    let cleaned = cleaned.trim_matches('-').to_string();
    if cleaned.is_empty() {
        return "file".to_string();
    }
    cleaned.chars().take(180).collect()
}

/// v1 `objectKeyFor`: 8 random hex chars + `-` + safe filename.
/// v1 takes the first 8 chars of a UUID (hex); this port takes 4
/// random bytes rendered as 8 lowercase hex chars — same shape.
pub fn object_key_for(filename: &str) -> Result<String, WordlessError> {
    let bytes =
        random_bytes(4).map_err(|e| internal_error(&format!("Random source failed: {e}")))?;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("{hex}-{}", safe_filename(filename)))
}

/// Public URL for an object key (v1 `toRecord`: `/media/` +
/// `encodeURIComponent(objectKey)`).
pub fn media_url_for_key(object_key: &str) -> String {
    let mut out = String::from("/media/");
    for b in object_key.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// v1 `slugify` (posts/service.ts): NFD-normalise, strip combining
/// marks, lowercase, non-alphanumeric runs → `-`, cap 200, empty →
/// `"post"`. Rust has no NFD normaliser in this workspace (no
/// `unicode-normalization` dep, offline), so the common Latin-1
/// accented letters are hand-mapped to their base letter and any
/// other non-ASCII letter becomes a separator. That differs from v1
/// for combining-mark sequences (e.g. `e` + U+0301): v1 yields `e`,
/// this yields a separator. Documented, not hidden.
pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for c in text.chars() {
        let mapped: Option<char> = match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => Some('a'),
            'è' | 'é' | 'ê' | 'ë' => Some('e'),
            'ì' | 'í' | 'î' | 'ï' => Some('i'),
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => Some('o'),
            'ù' | 'ú' | 'û' | 'ü' => Some('u'),
            'ý' | 'ÿ' => Some('y'),
            'ñ' => Some('n'),
            'ç' => Some('c'),
            'ß' => None, // handled below as "ss"
            'æ' => None, // handled below as "ae"
            'œ' => None, // handled below as "oe"
            _ => None,
        };
        if c == 'ß' {
            out.push_str("ss");
            last_dash = false;
            continue;
        }
        if c == 'æ' {
            out.push_str("ae");
            last_dash = false;
            continue;
        }
        if c == 'œ' {
            out.push_str("oe");
            last_dash = false;
            continue;
        }
        let base = mapped.unwrap_or(c);
        if base.is_ascii_alphanumeric() {
            out.push(base.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let slug = out.trim_matches('-').to_string();
    let slug: String = slug.chars().take(200).collect();
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() {
        "post".to_string()
    } else {
        slug
    }
}

/// v1 `parseIdParam` for media ids: `Invalid media id.`
pub fn parse_media_id(raw: &str) -> Result<i64, WordlessError> {
    match raw.trim().parse::<i64>() {
        Ok(n) if n >= 1 => Ok(n),
        _ => Err(bad_request("Invalid media id.")),
    }
}

// ------------------------------------------------------------ post rows

async fn default_comment_status(db: &dyn Db, site_id: i64) -> Result<String, WordlessError> {
    // v1 settings/service.ts getDefaultCommentStatus: option
    // `default_comment_status`, default "open"; only "closed" wins.
    // P12: a per-site option.
    let raw: String = wordless_multisite::get_site_option(
        db,
        site_id,
        "default_comment_status",
        "open".to_string(),
    )
    .await?;
    Ok(if raw == "closed" {
        "closed".to_string()
    } else {
        "open".to_string()
    })
}

/// Create the `wp_posts` attachment row (v1 `createPost` with
/// `type: "attachment"`, `status: "publish"`). Slug uniqueness is
/// checked exactly as v1 does it: `findPostBySlug` only matches
/// `post_type IN ('post', 'page')`, so attachments never collide
/// with each other.
async fn create_attachment_post(
    db: &dyn Db,
    site_id: i64,
    title: &str,
    author_id: i64,
    mime_type: &str,
) -> Result<PostRow, WordlessError> {
    let title = require_string(&json!(title), "title", 1, 1000)?;
    let base_slug = slugify(&title);
    let mut candidate = base_slug.clone();
    let mut n = 2;
    loop {
        let existing = db.query_one("SELECT * FROM wp_posts WHERE post_name = ? AND post_type IN ('post', 'page') AND site_id = ? LIMIT 1",
            &[Param::Text(candidate.clone()), Param::Integer(site_id)],
        )
        .await?;
        if existing.is_none() {
            break;
        }
        candidate = format!("{base_slug}-{n}");
        n += 1;
    }
    let now = now_iso();
    let comment_status = default_comment_status(db, site_id).await?;
    // Same 14 columns as v1 posts/repo.ts insertPostRow.
    let meta = db.exec("INSERT INTO wp_posts (site_id, post_author, post_date, post_date_gmt, post_content, post_title, post_excerpt, post_status, post_name, post_modified, post_modified_gmt, post_parent, post_type, post_mime_type, comment_status) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        &[
            Param::Integer(site_id),
            Param::Integer(author_id),
            Param::Text(now.clone()),
            Param::Text(now.clone()),
            Param::Text(String::new()),
            Param::Text(title.clone()),
            Param::Text(String::new()),
            Param::Text("publish".to_string()),
            Param::Text(candidate),
            Param::Text(now.clone()),
            Param::Text(now.clone()),
            Param::Integer(0),
            Param::Text("attachment".to_string()),
            Param::Text(mime_type.to_string()),
            Param::Text(comment_status),
        ],
    )
    .await?;
    let Some(id) = meta.last_row_id else {
        return Err(internal_error("Failed to read back the created post."));
    };
    let row = db
        .query_one(
            "SELECT * FROM wp_posts WHERE ID = ? AND site_id = ? LIMIT 1",
            &[Param::Integer(id), Param::Integer(site_id)],
        )
        .await?;
    match row {
        Some(row) => Ok(row_to_post(&row)),
        None => Err(internal_error("Failed to read back the created post.")),
    }
}

/// Permanently delete a post and its postmeta (v1
/// `deletePost`/`deletePostRow`): meta first, then child revisions,
/// then the row itself.
async fn delete_post_row(db: &dyn Db, site_id: i64, id: i64) -> Result<u64, WordlessError> {
    db.exec(
        "DELETE FROM wp_postmeta WHERE post_id = ? AND site_id = ?",
        &[Param::Integer(id), Param::Integer(site_id)],
    )
    .await?;
    db.exec(
        "DELETE FROM wp_posts WHERE post_parent = ? AND post_type = 'revision' AND site_id = ?",
        &[Param::Integer(id), Param::Integer(site_id)],
    )
    .await?;
    let meta = db
        .exec("DELETE FROM wp_posts WHERE ID = ? AND site_id = ?", &[Param::Integer(id), Param::Integer(site_id)])
        .await?;
    Ok(meta.changes)
}

// ----------------------------------------------------------------- meta

fn parse_meta(raw: &Value) -> AttachmentMeta {
    let fallback = AttachmentMeta::default();
    let Some(s) = raw.as_str() else {
        return fallback;
    };
    let Ok(parsed) = serde_json::from_str::<Value>(s) else {
        return fallback;
    };
    AttachmentMeta {
        filename: parsed
            .get("filename")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        size: parsed.get("size").and_then(|v| v.as_i64()).unwrap_or(0),
        alt: parsed
            .get("alt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    }
}

async fn read_meta(db: &dyn Db, site_id: i64, post_id: i64) -> Result<(String, AttachmentMeta), WordlessError> {
    let rows = db
        .query_all(
            "SELECT meta_key, meta_value FROM wp_postmeta WHERE post_id = ? AND meta_key IN (?, ?) AND site_id = ?",
            &[
                Param::Integer(post_id),
                Param::Text(ATTACHED_FILE_KEY.to_string()),
                Param::Text(METADATA_KEY.to_string()),
                Param::Integer(site_id),
            ],
        )
        .await?;
    let mut object_key = String::new();
    let mut meta = AttachmentMeta::default();
    for row in &rows {
        match row_text(row, "meta_key").as_str() {
            ATTACHED_FILE_KEY => object_key = row_text(row, "meta_value"),
            METADATA_KEY => meta = parse_meta(&row["meta_value"]),
            _ => {}
        }
    }
    Ok((object_key, meta))
}

async fn write_meta(
    db: &dyn Db,
    site_id: i64,
    post_id: i64,
    object_key: &str,
    meta: &AttachmentMeta,
) -> Result<(), WordlessError> {
    db.exec(
        "INSERT INTO wp_postmeta (site_id, post_id, meta_key, meta_value) VALUES (?, ?, ?, ?)",
        &[
            Param::Integer(site_id),
            Param::Integer(post_id),
            Param::Text(ATTACHED_FILE_KEY.to_string()),
            Param::Text(object_key.to_string()),
        ],
    )
    .await?;
    db.exec(
        "INSERT INTO wp_postmeta (site_id, post_id, meta_key, meta_value) VALUES (?, ?, ?, ?)",
        &[
            Param::Integer(site_id),
            Param::Integer(post_id),
            Param::Text(METADATA_KEY.to_string()),
            Param::Text(
                json!({ "filename": meta.filename, "size": meta.size, "alt": meta.alt })
                    .to_string(),
            ),
        ],
    )
    .await?;
    Ok(())
}

fn to_record(post: &PostRow, object_key: &str, meta: &AttachmentMeta) -> MediaRecord {
    MediaRecord {
        id: post.id,
        title: post.title.clone(),
        slug: post.slug.clone(),
        mime_type: post.mime_type.clone(),
        object_key: object_key.to_string(),
        author_id: post.author_id,
        date: post.date.clone(),
        url: if object_key.is_empty() {
            String::new()
        } else {
            media_url_for_key(object_key)
        },
        filename: meta.filename.clone(),
        size: meta.size,
        alt: meta.alt.clone(),
    }
}

// --------------------------------------------------------------- service

/// Store bytes in R2 and create the attachment record (v1
/// `uploadMedia`). Order is v1's: validate → magic-byte check →
/// `put` → create post → write meta. (As in v1, a post-creation
/// failure after the `put` leaves an orphaned object; the coordinator
/// owns retry/cleanup, exactly as v1 does.)
pub async fn upload_media(
    db: &dyn Db,
    site_id: i64,
    store: &dyn ObjectStore,
    input: UploadMediaInput,
) -> Result<MediaRecord, WordlessError> {
    validate_upload(&input.mime_type, input.data.len())?;
    let filename = safe_filename(&input.filename);
    // Milestone 9: magic-byte sniffing — a PHP script renamed to
    // .jpg is rejected here, before any bytes reach R2.
    verify_upload_bytes(&filename, &input.mime_type, &input.data)?;
    let key = object_key_for(&filename)?;
    let title = input
        .title
        .as_ref()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| filename.clone());
    store
        .put_object(&key, input.data.clone(), &input.mime_type)
        .await?;
    let post = create_attachment_post(db, site_id, &title, input.author_id, &input.mime_type).await?;
    let meta = AttachmentMeta {
        filename: filename.clone(),
        size: input.data.len() as i64,
        alt: input
            .alt
            .as_ref()
            .map(|a| a.trim().to_string())
            .unwrap_or_default(),
    };
    write_meta(db, site_id, post.id, &key, &meta).await?;
    Ok(to_record(&post, &key, &meta))
}

pub async fn get_media_record(db: &dyn Db, site_id: i64, id: i64) -> Result<Option<MediaRecord>, WordlessError> {
    let row = db
        .query_one(
            "SELECT * FROM wp_posts WHERE ID = ? AND site_id = ? LIMIT 1",
            &[Param::Integer(id), Param::Integer(site_id)],
        )
        .await?;
    let Some(row) = row else { return Ok(None) };
    let post_type = row_text(&row, "post_type");
    if post_type != "attachment" {
        return Ok(None);
    }
    let post = row_to_post(&row);
    let (object_key, meta) = read_meta(db, site_id, id).await?;
    Ok(Some(to_record(&post, &object_key, &meta)))
}

/// Find an attachment by its R2 object key (powers `GET /media/:key`).
pub async fn find_media_by_key(
    db: &dyn Db,
    site_id: i64,
    object_key: &str,
) -> Result<Option<MediaRecord>, WordlessError> {
    let row = db
        .query_one(
            "SELECT post_id FROM wp_postmeta WHERE meta_key = ? AND meta_value = ? AND site_id = ? LIMIT 1",
            &[
                Param::Text(ATTACHED_FILE_KEY.to_string()),
                Param::Text(object_key.to_string()),
                Param::Integer(site_id),
            ],
        )
        .await?;
    let Some(row) = row else { return Ok(None) };
    let post_id = row_int(&row, "post_id");
    if post_id <= 0 {
        return Ok(None);
    }
    get_media_record(db, site_id, post_id).await
}

/// Move an attachment's stored object key (file-manager rename).
/// Updates `_wp_attached_file` and the metadata filename. Returns
/// the record, or `None` when the key is not an attachment.
pub async fn rename_media_object(
    db: &dyn Db,
    site_id: i64,
    old_key: &str,
    new_key: &str,
) -> Result<Option<MediaRecord>, WordlessError> {
    let Some(record) = find_media_by_key(db, site_id, old_key).await? else {
        return Ok(None);
    };
    let (_, meta) = read_meta(db, site_id, record.id).await?;
    db.exec(
        "UPDATE wp_postmeta SET meta_value = ? WHERE post_id = ? AND meta_key = ? AND site_id = ?",
        &[
            Param::Text(new_key.to_string()),
            Param::Integer(record.id),
            Param::Text(ATTACHED_FILE_KEY.to_string()),
            Param::Integer(site_id),
        ],
    )
    .await?;
    let updated_meta = AttachmentMeta {
        filename: new_key
            .rsplit('/')
            .next()
            .unwrap_or(&meta.filename)
            .to_string(),
        size: meta.size,
        alt: meta.alt,
    };
    db.exec("UPDATE wp_postmeta SET meta_value = ? WHERE post_id = ? AND meta_key = ? AND site_id = ?",
        &[
            Param::Text(
                json!({ "filename": updated_meta.filename, "size": updated_meta.size, "alt": updated_meta.alt })
                    .to_string(),
            ),
            Param::Integer(record.id),
            Param::Text(METADATA_KEY.to_string()),
            Param::Integer(site_id),
        ],
    )
    .await?;
    get_media_record(db, site_id, record.id).await
}

pub async fn list_media(
    db: &dyn Db,
    site_id: i64,
    opts: ListMediaOpts,
) -> Result<MediaListResult, WordlessError> {
    let page = match opts.page {
        Some(p) if p >= 1 => p,
        _ => 1,
    };
    let per_page = match opts.per_page {
        Some(p) => p.clamp(1, 100),
        None => 20,
    };
    let mut where_sql = "WHERE site_id = ? AND post_type = ? AND post_status != ?".to_string();
    let mut params: Vec<Param> = vec![
        Param::Integer(site_id),
        Param::Text("attachment".to_string()),
        Param::Text("trash".to_string()),
    ];
    if let Some(mime) = &opts.mime_type {
        where_sql.push_str(" AND post_mime_type = ?");
        params.push(Param::Text(mime.clone()));
    }
    let total_row = db
        .query_one(
            &format!("SELECT COUNT(*) AS n FROM wp_posts {where_sql}"),
            &params,
        )
        .await?;
    let total = total_row.as_ref().map(|r| row_int(r, "n")).unwrap_or(0);
    let mut list_params = params.clone();
    list_params.push(Param::Integer(per_page));
    list_params.push(Param::Integer((page - 1) * per_page));
    let rows = db.query_all(&format!("SELECT * FROM wp_posts {where_sql} ORDER BY post_date DESC, ID DESC LIMIT ? OFFSET ?"),
        &list_params,
    )
    .await?;
    let mut media = Vec::new();
    for row in &rows {
        let post = row_to_post(row);
        let (object_key, meta) = read_meta(db, site_id, post.id).await?;
        media.push(to_record(&post, &object_key, &meta));
    }
    Ok(MediaListResult {
        media,
        total,
        page,
        per_page,
        total_pages: if total == 0 {
            0
        } else {
            (total + per_page - 1) / per_page
        },
    })
}

/// Update alt text, stored in `_wp_attachment_metadata` (v1
/// `updateMediaAlt`). Alt is trimmed and capped at 500 characters.
pub async fn update_media_alt(
    db: &dyn Db,
    site_id: i64,
    id: i64,
    alt: &str,
) -> Result<MediaRecord, WordlessError> {
    let Some(record) = get_media_record(db, site_id, id).await? else {
        return Err(not_found(&format!("Media item {id} not found.")));
    };
    let clean_alt: String = alt.trim().chars().take(500).collect();
    let meta = AttachmentMeta {
        filename: record.filename.clone(),
        size: record.size,
        alt: clean_alt.clone(),
    };
    db.exec(
        "UPDATE wp_postmeta SET meta_value = ? WHERE post_id = ? AND meta_key = ? AND site_id = ?",
        &[
            Param::Text(
                json!({ "filename": meta.filename, "size": meta.size, "alt": meta.alt })
                    .to_string(),
            ),
            Param::Integer(id),
            Param::Text(METADATA_KEY.to_string()),
            Param::Integer(site_id),
        ],
    )
    .await?;
    Ok(MediaRecord {
        alt: clean_alt,
        ..record
    })
}

/// Delete the R2 object, the attachment post and its meta (v1
/// `deleteMedia`). Returns `true` on success.
pub async fn delete_media(
    db: &dyn Db,
    site_id: i64,
    store: &dyn ObjectStore,
    id: i64,
) -> Result<bool, WordlessError> {
    let Some(record) = get_media_record(db, site_id, id).await? else {
        return Err(not_found(&format!("Media item {id} not found.")));
    };
    if !record.object_key.is_empty() {
        store.delete_object(&record.object_key).await?;
    }
    Ok(delete_post_row(db, site_id, id).await? > 0)
}

// ------------------------------------------------------------ derivatives

/// `GET /media/:key?width=&format=` — variant only when Images is
/// bound (v1 `getDerivativeOrOriginal`). Defaults when only one of
/// width/format is given: width 1024, format `webp`. Width and
/// format are validated even when unbound, exactly as in v1.
pub async fn resolve_media_request(
    db: &dyn Db,
    record: &MediaRecord,
    width: Option<i64>,
    format: Option<&str>,
    images_bound: bool,
) -> Result<MediaResolution, WordlessError> {
    let (width, format) = match (width, format) {
        (None, None) => {
            return Ok(MediaResolution::Original {
                object_key: record.object_key.clone(),
                derivatives_unavailable: false,
            });
        }
        (w, f) => (w.unwrap_or(1024), f.unwrap_or("webp")),
    };
    if !is_valid_derivative_width(width) {
        return Err(bad_request("width must be an integer between 32 and 4096."));
    }
    if !is_valid_derivative_format(format) {
        return Err(bad_request("format must be webp, jpeg or png."));
    }
    if !images_bound {
        return Ok(MediaResolution::Original {
            object_key: record.object_key.clone(),
            derivatives_unavailable: true,
        });
    }
    let existing = db.query_one("SELECT * FROM wl_media_derivatives WHERE media_id = ? AND width = ? AND format = ? LIMIT 1",
        &[
            Param::Integer(record.id),
            Param::Integer(width),
            Param::Text(format.to_string()),
        ],
    )
    .await?;
    if let Some(row) = existing {
        return Ok(MediaResolution::Derivative {
            object_key: row_text(&row, "object_key"),
            format: format.to_string(),
        });
    }
    Ok(MediaResolution::NeedsDerivation {
        media_id: record.id,
        object_key: record.object_key.clone(),
        width,
        format: format.to_string(),
    })
}

/// Persist a derived variant produced by the Images binding (the
/// coordinator's half of v1 `getDerivativeOrOriginal`): store the
/// bytes in R2 under `media/derivatives/<id>-<w>.<fmt>` and record
/// the row in `wl_media_derivatives`.
pub async fn store_derivative(
    db: &dyn Db,
    store: &dyn ObjectStore,
    media_id: i64,
    width: i64,
    format: &str,
    bytes: Vec<u8>,
) -> Result<DerivativeRecord, WordlessError> {
    if !is_valid_derivative_width(width) {
        return Err(bad_request("width must be an integer between 32 and 4096."));
    }
    if !is_valid_derivative_format(format) {
        return Err(bad_request("format must be webp, jpeg or png."));
    }
    let key = derivative_key(media_id, width, format);
    let content_type = format!("image/{format}");
    store.put_object(&key, bytes.clone(), &content_type).await?;
    let now = now_iso();
    let meta = db.exec("INSERT INTO wl_media_derivatives (media_id, width, format, object_key, size, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        &[
            Param::Integer(media_id),
            Param::Integer(width),
            Param::Text(format.to_string()),
            Param::Text(key.clone()),
            Param::Integer(bytes.len() as i64),
            Param::Text(now.clone()),
        ],
    )
    .await?;
    Ok(DerivativeRecord {
        id: meta.last_row_id.unwrap_or(0),
        media_id,
        width,
        format: format.to_string(),
        object_key: key,
        size: bytes.len() as i64,
        created_at: now,
    })
}

/// Stored derivatives for one media item, for srcset generation (v1
/// `listDerivatives`).
pub async fn list_derivatives(
    db: &dyn Db,
    media_id: i64,
) -> Result<Vec<DerivativeRecord>, WordlessError> {
    let rows = db
        .query_all(
            "SELECT * FROM wl_media_derivatives WHERE media_id = ? ORDER BY width ASC",
            &[Param::Integer(media_id)],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|row| DerivativeRecord {
            id: row_int(row, "id"),
            media_id: row_int(row, "media_id"),
            width: row_int(row, "width"),
            format: row_text(row, "format"),
            object_key: row_text(row, "object_key"),
            size: row_int(row, "size"),
            created_at: row_text(row, "created_at"),
        })
        .collect())
}

/// Build a srcset string from stored derivative records (v1
/// `buildSrcset`). Empty when none exist.
pub fn build_srcset(base_media_url: &str, derivatives: &[DerivativeRecord]) -> String {
    derivatives
        .iter()
        .map(|d| {
            format!(
                "{base_media_url}?width={}&format={} {}w",
                d.width, d.format, d.width
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Derivative status for Site Health / the media page (v1
/// `derivativeStatus`): measured, never claimed.
pub async fn derivative_status(db: &dyn Db, images_bound: bool) -> Result<Value, WordlessError> {
    let row = db
        .query_one("SELECT COUNT(*) AS n FROM wl_media_derivatives", &[])
        .await?;
    let stored = row.as_ref().map(|r| row_int(r, "n")).unwrap_or(0);
    Ok(json!({ "bound": images_bound, "stored": stored, "widths": DERIVATIVE_WIDTHS }))
}

// ---------------------------------------------------------------- serving

/// Fetch a stored object for serving (v1 `GET /media/:key` body).
/// `content_type` is the record's MIME for originals, or
/// `image/<format>` for derivatives. Returns 404 when the object is
/// gone from R2 even though the record exists.
pub async fn serve_stored_object(
    store: &dyn ObjectStore,
    object_key: &str,
    content_type: &str,
    derivatives_unavailable: bool,
) -> Result<ServedMedia, WordlessError> {
    let Some((bytes, _)) = store.get_object(object_key).await? else {
        return Err(not_found("Media file not found."));
    };
    Ok(ServedMedia {
        bytes,
        content_type: content_type.to_string(),
        cache_control: MEDIA_CACHE_CONTROL,
        derivatives_header: if derivatives_unavailable {
            Some("unavailable")
        } else {
            None
        },
    })
}

/// Parse a `Range: bytes=a-b` header against a total length. NEW —
/// v1 has no Range support (it always serves the whole object with
/// 200). Returns `None` for absent, malformed, or unsatisfiable
/// ranges (the coordinator answers those with 200 / 416).
pub fn parse_range_header(header: Option<&str>, total_len: u64) -> Option<ByteRange> {
    let header = header?.trim();
    let rest = header.strip_prefix("bytes=")?;
    let (start_s, end_s) = rest.split_once('-')?;
    if total_len == 0 {
        return None;
    }
    if start_s.is_empty() {
        // Suffix range: bytes=-N → the last N bytes.
        let suffix: u64 = end_s.parse().ok()?;
        if suffix == 0 {
            return None;
        }
        let start = total_len.saturating_sub(suffix);
        return Some(ByteRange {
            start,
            end: total_len - 1,
        });
    }
    let start: u64 = start_s.parse().ok()?;
    if start >= total_len {
        return None;
    }
    let end = if end_s.is_empty() {
        total_len - 1
    } else {
        let end: u64 = end_s.parse().ok()?;
        if end < start {
            return None;
        }
        end.min(total_len - 1)
    };
    Some(ByteRange { start, end })
}

/// Slice bytes for a parsed range (inclusive end).
pub fn slice_for_range(bytes: &[u8], range: ByteRange) -> &[u8] {
    let start = range.start as usize;
    let end = (range.end as usize + 1).min(bytes.len());
    if start >= end {
        return &[];
    }
    &bytes[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{block_on, FakeDb, FakeStore};

    const PNG: &[u8] = &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3];
    const PDF: &[u8] = &[0x25, 0x50, 0x44, 0x46, 0x2d, 0x31, 0x2e, 0x34];

    fn upload_input(filename: &str, mime: &str, data: &[u8]) -> UploadMediaInput {
        UploadMediaInput {
            title: None,
            filename: filename.to_string(),
            mime_type: mime.to_string(),
            data: data.to_vec(),
            author_id: 1,
            alt: None,
        }
    }

    #[test]
    fn validate_upload_matches_v1_messages() {
        for mime in ALLOWED_MIME_TYPES {
            assert!(validate_upload(mime, 100).is_ok());
        }
        let err = validate_upload("text/plain", 100).unwrap_err();
        assert!(err.message.contains("not allowed"));
        assert_eq!(err.status, 400);
        let err = validate_upload("application/x-php", 100).unwrap_err();
        assert!(err.message.contains("not allowed"));
        let err = validate_upload("image/png", 50 * 1024 * 1024 + 1).unwrap_err();
        assert!(err.message.contains("too large"));
        let err = validate_upload("image/png", 0).unwrap_err();
        assert!(err.message.contains("empty"));
    }

    #[test]
    fn safe_filename_and_slugify_follow_v1() {
        assert_eq!(safe_filename("My Photo.PNG"), "My-Photo.PNG");
        assert_eq!(safe_filename("/etc/passwd"), "passwd");
        assert_eq!(safe_filename("a/b\\c.png"), "c.png");
        assert_eq!(safe_filename("!!!"), "file");
        assert_eq!(safe_filename("a  b.png"), "a-b.png");
        assert_eq!(slugify("Hello, World!"), "hello-world");
        assert_eq!(slugify("Café & Bar"), "cafe-bar");
        assert_eq!(slugify(""), "post");
        assert_eq!(slugify("!!!"), "post");
        // Object keys are 8 hex chars + "-" + safe filename.
        let key = object_key_for("My Photo.PNG").unwrap();
        assert_eq!(key.len(), 8 + 1 + "My-Photo.PNG".len());
        assert!(key.ends_with("-My-Photo.PNG"));
        assert!(key[..8].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn upload_list_get_update_delete_round_trip() {
        block_on(async {
            let db = FakeDb::new();
            let store = FakeStore::new();
            let record = upload_media(
                &db, 1,
                &store,
                UploadMediaInput {
                    alt: Some("A test photo".to_string()),
                    ..upload_input("My Photo.PNG", "image/png", PNG)
                },
            )
            .await
            .unwrap();
            assert!(record.id > 0);
            assert!(record.url.starts_with("/media/"));
            assert!(record.url.ends_with("-My-Photo.PNG"));
            assert_eq!(record.size, PNG.len() as i64);
            assert_eq!(record.alt, "A test photo");
            assert_eq!(record.filename, "My-Photo.PNG");
            assert!(store.contains(&record.object_key));
            assert!(!record.object_key.contains('/'), "v1 keys are flat");

            let read_back = get_media_record(&db, 1, record.id).await.unwrap().unwrap();
            assert_eq!(read_back.object_key, record.object_key);
            assert_eq!(read_back.alt, "A test photo");

            let by_key = find_media_by_key(&db, 1, &record.object_key)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(by_key.id, record.id);
            assert!(find_media_by_key(&db, 1, "no-such-key")
                .await
                .unwrap()
                .is_none());
            assert!(get_media_record(&db, 1, 9999).await.unwrap().is_none());

            let listed = list_media(&db, 1, ListMediaOpts::default()).await.unwrap();
            assert_eq!(listed.total, 1);
            assert_eq!(listed.media[0].id, record.id);
            assert_eq!(listed.page, 1);
            assert_eq!(listed.per_page, 20);
            assert_eq!(listed.total_pages, 1);

            let updated = update_media_alt(&db, 1, record.id, "  New alt text  ")
                .await
                .unwrap();
            assert_eq!(updated.alt, "New alt text");
            let err = update_media_alt(&db, 1, 9999, "x").await.unwrap_err();
            assert_eq!(err.status, 404);
            assert!(err.message.contains("Media item 9999 not found."));

            let deleted = delete_media(&db, 1, &store, record.id).await.unwrap();
            assert!(deleted);
            assert!(!store.contains(&record.object_key));
            assert!(get_media_record(&db, 1, record.id).await.unwrap().is_none());
            assert_eq!(
                list_media(&db, 1, ListMediaOpts::default())
                    .await
                    .unwrap()
                    .total,
                0
            );
            assert_eq!(
                delete_media(&db, 1, &store, record.id)
                    .await
                    .unwrap_err()
                    .status,
                404
            );
        });
    }

    #[test]
    fn upload_rejects_disallowed_without_storing() {
        block_on(async {
            let db = FakeDb::new();
            let store = FakeStore::new();
            let err = upload_media(
                &db, 1,
                &store,
                upload_input("evil.php", "application/x-php", b"<?php"),
            )
            .await
            .unwrap_err();
            assert!(err.message.contains("not allowed"));
            assert_eq!(store.len(), 0);
            assert_eq!(
                list_media(&db, 1, ListMediaOpts::default())
                    .await
                    .unwrap()
                    .total,
                0
            );
            // Magic-byte mismatch: PNG bytes labelled PDF.
            let err = upload_media(&db, 1, &store, upload_input("b.pdf", "application/pdf", PNG))
                .await
                .unwrap_err();
            assert!(err.message.contains("do not match"));
            assert_eq!(store.len(), 0);
        });
    }

    #[test]
    fn list_filters_by_mime_and_paginates() {
        block_on(async {
            let db = FakeDb::new();
            let store = FakeStore::new();
            upload_media(&db, 1, &store, upload_input("a.png", "image/png", PNG))
                .await
                .unwrap();
            upload_media(&db, 1, &store, upload_input("b.pdf", "application/pdf", PDF))
                .await
                .unwrap();
            assert_eq!(
                list_media(&db, 1, ListMediaOpts::default())
                    .await
                    .unwrap()
                    .total,
                2
            );
            let png_only = list_media(
                &db, 1,
                ListMediaOpts {
                    mime_type: Some("image/png".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            assert_eq!(png_only.total, 1);
            assert_eq!(png_only.media[0].filename, "a.png");
            // Newest first (post_date DESC, ID DESC): b.pdf first.
            let all = list_media(&db, 1, ListMediaOpts::default()).await.unwrap();
            assert_eq!(all.media[0].filename, "b.pdf");
            let page2 = list_media(
                &db, 1,
                ListMediaOpts {
                    page: Some(2),
                    per_page: Some(1),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            assert_eq!(page2.total, 2);
            assert_eq!(page2.total_pages, 2);
            assert_eq!(page2.media[0].filename, "a.png");
            // per_page clamps to 1..100.
            let clamped = list_media(
                &db, 1,
                ListMediaOpts {
                    per_page: Some(500),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            assert_eq!(clamped.per_page, 100);
        });
    }

    #[test]
    fn rename_media_object_updates_meta() {
        block_on(async {
            let db = FakeDb::new();
            let store = FakeStore::new();
            let record = upload_media(&db, 1, &store, upload_input("photo.png", "image/png", PNG))
                .await
                .unwrap();
            assert!(rename_media_object(&db, 1, "missing-key", "new.png")
                .await
                .unwrap()
                .is_none());
            let renamed = rename_media_object(&db, 1, &record.object_key, "renamed.png")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(renamed.filename, "renamed.png");
            assert_eq!(renamed.object_key, "renamed.png");
            assert!(find_media_by_key(&db, 1, &record.object_key)
                .await
                .unwrap()
                .is_none());
        });
    }

    #[test]
    fn derivatives_unbound_serve_original_and_say_so() {
        block_on(async {
            let db = FakeDb::new();
            let store = FakeStore::new();
            let record = upload_media(&db, 1, &store, upload_input("photo.png", "image/png", PNG))
                .await
                .unwrap();
            // No variant requested → original, no header.
            let r = resolve_media_request(&db, &record, None, None, false)
                .await
                .unwrap();
            assert!(matches!(
                r,
                MediaResolution::Original {
                    derivatives_unavailable: false,
                    ..
                }
            ));
            // Variant requested, Images unbound → original + unavailable flag.
            let r = resolve_media_request(&db, &record, Some(640), Some("webp"), false)
                .await
                .unwrap();
            assert!(matches!(
                r,
                MediaResolution::Original {
                    derivatives_unavailable: true,
                    ..
                }
            ));
            // Bad width / format are rejected even when unbound (v1).
            assert!(
                resolve_media_request(&db, &record, Some(10), Some("webp"), false)
                    .await
                    .is_err()
            );
            assert!(
                resolve_media_request(&db, &record, Some(640), Some("gif"), false)
                    .await
                    .is_err()
            );
            // Defaults: width-only → 1024, format-only → webp.
            let r = resolve_media_request(&db, &record, Some(640), None, false)
                .await
                .unwrap();
            assert!(matches!(
                r,
                MediaResolution::Original {
                    derivatives_unavailable: true,
                    ..
                }
            ));
            // Bound but nothing stored → NeedsDerivation; store it; next
            // call returns the stored derivative.
            let r = resolve_media_request(&db, &record, Some(640), Some("webp"), true)
                .await
                .unwrap();
            assert!(matches!(
                r,
                MediaResolution::NeedsDerivation { width: 640, .. }
            ));
            let derived_bytes = b"fake-webp-bytes".to_vec();
            let stored =
                store_derivative(&db, &store, record.id, 640, "webp", derived_bytes.clone())
                    .await
                    .unwrap();
            assert_eq!(stored.object_key, "media/derivatives/1-640.webp");
            assert!(store.contains("media/derivatives/1-640.webp"));
            let r = resolve_media_request(&db, &record, Some(640), Some("webp"), true)
                .await
                .unwrap();
            assert!(matches!(r, MediaResolution::Derivative { .. }));
            assert_eq!(
                build_srcset(
                    "/media/x.png",
                    &list_derivatives(&db, record.id).await.unwrap()
                ),
                "/media/x.png?width=640&format=webp 640w"
            );
            let status = derivative_status(&db, false).await.unwrap();
            assert_eq!(
                status,
                json!({ "bound": false, "stored": 1, "widths": [320, 640, 1024, 1600] })
            );
        });
    }

    #[test]
    fn serve_stored_object_uses_v1_headers_and_404s_when_gone() {
        block_on(async {
            let db = FakeDb::new();
            let store = FakeStore::new();
            let record = upload_media(&db, 1, &store, upload_input("photo.png", "image/png", PNG))
                .await
                .unwrap();
            let served = serve_stored_object(&store, &record.object_key, "image/png", false)
                .await
                .unwrap();
            assert_eq!(served.bytes, PNG);
            assert_eq!(served.content_type, "image/png");
            assert_eq!(served.cache_control, "public, max-age=31536000, immutable");
            assert_eq!(served.derivatives_header, None);
            let served = serve_stored_object(&store, &record.object_key, "image/png", true)
                .await
                .unwrap();
            assert_eq!(served.derivatives_header, Some("unavailable"));
            assert_eq!(
                serve_stored_object(&store, "gone-key", "image/png", false)
                    .await
                    .unwrap_err()
                    .status,
                404
            );
        });
    }

    #[test]
    fn range_parsing_is_new_and_tested() {
        let total = 10u64;
        assert_eq!(
            parse_range_header(Some("bytes=0-4"), total),
            Some(ByteRange { start: 0, end: 4 })
        );
        assert_eq!(
            parse_range_header(Some("bytes=5-"), total),
            Some(ByteRange { start: 5, end: 9 })
        );
        assert_eq!(
            parse_range_header(Some("bytes=-3"), total),
            Some(ByteRange { start: 7, end: 9 })
        );
        assert_eq!(
            parse_range_header(Some("bytes=0-99"), total),
            Some(ByteRange { start: 0, end: 9 })
        );
        assert_eq!(parse_range_header(Some("bytes=10-20"), total), None);
        assert_eq!(parse_range_header(Some("bytes=5-2"), total), None);
        assert_eq!(parse_range_header(Some("items=0-4"), total), None);
        assert_eq!(parse_range_header(None, total), None);
        let bytes: Vec<u8> = (0..10).collect();
        assert_eq!(
            slice_for_range(&bytes, ByteRange { start: 2, end: 4 }),
            &[2, 3, 4]
        );
    }
}
