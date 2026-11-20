//! File manager — port of v1 `src/modules/files/service.ts`
//! (Milestone 11).
//!
//! Lists the objects in the R2 bucket with folder navigation by key
//! prefix, and lets an administrator upload, download, rename and
//! delete files. Every operation needs `manage_options` (enforced by
//! the routes, not here).
//!
//! Safety rules, all ported from v1 and tested:
//! - No shell access, no in-place editing of files. PHP files are
//!   view-only through download; they are never executed.
//! - Path traversal is rejected: keys and prefixes must be relative,
//!   with no `..`, no leading `/`, no backslashes, no empty segments.
//! - Managed prefixes are read-only here: `extensions/`, `themes/`,
//!   `backups/` and `imports/` are owned by the Marketplace, Themes,
//!   Backup and Import flows. Upload/rename/delete under them is
//!   refused with a message pointing at the owning page, so a rename
//!   cannot silently break an installed extension or orphan a backup.
//!   Media files (root keys) are writable; deleting one also deletes
//!   its media-library record, renaming one updates its record.
//! - Uploads use the same MIME allowlist and 50 MB cap as the media
//!   library, plus the same magic-byte sniffing. A file-manager upload
//!   is a plain R2 object, not a media-library attachment.
//! - Listings are capped at 1000 entries per folder; `truncated`
//!   says so honestly.
//!
//! One adaptation from v1: v1's `R2MediaStorage.delete` returns
//! `false` for a missing key (it `head`s first), while the kernel
//! `ObjectStore::delete_object` is a no-op success for missing keys.
//! [`delete_file`] therefore checks existence with `head_object`
//! first, so a missing file still 404s with v1's message.

use serde::{Deserialize, Serialize};
use wordless_db::{Db, Param};
use wordless_kernel::{
    bad_request,
    error::{not_found, WordlessError},
    storage::ObjectStore,
    time::now_iso,
};

use crate::{
    magic::verify_upload_bytes,
    service::{delete_media, find_media_by_key, rename_media_object, validate_upload},
};

/// Prefixes owned by other flows — read-only in the file manager
/// (v1 `MANAGED_PREFIXES`).
pub const MANAGED_PREFIXES: &[&str] = &["extensions/", "themes/", "backups/", "imports/"];

/// Maximum entries in one folder listing (v1 `MAX_FILE_LIST_ENTRIES`).
pub const MAX_FILE_LIST_ENTRIES: usize = 1000;

/// One page of a folder listing (v1 `FileEntry` / `FileListResult`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub key: String,
    pub name: String,
    pub size: i64,
    /// Lowercase extension without the dot, or `"file"`.
    #[serde(rename = "type")]
    pub file_type: String,
    pub modified: Option<String>,
    /// True when the key belongs to a media-library attachment.
    pub is_media: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileListResult {
    pub prefix: String,
    pub folders: Vec<String>,
    pub files: Vec<FileEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct UploadFileInput {
    pub filename: String,
    pub mime_type: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteFileResult {
    pub deleted: bool,
}

#[derive(Debug, Clone)]
pub struct DownloadedFile {
    pub data: Vec<u8>,
    pub content_type: String,
    pub name: String,
}

fn is_valid_segment(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// Normalise + validate a folder prefix (`""` = bucket root).
/// Throws 400 with v1's message.
pub fn validate_prefix(raw: &str) -> Result<String, WordlessError> {
    let prefix = raw.trim().trim_matches('/');
    if prefix.is_empty() {
        return Ok(String::new());
    }
    if prefix.contains('\\') || prefix.contains("..") {
        return Err(bad_request(&format!("Invalid folder path \"{raw}\".")));
    }
    if prefix.split('/').any(|s| !is_valid_segment(s)) {
        return Err(bad_request(&format!("Invalid folder path \"{raw}\".")));
    }
    Ok(prefix.to_string())
}

/// Validate a full object key (relative path, no traversal).
/// Throws 400 with v1's message.
pub fn validate_file_key(raw: &str) -> Result<String, WordlessError> {
    let key = raw.trim();
    if key.is_empty()
        || key.starts_with('/')
        || key.ends_with('/')
        || key.contains('\\')
        || key.contains("..")
    {
        return Err(bad_request(&format!("Invalid file path \"{raw}\".")));
    }
    if key.split('/').any(|s| !is_valid_segment(s)) {
        return Err(bad_request(&format!("Invalid file path \"{raw}\".")));
    }
    Ok(key.to_string())
}

/// Validate a single file name (one segment, no separators).
/// Throws 400 with v1's message.
pub fn validate_file_name(raw: &str) -> Result<String, WordlessError> {
    let name = raw.trim();
    if name.is_empty() || name.len() > 180 || !is_valid_segment(name) || name == "." || name == ".."
    {
        return Err(bad_request(&format!("Invalid file name \"{raw}\".")));
    }
    Ok(name.to_string())
}

fn is_managed(key: &str) -> bool {
    MANAGED_PREFIXES.iter().any(|p| key.starts_with(p))
}

fn managed_message(key: &str, verb: &str) -> WordlessError {
    let first = key.split('/').next().unwrap_or("");
    bad_request(&format!(
        "Files under \"{first}/\" are managed by their own page and cannot be {verb} here."
    ))
}

fn type_of(key: &str) -> String {
    match key.rfind('.') {
        Some(dot) => {
            let ext = key[dot + 1..].to_lowercase();
            if !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
                ext
            } else {
                "file".to_string()
            }
        }
        None => "file".to_string(),
    }
}

async fn media_keys(db: &dyn Db, site_id: i64) -> Result<std::collections::HashSet<String>, WordlessError> {
    let rows = db
        .query_all(
            "SELECT meta_value FROM wp_postmeta WHERE meta_key = '_wp_attached_file' AND site_id = ?",
            &[Param::Integer(site_id)],
        )
        .await?;
    let mut out = std::collections::HashSet::new();
    for row in &rows {
        if let Some(v) = row.get("meta_value").and_then(|v| v.as_str()) {
            if !v.is_empty() {
                out.insert(v.to_string());
            }
        }
    }
    Ok(out)
}

/// List one folder: immediate subfolders + files directly inside it
/// (v1 `listFiles`). Pages through the whole prefix (v1's
/// `listObjects` loops all R2 pages), then caps the result at 1000
/// entries; `truncated` is computed on the full counts, as in v1.
pub async fn list_files(
    db: &dyn Db,
    site_id: i64,
    store: &dyn ObjectStore,
    raw_prefix: &str,
) -> Result<FileListResult, WordlessError> {
    let prefix = validate_prefix(raw_prefix)?;
    let list_prefix = if prefix.is_empty() {
        String::new()
    } else {
        format!("{prefix}/")
    };
    let mut objects = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = store
            .list_objects(&list_prefix, MAX_FILE_LIST_ENTRIES, cursor.as_deref())
            .await?;
        objects.extend(page.objects);
        if !page.truncated {
            break;
        }
        cursor = page.cursor;
        if cursor.is_none() {
            break;
        }
    }
    let attachments = media_keys(db, site_id).await?;
    let mut folders = std::collections::HashSet::new();
    let mut files: Vec<FileEntry> = Vec::new();
    for obj in &objects {
        let Some(rest) = obj.key.strip_prefix(&list_prefix) else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        match rest.find('/') {
            Some(slash) => {
                folders.insert(rest[..slash].to_string());
            }
            None => files.push(FileEntry {
                key: obj.key.clone(),
                name: rest.to_string(),
                size: obj.size as i64,
                file_type: type_of(&obj.key),
                modified: obj.uploaded_at.clone(),
                is_media: attachments.contains(&obj.key),
            }),
        }
    }
    let truncated = files.len() + folders.len() > MAX_FILE_LIST_ENTRIES;
    let mut folders: Vec<String> = folders.into_iter().collect();
    folders.sort();
    folders.truncate(MAX_FILE_LIST_ENTRIES);
    files.sort_by(|a, b| a.name.cmp(&b.name));
    files.truncate(MAX_FILE_LIST_ENTRIES);
    Ok(FileListResult {
        prefix,
        folders,
        files,
        truncated,
    })
}

/// Upload a file into a folder. Refused under managed prefixes (v1
/// `uploadFile`).
pub async fn upload_file(
    store: &dyn ObjectStore,
    raw_prefix: &str,
    input: UploadFileInput,
) -> Result<FileEntry, WordlessError> {
    let prefix = validate_prefix(raw_prefix)?;
    let name = validate_file_name(&input.filename)?;
    let key = if prefix.is_empty() {
        name.clone()
    } else {
        format!("{prefix}/{name}")
    };
    if is_managed(&key) {
        return Err(managed_message(&key, "uploaded"));
    }
    validate_upload(&input.mime_type, input.data.len())?;
    verify_upload_bytes(&name, &input.mime_type, &input.data)?;
    if store.head_object(&key).await?.is_some() {
        return Err(bad_request(&format!(
            "A file named \"{name}\" already exists in this folder."
        )));
    }
    store
        .put_object(&key, input.data.clone(), &input.mime_type)
        .await?;
    Ok(FileEntry {
        key: key.clone(),
        name,
        size: input.data.len() as i64,
        file_type: type_of(&key),
        modified: Some(now_iso()),
        is_media: false,
    })
}

/// Read a file's bytes for download (v1 `downloadFile`).
pub async fn download_file(
    store: &dyn ObjectStore,
    raw_key: &str,
) -> Result<DownloadedFile, WordlessError> {
    let key = validate_file_key(raw_key)?;
    let Some((data, content_type)) = store.get_object(&key).await? else {
        return Err(not_found(&format!("File \"{key}\" not found.")));
    };
    Ok(DownloadedFile {
        data,
        content_type: content_type.unwrap_or_else(|| "application/octet-stream".to_string()),
        name: key.rsplit('/').next().unwrap_or(&key).to_string(),
    })
}

/// Rename a file within its folder (copy + delete). Media records
/// follow the new key (v1 `renameFile`).
pub async fn rename_file(
    db: &dyn Db,
    site_id: i64,
    store: &dyn ObjectStore,
    raw_key: &str,
    raw_new_name: &str,
) -> Result<FileEntry, WordlessError> {
    let key = validate_file_key(raw_key)?;
    let new_name = validate_file_name(raw_new_name)?;
    if is_managed(&key) {
        return Err(managed_message(&key, "renamed"));
    }
    let new_key = match key.rfind('/') {
        Some(slash) => format!("{}{}", &key[..slash + 1], new_name),
        None => new_name.clone(),
    };
    if new_key == key {
        return Err(bad_request("The new name is the same as the current name."));
    }
    if store.head_object(&new_key).await?.is_some() {
        return Err(bad_request(&format!(
            "A file named \"{new_name}\" already exists in this folder."
        )));
    }
    let Some((data, content_type)) = store.get_object(&key).await? else {
        return Err(not_found(&format!("File \"{key}\" not found.")));
    };
    let content_type = content_type.unwrap_or_else(|| "application/octet-stream".to_string());
    store
        .put_object(&new_key, data.clone(), &content_type)
        .await?;
    store.delete_object(&key).await?;
    let attachment = rename_media_object(db, site_id, &key, &new_key).await?;
    Ok(FileEntry {
        key: new_key.clone(),
        name: new_name,
        size: data.len() as i64,
        file_type: type_of(&new_key),
        modified: Some(now_iso()),
        is_media: attachment.is_some(),
    })
}

/// Delete a file. A media attachment's library record is deleted too
/// (v1 `deleteFile`).
pub async fn delete_file(
    db: &dyn Db,
    site_id: i64,
    store: &dyn ObjectStore,
    raw_key: &str,
) -> Result<DeleteFileResult, WordlessError> {
    let key = validate_file_key(raw_key)?;
    if is_managed(&key) {
        return Err(managed_message(&key, "deleted"));
    }
    if let Some(attachment) = find_media_by_key(db, site_id, &key).await? {
        delete_media(db, site_id, store, attachment.id).await?;
        return Ok(DeleteFileResult { deleted: true });
    }
    // The kernel store's delete is a no-op for missing keys, so check
    // existence first to keep v1's 404 for a missing file.
    if store.head_object(&key).await?.is_none() {
        return Err(not_found(&format!("File \"{key}\" not found.")));
    }
    store.delete_object(&key).await?;
    Ok(DeleteFileResult { deleted: true })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        service::{upload_media, UploadMediaInput as MediaUpload},
        test_support::{block_on, FakeDb, FakeStore},
    };

    const PNG: &[u8] = &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3];

    /// Mirrors the v1 files.test.ts seed: managed-prefix objects plus
    /// one media attachment.
    async fn seeded() -> (FakeDb, FakeStore) {
        let db = FakeDb::new();
        let store = FakeStore::new();
        store
            .put_object(
                "extensions/my-plugin/plugin.php",
                b"<?php // hi".to_vec(),
                "application/octet-stream",
            )
            .await
            .unwrap();
        store
            .put_object(
                "extensions/my-plugin/readme.txt",
                b"readme".to_vec(),
                "text/plain",
            )
            .await
            .unwrap();
        store
            .put_object(
                "themes/my-theme/style.css",
                b"/* theme */".to_vec(),
                "text/css",
            )
            .await
            .unwrap();
        store
            .put_object("backups/backup-1.zip", vec![1, 2], "application/zip")
            .await
            .unwrap();
        upload_media(
            &db, 1,
            &store,
            MediaUpload {
                title: None,
                filename: "photo.png".to_string(),
                mime_type: "image/png".to_string(),
                data: PNG.to_vec(),
                author_id: 1,
                alt: None,
            },
        )
        .await
        .unwrap();
        (db, store)
    }

    fn file_input(name: &str, mime: &str, data: &[u8]) -> UploadFileInput {
        UploadFileInput {
            filename: name.to_string(),
            mime_type: mime.to_string(),
            data: data.to_vec(),
        }
    }

    #[test]
    fn validators_accept_and_reject_like_v1() {
        assert_eq!(validate_prefix("").unwrap(), "");
        assert_eq!(validate_prefix("uploads/2026").unwrap(), "uploads/2026");
        assert_eq!(validate_prefix("/absolute").unwrap(), "absolute");
        assert_eq!(validate_file_key("photo.png").unwrap(), "photo.png");
        assert_eq!(
            validate_file_key("extensions/my-plugin/readme.txt").unwrap(),
            "extensions/my-plugin/readme.txt"
        );
        assert_eq!(validate_file_name("photo-2.png").unwrap(), "photo-2.png");

        assert!(validate_prefix("../secret").is_err());
        assert!(validate_prefix("a/../../b").is_err());
        assert!(validate_prefix("a\\b").is_err());
        assert!(validate_file_key("../wp-config.php").is_err());
        assert!(validate_file_key("/etc/passwd").is_err());
        assert!(validate_file_key("a//b").is_err());
        assert!(validate_file_key("folder/").is_err());
        assert!(validate_file_key("a\\b.txt").is_err());
        assert!(validate_file_name("../evil.php").is_err());
        assert!(validate_file_name("a/b.txt").is_err());
        assert!(validate_file_name("").is_err());
        // Messages are v1's.
        assert_eq!(
            validate_prefix("../secret").unwrap_err().message,
            "Invalid folder path \"../secret\"."
        );
        assert_eq!(
            validate_file_key("/etc/passwd").unwrap_err().message,
            "Invalid file path \"/etc/passwd\"."
        );
    }

    #[test]
    fn list_root_and_nested_prefixes() {
        block_on(async {
            let (db, store) = seeded().await;
            let root = list_files(&db, 1, &store, "").await.unwrap();
            assert_eq!(root.prefix, "");
            assert_eq!(root.folders, vec!["backups", "extensions", "themes"]);
            assert_eq!(root.files.len(), 1);
            assert!(root.files[0].name.ends_with("photo.png"));
            assert!(root.files[0].is_media);
            assert_eq!(root.files[0].size, PNG.len() as i64);
            assert_eq!(root.files[0].file_type, "png");
            assert!(!root.truncated);

            let ext = list_files(&db, 1, &store, "extensions/my-plugin")
                .await
                .unwrap();
            assert!(ext.folders.is_empty());
            let names: Vec<&str> = ext.files.iter().map(|f| f.name.as_str()).collect();
            assert_eq!(names, vec!["plugin.php", "readme.txt"]);
            assert!(ext.files.iter().all(|f| !f.is_media));
        });
    }

    #[test]
    fn upload_refuses_managed_prefixes_duplicates_and_bad_types() {
        block_on(async {
            let (_db, store) = seeded().await;
            let entry = upload_file(&store, "uploads", file_input("pic.png", "image/png", PNG))
                .await
                .unwrap();
            assert_eq!(entry.key, "uploads/pic.png");
            assert!(store.contains("uploads/pic.png"));

            // Managed prefix.
            let err = upload_file(
                &store,
                "extensions/my-plugin",
                file_input("x.png", "image/png", PNG),
            )
            .await
            .unwrap_err();
            assert!(err.message.contains("managed by their own page"));
            assert_eq!(
                err.message,
                "Files under \"extensions/\" are managed by their own page and cannot be uploaded here."
            );
            // Duplicate.
            upload_file(&store, "", file_input("dup.png", "image/png", PNG))
                .await
                .unwrap();
            let err = upload_file(&store, "", file_input("dup.png", "image/png", PNG))
                .await
                .unwrap_err();
            assert_eq!(
                err.message,
                "A file named \"dup.png\" already exists in this folder."
            );
            // Disallowed type / oversized.
            let err = upload_file(&store, "", file_input("run.php", "application/x-php", &[1]))
                .await
                .unwrap_err();
            assert!(err.message.contains("not allowed"));
            let big = vec![0u8; 50 * 1024 * 1024 + 1];
            let err = upload_file(&store, "", file_input("big.png", "image/png", &big))
                .await
                .unwrap_err();
            assert!(err.message.contains("too large"));
        });
    }

    #[test]
    fn rename_and_delete_follow_media_records() {
        block_on(async {
            let (db, store) = seeded().await;
            // Rename of a missing file 404s.
            assert!(rename_file(&db, 1, &store, "uploads-x.png", "x.png")
                .await
                .unwrap_err()
                .message
                .contains("not found"));
            let entry = upload_file(&store, "", file_input("old.png", "image/png", PNG))
                .await
                .unwrap();
            assert_eq!(entry.key, "old.png");
            let done = rename_file(&db, 1, &store, "old.png", "new.png")
                .await
                .unwrap();
            assert_eq!(done.key, "new.png");
            assert!(!store.contains("old.png"));
            assert!(store.contains("new.png"));
            // Media attachment rename follows the record.
            let root = list_files(&db, 1, &store, "").await.unwrap();
            let media_key = root.files.iter().find(|f| f.is_media).unwrap().key.clone();
            let media_renamed = rename_file(&db, 1, &store, &media_key, "renamed-photo.png")
                .await
                .unwrap();
            assert!(media_renamed.is_media);
            let after = list_files(&db, 1, &store, "").await.unwrap();
            let media_after = after.files.iter().find(|f| f.is_media).unwrap();
            assert_eq!(media_after.name, "renamed-photo.png");

            // Refusals.
            let err = rename_file(&db, 1, &store, "extensions/my-plugin/readme.txt", "other.txt")
                .await
                .unwrap_err();
            assert!(err.message.contains("managed by their own page"));
            upload_file(&store, "", file_input("a.png", "image/png", PNG))
                .await
                .unwrap();
            upload_file(&store, "", file_input("b.png", "image/png", PNG))
                .await
                .unwrap();
            let err = rename_file(&db, 1, &store, "a.png", "b.png")
                .await
                .unwrap_err();
            assert!(err.message.contains("already exists"));
            let err = rename_file(&db, 1, &store, "a.png", "a.png")
                .await
                .unwrap_err();
            assert_eq!(err.message, "The new name is the same as the current name.");

            // Delete plain file, then media attachment (record included).
            upload_file(&store, "", file_input("gone.png", "image/png", PNG))
                .await
                .unwrap();
            assert!(delete_file(&db, 1, &store, "gone.png").await.unwrap().deleted);
            assert!(!store.contains("gone.png"));
            assert!(delete_file(&db, 1, &store, "gone.png")
                .await
                .unwrap_err()
                .message
                .contains("not found"));
            let root = list_files(&db, 1, &store, "").await.unwrap();
            let media_key = root.files.iter().find(|f| f.is_media).unwrap().key.clone();
            assert!(delete_file(&db, 1, &store, &media_key).await.unwrap().deleted);
            let after = list_files(&db, 1, &store, "").await.unwrap();
            assert!(after.files.iter().all(|f| !f.is_media));

            // Managed deletes refused.
            let err = delete_file(&db, 1, &store, "backups/backup-1.zip")
                .await
                .unwrap_err();
            assert!(err.message.contains("managed by their own page"));
            let err = delete_file(&db, 1, &store, "themes/my-theme/style.css")
                .await
                .unwrap_err();
            assert!(err.message.contains("managed by their own page"));
        });
    }

    #[test]
    fn download_returns_bytes_and_rejects_traversal() {
        block_on(async {
            let (_db, store) = seeded().await;
            let got = download_file(&store, "extensions/my-plugin/readme.txt")
                .await
                .unwrap();
            assert_eq!(got.data, b"readme");
            assert_eq!(got.name, "readme.txt");
            assert!(download_file(&store, "missing.txt").await.is_err());
            assert!(download_file(&store, "../secret.txt").await.is_err());
        });
    }
}
