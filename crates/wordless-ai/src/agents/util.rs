//! Small pure helpers shared by the deterministic agents: heading
//! counting and WCAG contrast math. No regex crate is used anywhere
//! in this crate. (Money formatting lives in `wordless-commerce` —
//! the agents use that crate's `format_money`, not a local copy.)
//!
//! `count_h1` counts `<h1` headings (case-insensitive, `<h1`
//! followed by whitespace or `>`), mirroring v1's `/<h1[\s>]/gi`
//! match.

/// Count `<h1` headings (case-insensitive, `<h1` followed by
/// whitespace or `>`), mirroring v1's `/<h1[\s>]/gi` match.
pub fn count_h1(content: &str) -> usize {
    let lower = content.to_lowercase();
    let bytes = lower.as_bytes();
    let mut count = 0;
    let mut i = 0;
    while i + 3 < bytes.len() {
        if &bytes[i..i + 3] == b"<h1" {
            let next = bytes[i + 3];
            if next == b'>' || next.is_ascii_whitespace() {
                count += 1;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    count
}

/// Relative luminance of a `#rrggbb` colour (WCAG). Port of v1
/// `luminance` in `analysis.ts`.
pub fn luminance(hex: &str) -> Option<f64> {
    let hex = hex.trim();
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |start: usize| -> f64 {
        let v = u8::from_str_radix(&hex[start..start + 2], 16).unwrap_or(0) as f64 / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    Some(0.2126 * channel(0) + 0.7152 * channel(2) + 0.0722 * channel(4))
}

/// WCAG contrast ratio between two `#rrggbb` colours. Port of v1
/// `contrastRatio` in `analysis.ts`.
pub fn contrast_ratio(a: &str, b: &str) -> Option<f64> {
    let la = luminance(a)?;
    let lb = luminance(b)?;
    let hi = la.max(lb);
    let lo = la.min(lb);
    Some((hi + 0.05) / (lo + 0.05))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_black_on_white_is_21() {
        let ratio = contrast_ratio("#000000", "#ffffff").unwrap();
        assert!((ratio - 21.0).abs() < 0.01);
    }

    #[test]
    fn h1_counting_is_case_insensitive() {
        assert_eq!(count_h1("<h1>A</h1><H1>B</H1><h2>C</h2>"), 2);
        assert_eq!(count_h1("no headings here"), 0);
    }
}
