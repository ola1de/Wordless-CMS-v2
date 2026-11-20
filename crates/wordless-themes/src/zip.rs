//! Theme ZIP handling (P2) — port of v1 `src/modules/themes/zip.ts`.
//!
//! Extracts an uploaded theme ZIP with the `zip` crate (in-memory
//! `Cursor`, no Worker types). Validates structure before anything is
//! stored: size cap, file count cap, no path traversal, and a single
//! top-level folder (the usual WordPress theme ZIP layout) is stripped.
//!
//! Caps (v1.1.0, M11): 10,000 files / 50 MB per ZIP.

use std::io::{Cursor, Read};

use wordless_kernel::error::{bad_request, WordlessError};

/// Maximum theme ZIP size in bytes (50 MB).
pub const MAX_THEME_ZIP_BYTES: usize = 50 * 1024 * 1024;
/// Maximum number of files in a theme ZIP.
pub const MAX_THEME_FILES: usize = 10_000;

/// File extensions a theme ZIP may contain (others are skipped with a
/// warning). Same set as v1 `ALLOWED_THEME_EXTENSIONS`.
pub const ALLOWED_THEME_EXTENSIONS: &[&str] = &[
    ".html", ".css", ".json", ".php", ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".woff",
    ".woff2", ".ttf", ".otf", ".txt", ".md",
];

/// An extracted theme ZIP: relative path → raw bytes, in ZIP order,
/// plus non-fatal warnings (skipped files).
#[derive(Debug, Clone, Default)]
pub struct ExtractedThemeZip {
    /// Relative path → raw bytes (text files decoded by the caller).
    pub files: Vec<(String, Vec<u8>)>,
    pub warnings: Vec<String>,
}

impl ExtractedThemeZip {
    /// Raw bytes of one file, or `None` when it is not in the ZIP.
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.files
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, bytes)| bytes.as_slice())
    }

    /// True when the ZIP contains `path`.
    pub fn contains(&self, path: &str) -> bool {
        self.get(path).is_some()
    }

    /// Relative paths, in ZIP order.
    pub fn paths(&self) -> Vec<&str> {
        self.files.iter().map(|(p, _)| p.as_str()).collect()
    }

    /// All files whose path starts with `prefix`, as path → text.
    pub fn text_files_under(&self, prefix: &str) -> Vec<(String, String)> {
        self.files
            .iter()
            .filter(|(path, _)| path.starts_with(prefix))
            .map(|(path, bytes)| (path.clone(), String::from_utf8_lossy(bytes).into_owned()))
            .collect()
    }
}

/// Extract + validate a theme ZIP. Returns `bad_request` (400) on bad
/// input, with v1's error messages.
pub fn extract_theme_zip(data: &[u8]) -> Result<ExtractedThemeZip, WordlessError> {
    if data.is_empty() {
        return Err(bad_request("The uploaded ZIP is empty."));
    }
    if data.len() > MAX_THEME_ZIP_BYTES {
        return Err(bad_request(&format!(
            "Theme ZIP is {} bytes; the limit is {} bytes (50 MB).",
            data.len(),
            MAX_THEME_ZIP_BYTES
        )));
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(data)).map_err(|_| {
        bad_request("Could not read the uploaded ZIP file. It must be a valid .zip archive.")
    })?;
    // Directory entries are skipped before the count, as in v1.
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
    if entries.len() > MAX_THEME_FILES {
        return Err(bad_request(&format!(
            "The ZIP contains {} files; the limit is {}.",
            entries.len(),
            MAX_THEME_FILES
        )));
    }
    // A WordPress theme ZIP usually has one top-level folder (mytheme/…).
    // Strip it when every file shares the same first segment.
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
    let mut warnings: Vec<String> = Vec::new();
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
        if relative
            .split('/')
            .any(|seg| seg == "__MACOSX" || seg.starts_with('.'))
        {
            continue; // Skip macOS metadata / hidden files silently.
        }
        let dot = relative.rfind('.');
        let ext = match dot {
            Some(d) => relative[d..].to_lowercase(),
            None => String::new(),
        };
        if !ALLOWED_THEME_EXTENSIONS.contains(&ext.as_str()) {
            warnings.push(format!(
                "Skipped unsupported file \"{relative}\" (extension \"{ext}\")."
            ));
            continue;
        }
        files.push((relative, bytes));
    }
    if files.is_empty() {
        return Err(bad_request("The ZIP contains no usable theme files."));
    }
    Ok(ExtractedThemeZip { files, warnings })
}

/// Read a text file from an extracted ZIP (errors when missing).
pub fn zip_text_file(files: &ExtractedThemeZip, path: &str) -> Result<String, WordlessError> {
    match files.get(path) {
        Some(bytes) => Ok(String::from_utf8_lossy(bytes).into_owned()),
        None => Err(bad_request(&format!(
            "The theme ZIP is missing \"{path}\"."
        ))),
    }
}

/// All text files under a prefix (templates/, parts/), as path → string.
pub fn zip_text_files_under(files: &ExtractedThemeZip, prefix: &str) -> Vec<(String, String)> {
    files.text_files_under(prefix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::{SimpleFileOptions, ZipWriter};

    /// Build a ZIP in memory from (path, content) pairs (stored, no
    /// compression — the reader handles both).
    fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (path, content) in files {
            writer.start_file(*path, opts).unwrap();
            writer.write_all(content).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn reads_deflated_zip_entries() {
        // Real-world theme ZIPs are deflated (fflate / zip tools), not
        // stored — prove the reader handles them.
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("mytheme/style.css", opts).unwrap();
        writer
            .write_all(b"/*\nTheme Name: Deflated\nVersion: 1.0.0\n*/\nbody { color: red; }")
            .unwrap();
        writer.start_file("mytheme/theme.json", opts).unwrap();
        writer.write_all(b"{\"version\": 3}").unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let out = extract_theme_zip(&bytes).unwrap();
        assert_eq!(out.paths(), vec!["style.css", "theme.json"]);
        assert!(
            String::from_utf8_lossy(out.get("style.css").unwrap()).contains("Theme Name: Deflated")
        );
    }

    #[test]
    fn rejects_non_zip_and_oversized_zip() {
        let err = extract_theme_zip(b"not a zip").unwrap_err();
        assert_eq!(err.status, 400);
        let big = vec![0u8; 51 * 1024 * 1024];
        let err = extract_theme_zip(&big).unwrap_err();
        assert!(err.message.contains("50 MB"), "{}", err.message);
    }

    #[test]
    fn accepts_zip_with_201_files() {
        let mut files: Vec<(String, Vec<u8>)> =
            vec![("style.css".to_string(), b"/* Theme Name: Big */".to_vec())];
        for i in 0..200 {
            files.push((
                format!("part-{i}.html"),
                b"<!-- wp:paragraph --><p>x</p><!-- /wp:paragraph -->".to_vec(),
            ));
        }
        let refs: Vec<(&str, &[u8])> = files
            .iter()
            .map(|(p, b)| (p.as_str(), b.as_slice()))
            .collect();
        let out = extract_theme_zip(&zip_of(&refs)).unwrap();
        assert_eq!(out.files.len(), 201);
    }

    #[test]
    fn rejects_zip_with_more_than_10000_files() {
        let mut files: Vec<(String, Vec<u8>)> =
            vec![("style.css".to_string(), b"/* Theme Name: Huge */".to_vec())];
        for i in 0..10000 {
            files.push((format!("part-{i}.html"), b"x".to_vec()));
        }
        let refs: Vec<(&str, &[u8])> = files
            .iter()
            .map(|(p, b)| (p.as_str(), b.as_slice()))
            .collect();
        let err = extract_theme_zip(&zip_of(&refs)).unwrap_err();
        assert!(err.message.contains("limit is 10000"), "{}", err.message);
    }

    #[test]
    fn rejects_path_traversal_entry() {
        let zip = zip_of(&[("../evil.txt", b"x"), ("style.css", b"/* Theme Name: X */")]);
        let err = extract_theme_zip(&zip).unwrap_err();
        assert!(err.message.contains("unsafe path"), "{}", err.message);
    }

    #[test]
    fn strips_single_top_level_folder() {
        let zip = zip_of(&[
            ("test-block/style.css", b"body { color: #111; }"),
            ("test-block/theme.json", b"{}"),
            (
                "test-block/templates/index.html",
                b"<!-- wp:paragraph --><p>x</p><!-- /wp:paragraph -->",
            ),
            ("test-block/parts/header.html", b"<!-- wp:site-title /-->"),
            (
                "test-block/parts/footer.html",
                b"<!-- wp:paragraph --><p>Footer</p><!-- /wp:paragraph -->",
            ),
        ]);
        let out = extract_theme_zip(&zip).unwrap();
        let mut paths = out.paths();
        paths.sort_unstable();
        assert_eq!(
            paths,
            vec![
                "parts/footer.html",
                "parts/header.html",
                "style.css",
                "templates/index.html",
                "theme.json"
            ]
        );
    }

    #[test]
    fn skips_hidden_files_and_warns_on_unsupported_extensions() {
        let zip = zip_of(&[
            ("style.css", b"/* Theme Name: X */"),
            (".DS_Store", b"junk"),
            ("notes.exe", b"junk"),
        ]);
        let out = extract_theme_zip(&zip).unwrap();
        assert_eq!(out.files.len(), 1);
        assert_eq!(out.warnings.len(), 1);
        assert!(out.warnings[0].contains("notes.exe"), "{}", out.warnings[0]);
    }

    #[test]
    fn rejects_zip_with_no_usable_files() {
        let zip = zip_of(&[("notes.exe", b"junk")]);
        let err = extract_theme_zip(&zip).unwrap_err();
        assert!(
            err.message.contains("no usable theme files"),
            "{}",
            err.message
        );
    }
}
