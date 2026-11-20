//! HTML escaping and defence-in-depth sanitising (moved from
//! `wordless-render` blocks in P3 so `wordless-elementor` can use
//! them without depending on the whole render crate — the render
//! crate depends on elementor for post-content rendering, so the
//! escapers must live below both).
//!
//! Port of v1 `escapeHtml` (`src/modules/render/theme.ts`) and the
//! best-effort strip in `src/modules/render/blocks.ts`. This is NOT
//! a full HTML sanitizer (same caveat as v1).

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

pub fn strip_dangerous_html(html: &str) -> String {
    let no_script = strip_script_elements(html);
    let no_handlers = strip_event_handlers(&no_script);
    strip_javascript_hrefs(&no_handlers)
}

fn strip_script_elements(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    let mut rest_lower = lower.as_str();
    loop {
        let Some(idx) = find_script_open(rest_lower) else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..idx]);
        let after_open = &rest[idx..];
        let Some(gt) = after_open.find('>') else {
            break;
        };
        let body_start = idx + gt + 1;
        let tail_lower = &rest_lower[body_start..];
        let Some(close_rel) = tail_lower.find("</script") else {
            // Unterminated script: v1's regex needs a close tag, so an
            // unterminated <script> is left as-is by v1. Keep the rest.
            out.push_str(&rest[idx..]);
            break;
        };
        let close_start = body_start + close_rel;
        let after_close = &rest[close_start..];
        let Some(close_gt) = after_close.find('>') else {
            break;
        };
        let next = close_start + close_gt + 1;
        rest = &rest[next..];
        rest_lower = &rest_lower[next..];
    }
    out
}

/// Index of `<script` followed by whitespace, `>` or `/` (input already lowercased).
fn find_script_open(lower: &str) -> Option<usize> {
    let mut from = 0;
    loop {
        let idx = lower[from..].find("<script")? + from;
        let after = lower.as_bytes().get(idx + 7).copied();
        match after {
            Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') | Some(b'>') | Some(b'/') => {
                return Some(idx)
            }
            _ => from = idx + 7,
        }
    }
}

/// Remove ` on<name>="..."` / `'...'` / bare-value attributes (case-insensitive `on*`).
fn strip_event_handlers(html: &str) -> String {
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            let mut j = i + 1;
            if j + 1 < bytes.len()
                && (bytes[j] == b'o' || bytes[j] == b'O')
                && (bytes[j + 1] == b'n' || bytes[j + 1] == b'N')
            {
                j += 2;
                let name_start = j;
                while j < bytes.len()
                    && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'-')
                {
                    j += 1;
                }
                if j > name_start {
                    let mut k = j;
                    while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                        k += 1;
                    }
                    if k < bytes.len() && bytes[k] == b'=' {
                        k += 1;
                        while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                            k += 1;
                        }
                        if k < bytes.len() && (bytes[k] == b'"' || bytes[k] == b'\'') {
                            let quote = bytes[k];
                            let mut end = k + 1;
                            while end < bytes.len() && bytes[end] != quote {
                                end += 1;
                            }
                            i = if end < bytes.len() { end + 1 } else { end };
                            continue;
                        }
                        let mut end = k;
                        while end < bytes.len()
                            && !bytes[end].is_ascii_whitespace()
                            && bytes[end] != b'>'
                        {
                            end += 1;
                        }
                        i = end;
                        continue;
                    }
                }
            }
        }
        let ch_len = utf8_len(bytes[i]);
        let end = (i + ch_len).min(bytes.len());
        out.push_str(&html[i..end]);
        i = end;
    }
    out
}

fn utf8_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b >> 5 == 0b110 {
        2
    } else if b >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

/// Replace `href="javascript:..."` / `href='javascript:...'` with `href="#"`.
/// v1 only matches the quoted forms (with optional whitespace around `=`).
fn strip_javascript_hrefs(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while i < html.len() {
        let Some(rel) = lower[i..].find("href") else {
            out.push_str(&html[i..]);
            break;
        };
        let href_idx = i + rel;
        let mut j = href_idx + 4;
        while j < html.len() && html.as_bytes()[j].is_ascii_whitespace() {
            j += 1;
        }
        if j < html.len() && html.as_bytes()[j] == b'=' {
            j += 1;
            while j < html.len() && html.as_bytes()[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < html.len() && (html.as_bytes()[j] == b'"' || html.as_bytes()[j] == b'\'') {
                let quote = html.as_bytes()[j];
                let val_start = j + 1;
                if lower[val_start..].starts_with("javascript:") {
                    let mut end = val_start;
                    while end < html.len() && html.as_bytes()[end] != quote {
                        end += 1;
                    }
                    out.push_str(&html[i..=j]);
                    out.push('#');
                    if end < html.len() {
                        out.push(quote as char);
                        i = end + 1;
                    } else {
                        i = end;
                    }
                    continue;
                }
            }
        }
        out.push_str(&html[i..href_idx + 4]);
        i = href_idx + 4;
    }
    out
}
