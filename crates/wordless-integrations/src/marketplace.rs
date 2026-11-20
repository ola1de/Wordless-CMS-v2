//! Marketplace (port of v1 `marketplace.ts`, M8 + M11).
//!
//! Deliberately small and honest: an admin installs an extension
//! from a ZIP upload carrying a `wordless.json` manifest (slug,
//! name, version, kind). Files are stored in R2 under
//! `extensions/<slug>/` and the extension is recorded in
//! `wl_extensions` (the same table the conversion pipeline uses),
//! starting inactive. There is NO remote directory, NO remote
//! download, and NO claim that an extension from a ZIP is safe to
//! run — uploaded extensions are inert records until a later
//! milestone loads their code. **Installed code is NEVER executed.**
//!
//! M11 compatibility: the worker wiring runs the PHP analyser
//! (parallel `wordless-conversion` crate) over the installed files
//! and passes its report JSON as the `report` argument; it is
//! stored in `wl_extensions.report_json` via the install. When no
//! report is supplied the install still succeeds and the extension
//! is recorded as "not analysed" (`report: None`). No PHP is
//! analysed inside this crate, and no converted code is generated
//! here — the M11 auto-convert step is the conversion crate's job
//! and its output is stored by the wiring, not by this function.

use std::io::{Cursor, Read};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use wordless_db::Db;
use wordless_kernel::{
    error::{bad_request, WordlessError},
    storage::ObjectStore,
    validate::require_string,
};

use crate::extensions::{
    get_extension, install_extension, list_extensions, set_extension_status, ExtensionRecord,
    ExtensionStatus, InstallExtensionInput,
};

/// Maximum marketplace ZIP size in bytes (50 MB, M11).
pub const MAX_MARKETPLACE_ZIP_BYTES: usize = 50 * 1024 * 1024;
/// Maximum number of files in a marketplace ZIP (M11).
pub const MAX_MARKETPLACE_FILES: usize = 10_000;

/// The `wordless.json` manifest (v1 `MarketplaceManifest`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceManifest {
    pub slug: String,
    pub name: String,
    pub version: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Result of a marketplace install (v1 `MarketplaceInstallResult`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceInstallResult {
    pub extension: ExtensionRecord,
    /// Number of files stored in R2.
    pub files: usize,
    /// The compatibility report passed in by the caller (the
    /// analyser's JSON), or `None` when no report was supplied —
    /// "not analysed", not "compatible".
    pub report: Option<Value>,
}

/// Parse + validate a `wordless.json` manifest, with v1's messages.
pub fn parse_manifest(raw: &str) -> Result<MarketplaceManifest, WordlessError> {
    let parsed: Value =
        serde_json::from_str(raw).map_err(|_| bad_request("wordless.json is not valid JSON."))?;
    if !parsed.is_object() {
        return Err(bad_request(
            "wordless.json must be a JSON object with slug, name and version.",
        ));
    }
    let slug = require_string(
        parsed.get("slug").unwrap_or(&Value::Null),
        "manifest.slug",
        1,
        100,
    )?
    .to_lowercase();
    if !crate::extensions::is_valid_slug(&slug) {
        return Err(bad_request(
            "Manifest slug must be lowercase letters, digits and hyphens only.",
        ));
    }
    let name = require_string(
        parsed.get("name").unwrap_or(&Value::Null),
        "manifest.name",
        1,
        200,
    )?;
    let version = require_string(
        parsed.get("version").unwrap_or(&Value::Null),
        "manifest.version",
        1,
        20,
    )?;
    let version_ok = {
        let parts: Vec<&str> = version.split('.').collect();
        parts.len() == 3
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    };
    if !version_ok {
        return Err(bad_request("Manifest version must look like \"1.0.0\"."));
    }
    let kind = match parsed.get("kind").and_then(Value::as_str) {
        Some(k) if !k.trim().is_empty() => k.trim().to_string(),
        _ => "plugin".to_string(),
    };
    if kind != "plugin" && kind != "theme" {
        return Err(bad_request(
            "Manifest kind must be \"plugin\" or \"theme\".",
        ));
    }
    Ok(MarketplaceManifest {
        slug,
        name,
        version,
        kind,
        description: parsed
            .get("description")
            .and_then(Value::as_str)
            .map(|s| s.to_string()),
    })
}

/// Extract + validate a marketplace ZIP: size cap, file-count cap,
/// no path traversal, single top-level folder stripped. Unlike the
/// theme importer, no file-extension allowlist applies here — every
/// file in the ZIP is stored (v1 `marketplace.ts`).
fn extract_marketplace_zip(data: &[u8]) -> Result<Vec<(String, Vec<u8>)>, WordlessError> {
    if data.is_empty() {
        return Err(bad_request("The uploaded ZIP is empty."));
    }
    if data.len() > MAX_MARKETPLACE_ZIP_BYTES {
        return Err(bad_request(&format!(
            "The ZIP is {} bytes; the limit is {} bytes (50 MB).",
            data.len(),
            MAX_MARKETPLACE_ZIP_BYTES
        )));
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(data)).map_err(|_| {
        bad_request("Could not read the uploaded ZIP file. It must be a valid .zip archive.")
    })?;
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|_| {
            bad_request("Could not read the uploaded ZIP file. It must be a valid .zip archive.")
        })?;
        let name = entry.name().to_string();
        if entry.is_dir() || name.ends_with('/') {
            continue;
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes).map_err(|_| {
            bad_request("Could not read the uploaded ZIP file. It must be a valid .zip archive.")
        })?;
        entries.push((name, bytes));
    }
    if entries.is_empty() {
        return Err(bad_request("The ZIP contains no files."));
    }
    if entries.len() > MAX_MARKETPLACE_FILES {
        return Err(bad_request(&format!(
            "The ZIP contains {} files; the limit is {}.",
            entries.len(),
            MAX_MARKETPLACE_FILES
        )));
    }
    // A ZIP usually has one top-level folder (myext/…). Strip it
    // when every file shares the same first segment.
    let first_segments: std::collections::HashSet<&str> = entries
        .iter()
        .map(|(path, _)| path.split('/').next().unwrap_or(""))
        .collect();
    let strip_prefix =
        if first_segments.len() == 1 && entries.iter().all(|(path, _)| path.contains('/')) {
            format!("{}/", first_segments.iter().next().unwrap())
        } else {
            String::new()
        };
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for (path, bytes) in entries {
        let relative = if strip_prefix.is_empty() {
            path.clone()
        } else {
            path.strip_prefix(&strip_prefix)
                .unwrap_or(&path)
                .to_string()
        };
        if relative.is_empty() || relative.starts_with('/') || relative.contains("..") {
            return Err(bad_request(&format!(
                "The ZIP contains an unsafe path: \"{path}\"."
            )));
        }
        files.push((relative, bytes));
    }
    Ok(files)
}

/// Validate + install an extension ZIP. Throws 400 on any bad input;
/// stores nothing on failure (all validation, including the
/// manifest, runs before the first R2 write).
///
/// `report` is the compatibility-report JSON produced by the
/// wordless-conversion analyser (passed in by the worker wiring —
/// this crate never analyses PHP). `None` records the install as
/// "not analysed".
///
/// # Errors
/// * 400 on a non-ZIP filename, empty/oversized ZIP, unreadable ZIP,
///   too many files, an unsafe path, a missing/invalid manifest, or
///   a non-object `report`.
/// * 500 on a database or storage failure.
pub async fn install_extension_from_zip(
    db: &dyn Db,
    store: &dyn ObjectStore,
    filename: &str,
    data: &[u8],
    report: Option<&Value>,
) -> Result<MarketplaceInstallResult, WordlessError> {
    if !filename.to_lowercase().ends_with(".zip") {
        return Err(bad_request("The marketplace install requires a .zip file."));
    }
    if let Some(r) = report {
        if !r.is_object() {
            return Err(bad_request("Compatibility report must be a JSON object."));
        }
    }
    let files = extract_marketplace_zip(data)?;
    let manifest_raw = files
        .iter()
        .find(|(path, _)| path == "wordless.json")
        .map(|(_, bytes)| bytes.clone());
    let Some(manifest_raw) = manifest_raw else {
        return Err(bad_request(
            "The ZIP must contain a wordless.json manifest at its root (slug, name, version, kind).",
        ));
    };
    let manifest_text = String::from_utf8_lossy(&manifest_raw).into_owned();
    let manifest = parse_manifest(&manifest_text)?;
    // Store files first; the wl_extensions row is the record of the
    // install. (R2 has no transaction: if a put fails midway, files
    // already stored stay stored — same limit as v1.)
    for (path, bytes) in &files {
        store
            .put_object(
                &format!("extensions/{}/{path}", manifest.slug),
                bytes.clone(),
                "application/octet-stream",
            )
            .await?;
    }
    let extension = install_extension(
        db,
        &InstallExtensionInput {
            slug: manifest.slug.clone(),
            name: manifest.name.clone(),
            version: Some(manifest.version.clone()),
            kind: Some(manifest.kind.clone()),
            source: Some("marketplace:upload".to_string()),
            status: Some(ExtensionStatus::Inactive),
            report: report.cloned(),
        },
    )
    .await?;
    Ok(MarketplaceInstallResult {
        extension,
        files: files.len(),
        report: report.cloned(),
    })
}

/// All installed extensions, ordered by slug.
///
/// # Errors
/// Returns `Err` only on a database failure.
pub async fn list_marketplace_extensions(
    db: &dyn Db,
) -> Result<Vec<ExtensionRecord>, WordlessError> {
    list_extensions(db).await
}

/// The stored compatibility report for one installed extension
/// (`None` = never analysed).
///
/// # Errors
/// * 404 when the slug is not installed.
/// * 500 on a database failure.
pub async fn get_marketplace_extension_report(
    db: &dyn Db,
    slug: &str,
) -> Result<Option<Value>, WordlessError> {
    let extension = get_extension(db, slug).await?;
    Ok(extension.report)
}

/// Activate or deactivate an installed extension. Activation only
/// records the status — it does not load or run any code.
///
/// # Errors
/// * 404 when the slug is not installed.
/// * 500 on a database failure.
pub async fn set_marketplace_extension_status(
    db: &dyn Db,
    slug: &str,
    status: ExtensionStatus,
) -> Result<ExtensionRecord, WordlessError> {
    set_extension_status(db, slug, status).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{block_on, FakeStore, SqliteDb};
    use serde_json::json;
    use std::io::Write;
    use zip::write::{SimpleFileOptions, ZipWriter};

    fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (path, content) in files {
            writer.start_file(*path, opts).unwrap();
            writer.write_all(content).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn manifest_zip(slug: &str) -> Vec<u8> {
        zip_of(&[
            (
                &format!("{slug}/wordless.json"),
                format!(
                    "{{\"slug\": \"{slug}\", \"name\": \"Mixed Plugin\", \"version\": \"1.0.0\", \"kind\": \"plugin\"}}"
                )
                .as_bytes(),
            ),
            (&format!("{slug}/index.php"), b"<?php // hello"),
            (&format!("{slug}/readme.txt"), b"readme"),
        ])
    }

    #[test]
    fn installs_a_valid_extension_zip_and_records_it_inactive() {
        let db = SqliteDb::new();
        let store = FakeStore::new();
        let report = json!({
            "phpFiles": 1, "totalFiles": 3, "convertible": 1, "partial": 0,
            "blocked": 0, "unparsed": 0, "skipped": 0, "verdict": "compatible",
            "topReasons": [], "files": [], "filesTruncated": false,
            "optionDefaults": [], "conversion": null, "analysedAt": "2026-11-19T00:00:00Z"
        });
        let result = block_on(install_extension_from_zip(
            &db,
            &store,
            "mixed-plugin.zip",
            &manifest_zip("mixed-plugin"),
            Some(&report),
        ))
        .unwrap();
        assert_eq!(result.extension.slug, "mixed-plugin");
        assert_eq!(result.extension.status, "inactive");
        assert_eq!(result.files, 3);
        assert_eq!(
            result.report.as_ref().unwrap()["verdict"],
            json!("compatible")
        );
        assert!(store.contains("extensions/mixed-plugin/wordless.json"));
        assert!(store.contains("extensions/mixed-plugin/index.php"));
        let listed = block_on(list_marketplace_extensions(&db)).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].report.as_ref().unwrap()["verdict"],
            json!("compatible")
        );
        let fetched = block_on(get_marketplace_extension_report(&db, "mixed-plugin")).unwrap();
        assert_eq!(fetched.unwrap()["verdict"], json!("compatible"));
        let active = block_on(set_marketplace_extension_status(
            &db,
            "mixed-plugin",
            ExtensionStatus::Active,
        ))
        .unwrap();
        assert_eq!(active.status, "active");
    }

    #[test]
    fn install_without_a_report_records_not_analysed() {
        let db = SqliteDb::new();
        let store = FakeStore::new();
        let result = block_on(install_extension_from_zip(
            &db,
            &store,
            "plain.zip",
            &manifest_zip("plain"),
            None,
        ))
        .unwrap();
        assert_eq!(result.report, None);
        assert_eq!(result.extension.report, None);
        assert_eq!(
            block_on(get_marketplace_extension_report(&db, "plain")).unwrap(),
            None
        );
    }

    #[test]
    fn rejects_a_zip_without_a_manifest() {
        let db = SqliteDb::new();
        let store = FakeStore::new();
        let zip = zip_of(&[("no-manifest/index.php", b"<?php")]);
        let err =
            block_on(install_extension_from_zip(&db, &store, "x.zip", &zip, None)).unwrap_err();
        assert!(err.message.contains("wordless.json"), "{}", err.message);
        assert_eq!(store.len(), 0, "nothing is stored on failure");
    }

    #[test]
    fn rejects_a_bad_manifest_version() {
        let db = SqliteDb::new();
        let store = FakeStore::new();
        let zip = zip_of(&[(
            "bad/wordless.json",
            b"{\"slug\": \"bad\", \"name\": \"Bad\", \"version\": \"1.0\", \"kind\": \"plugin\"}",
        )]);
        let err = block_on(install_extension_from_zip(
            &db, &store, "bad.zip", &zip, None,
        ))
        .unwrap_err();
        assert!(err.message.contains("1.0.0"), "{}", err.message);
    }

    #[test]
    fn rejects_path_traversal_in_the_zip() {
        let db = SqliteDb::new();
        let store = FakeStore::new();
        let zip = zip_of(&[
            ("evil/wordless.json", b"{\"slug\": \"evil\", \"name\": \"E\", \"version\": \"1.0.0\", \"kind\": \"plugin\"}"),
            ("../evil.txt", b"x"),
        ]);
        let err = block_on(install_extension_from_zip(
            &db, &store, "evil.zip", &zip, None,
        ))
        .unwrap_err();
        assert!(err.message.contains("unsafe path"), "{}", err.message);
    }

    #[test]
    fn rejects_a_non_zip_upload() {
        let db = SqliteDb::new();
        let store = FakeStore::new();
        let err = block_on(install_extension_from_zip(
            &db,
            &store,
            "plugin.tar.gz",
            b"not a zip",
            None,
        ))
        .unwrap_err();
        assert!(err.message.contains(".zip"), "{}", err.message);
    }

    #[test]
    fn rejects_a_zip_over_50_mb() {
        let db = SqliteDb::new();
        let store = FakeStore::new();
        let big = vec![0u8; 51 * 1024 * 1024];
        let err = block_on(install_extension_from_zip(
            &db, &store, "big.zip", &big, None,
        ))
        .unwrap_err();
        assert!(err.message.contains("50 MB"), "{}", err.message);
    }

    #[test]
    fn rejects_a_report_lookup_for_an_unknown_slug() {
        let db = SqliteDb::new();
        let err = block_on(get_marketplace_extension_report(&db, "nope")).unwrap_err();
        assert_eq!(err.status, 404);
        assert!(err.message.contains("not installed"), "{}", err.message);
    }

    #[test]
    fn limits_are_the_m11_values() {
        assert_eq!(MAX_MARKETPLACE_ZIP_BYTES, 50 * 1024 * 1024);
        assert_eq!(MAX_MARKETPLACE_FILES, 10_000);
    }
}
