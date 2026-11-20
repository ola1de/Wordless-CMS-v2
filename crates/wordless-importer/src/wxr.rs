//! WXR (WordPress eXtended RSS) parser (port of v1
//! `src/modules/importer/wxr-parser.ts`).
//!
//! The v1 parser uses `fast-xml-parser`; this crate cannot —
//! `quick-xml` (the doc's §2.2 choice) is not available offline in
//! this VM, and no XML crate is in `Cargo.lock`. This module therefore
//! contains a small hand-rolled XML reader for the WXR subset only
//! (`rss`/`channel`/`item`, titles, `content:encoded`,
//! `wp:post_type`/`post_status`/`post_name`/`post_date`, category/tag
//! terms, `wp:comment`). It is a real tokenizer — elements,
//! attributes, CDATA, comments, processing instructions, and
//! named/numeric entities — never regex. It is NOT a general XML
//! parser (no DTD, no namespaces beyond the `prefix:name` spelling,
//! no mixed-content fidelity beyond concatenated text) and makes no
//! claim to be one. All importing decisions (status mapping, slug
//! dedup, forced password reset) live in `service.rs`.

use thiserror::Error;

#[derive(Debug, Error)]
#[error("{0}")]
pub struct XmlError(pub String);

/// One parsed element: name (with its `prefix:` intact), attributes,
/// child elements, and concatenated character data (plain text and
/// CDATA, entity-decoded).
#[derive(Debug, Clone, Default)]
pub struct XmlNode {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<XmlNode>,
    pub text: String,
}

impl XmlNode {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn child(&self, name: &str) -> Option<&XmlNode> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn children_named(&self, name: &str) -> Vec<&XmlNode> {
        self.children.iter().filter(|c| c.name == name).collect()
    }

    /// Text of a child element, trimmed (v1's parser trims values).
    /// Empty when the child is absent.
    pub fn child_text(&self, name: &str) -> String {
        self.child(name)
            .map(|c| c.text.trim().to_string())
            .unwrap_or_default()
    }
}

const MAX_DEPTH: usize = 100;

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(input: &'a str) -> Self {
        // Skip a UTF-8 byte-order mark when present (some exporters emit one).
        let bytes = input.as_bytes();
        let pos = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
            3
        } else {
            0
        };
        Self { bytes, pos }
    }

    fn err(&self, msg: impl Into<String>) -> XmlError {
        XmlError(format!("{} (offset {})", msg.into(), self.pos))
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn starts_with(&self, pat: &str) -> bool {
        self.bytes[self.pos..].starts_with(pat.as_bytes())
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if (b as char).is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    /// Skip one prolog/comment/processing-instruction/doctype chunk.
    /// Returns true when something was skipped.
    fn skip_misc(&mut self) -> Result<bool, XmlError> {
        if self.starts_with("<!--") {
            let end = find_subslice(&self.bytes[self.pos..], b"-->")
                .ok_or_else(|| self.err("Unterminated XML comment"))?;
            self.pos += end + 3;
            Ok(true)
        } else if self.starts_with("<?") {
            let end = find_subslice(&self.bytes[self.pos..], b"?>")
                .ok_or_else(|| self.err("Unterminated processing instruction"))?;
            self.pos += end + 2;
            Ok(true)
        } else if self.starts_with("<!DOCTYPE") || self.starts_with("<!doctype") {
            // Skip to the closing '>' (doctype internals are not used
            // by WXR; a '[' subset would break this — WXR has none).
            let end = find_byte(&self.bytes[self.pos..], b'>')
                .ok_or_else(|| self.err("Unterminated DOCTYPE"))?;
            self.pos += end + 1;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn parse_document(&mut self) -> Result<XmlNode, XmlError> {
        loop {
            self.skip_ws();
            if self.skip_misc()? {
                continue;
            }
            break;
        }
        if self.peek() != Some(b'<') {
            return Err(self.err("WXR file is not valid XML: no root element"));
        }
        let root = self.parse_element(0)?;
        loop {
            self.skip_ws();
            if self.skip_misc()? {
                continue;
            }
            break;
        }
        if self.pos != self.bytes.len() {
            return Err(self.err("Trailing data after the root element"));
        }
        Ok(root)
    }

    fn parse_element(&mut self, depth: usize) -> Result<XmlNode, XmlError> {
        if depth > MAX_DEPTH {
            return Err(self.err("XML is nested too deeply"));
        }
        if self.peek() != Some(b'<') {
            return Err(self.err("Expected '<'"));
        }
        self.pos += 1;
        let name = self.read_name()?;
        if name.is_empty() {
            return Err(self.err("Empty element name"));
        }
        let mut node = XmlNode {
            name,
            ..XmlNode::default()
        };
        // Attributes / self-closing / open tag.
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'/') => {
                    self.pos += 1;
                    if self.peek() != Some(b'>') {
                        return Err(self.err("Expected '>' after '/'"));
                    }
                    self.pos += 1;
                    return Ok(node);
                }
                Some(b'>') => {
                    self.pos += 1;
                    break;
                }
                Some(_) => {
                    let attr_name = self.read_name()?;
                    if attr_name.is_empty() {
                        return Err(self.err("Expected attribute name"));
                    }
                    self.skip_ws();
                    if self.peek() != Some(b'=') {
                        return Err(self.err("Expected '=' after attribute name"));
                    }
                    self.pos += 1;
                    self.skip_ws();
                    let quote = self
                        .peek()
                        .ok_or_else(|| self.err("Unterminated attribute"))?;
                    if quote != b'"' && quote != b'\'' {
                        return Err(self.err("Attribute value must be quoted"));
                    }
                    self.pos += 1;
                    let start = self.pos;
                    let end = find_byte(&self.bytes[self.pos..], quote)
                        .ok_or_else(|| self.err("Unterminated attribute value"))?;
                    let raw = String::from_utf8_lossy(&self.bytes[start..start + end]).into_owned();
                    self.pos += end + 1;
                    node.attrs.push((attr_name, decode_entities(&raw)));
                }
                None => return Err(self.err("Unexpected end of input inside a tag")),
            }
        }
        // Content: text, CDATA, comments, PIs, child elements, close tag.
        loop {
            if self.pos >= self.bytes.len() {
                return Err(self.err(format!("Unterminated element <{}>", node.name)));
            }
            if self.starts_with("</") {
                self.pos += 2;
                let close_name = self.read_name()?;
                self.skip_ws();
                if self.peek() != Some(b'>') {
                    return Err(self.err("Expected '>' in a closing tag"));
                }
                self.pos += 1;
                if close_name != node.name {
                    return Err(self.err(format!(
                        "Mismatched closing tag </{close_name}> for <{}>",
                        node.name
                    )));
                }
                return Ok(node);
            }
            if self.starts_with("<!--") || self.starts_with("<?") {
                self.skip_misc()?;
                continue;
            }
            if self.starts_with("<![CDATA[") {
                self.pos += "<![CDATA[".len();
                let end = find_subslice(&self.bytes[self.pos..], b"]]>")
                    .ok_or_else(|| self.err("Unterminated CDATA section"))?;
                let raw =
                    String::from_utf8_lossy(&self.bytes[self.pos..self.pos + end]).into_owned();
                node.text.push_str(&raw);
                self.pos += end + 3;
                continue;
            }
            if self.peek() == Some(b'<') {
                let child = self.parse_element(depth + 1)?;
                node.children.push(child);
                continue;
            }
            // Character data up to the next '<'.
            let start = self.pos;
            let end =
                find_byte(&self.bytes[self.pos..], b'<').unwrap_or(self.bytes.len() - self.pos);
            let raw = String::from_utf8_lossy(&self.bytes[start..start + end]).into_owned();
            node.text.push_str(&decode_entities(&raw));
            self.pos += end;
        }
    }

    /// Read an element/attribute name: up to whitespace, `/`, `>`, or `=`.
    fn read_name(&mut self) -> Result<String, XmlError> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            let c = b as char;
            if c.is_ascii_whitespace() || c == '/' || c == '>' || c == '=' {
                break;
            }
            self.pos += 1;
        }
        Ok(String::from_utf8_lossy(&self.bytes[start..self.pos]).into_owned())
    }
}

fn find_byte(haystack: &[u8], needle: u8) -> Option<usize> {
    haystack.iter().position(|&b| b == needle)
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Decode `&amp;` `&lt;` `&gt;` `&quot;` `&apos;` and numeric
/// character references. Unknown entities are left literal (lenient:
/// WordPress content sometimes carries `&nbsp;` and friends in text).
pub fn decode_entities(input: &str) -> String {
    if !input.contains('&') {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        match after.find(';') {
            Some(semi) if semi <= 10 => {
                let entity = &after[..semi];
                let decoded: Option<String> = match entity {
                    "amp" => Some("&".to_string()),
                    "lt" => Some("<".to_string()),
                    "gt" => Some(">".to_string()),
                    "quot" => Some("\"".to_string()),
                    "apos" => Some("'".to_string()),
                    _ => {
                        if let Some(num) = entity
                            .strip_prefix("#x")
                            .or_else(|| entity.strip_prefix("#X"))
                        {
                            u32::from_str_radix(num, 16)
                                .ok()
                                .and_then(char::from_u32)
                                .map(|c| c.to_string())
                        } else if let Some(num) = entity.strip_prefix('#') {
                            num.parse::<u32>()
                                .ok()
                                .and_then(char::from_u32)
                                .map(|c| c.to_string())
                        } else {
                            None
                        }
                    }
                };
                match decoded {
                    Some(d) => out.push_str(&d),
                    None => {
                        out.push('&');
                        out.push_str(entity);
                        out.push(';');
                    }
                }
                rest = &after[semi + 1..];
            }
            _ => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------- WXR types

#[derive(Debug, Clone, Default)]
pub struct WxrAuthor {
    pub id: i64,
    pub login: String,
    pub email: String,
    pub display_name: String,
    pub first_name: String,
    pub last_name: String,
}

#[derive(Debug, Clone, Default)]
pub struct WxrTermDef {
    pub id: i64,
    pub taxonomy: String,
    pub slug: String,
    pub name: String,
    /// Parent category nicename (slug) — WXR gives a slug, not an ID.
    pub parent: String,
    pub description: String,
}

#[derive(Debug, Clone, Default)]
pub struct WxrMeta {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Default)]
pub struct WxrComment {
    pub id: i64,
    pub author: String,
    pub author_email: String,
    pub author_url: String,
    pub author_ip: String,
    pub date: String,
    pub date_gmt: String,
    pub content: String,
    pub approved: String,
    pub comment_type: String,
    pub parent: i64,
    pub user_id: i64,
    pub meta: Vec<WxrMeta>,
}

#[derive(Debug, Clone, Default)]
pub struct WxrItem {
    pub title: String,
    pub link: String,
    pub pub_date: String,
    pub creator: String,
    pub guid: String,
    pub description: String,
    pub content: String,
    pub excerpt: String,
    pub post_id: i64,
    pub post_date: String,
    pub post_date_gmt: String,
    pub post_name: String,
    pub status: String,
    pub post_type: String,
    pub post_parent: i64,
    pub menu_order: i64,
    pub post_password: String,
    pub is_sticky: bool,
    pub comment_status: String,
    pub ping_status: String,
    pub categories: Vec<WxrItemCategory>,
    pub meta: Vec<WxrMeta>,
    pub comments: Vec<WxrComment>,
}

#[derive(Debug, Clone, Default)]
pub struct WxrItemCategory {
    pub domain: String,
    pub nicename: String,
    pub name: String,
}

#[derive(Debug, Clone, Default)]
pub struct WxrExport {
    pub title: String,
    pub link: String,
    pub authors: Vec<WxrAuthor>,
    pub terms: Vec<WxrTermDef>,
    pub items: Vec<WxrItem>,
}

fn num(node: &XmlNode, name: &str) -> i64 {
    node.child_text(name).parse::<i64>().unwrap_or(0)
}

fn parse_meta(nodes: Vec<&XmlNode>) -> Vec<WxrMeta> {
    nodes
        .iter()
        .map(|m| WxrMeta {
            key: m.child_text("wp:meta_key"),
            value: m.child_text("wp:meta_value"),
        })
        .collect()
}

fn parse_comment(raw: &XmlNode) -> WxrComment {
    WxrComment {
        id: num(raw, "wp:comment_id"),
        author: raw.child_text("wp:comment_author"),
        author_email: raw.child_text("wp:comment_author_email"),
        author_url: raw.child_text("wp:comment_author_url"),
        author_ip: raw.child_text("wp:comment_author_IP"),
        date: raw.child_text("wp:comment_date"),
        date_gmt: raw.child_text("wp:comment_date_gmt"),
        content: raw.child_text("wp:comment_content"),
        approved: raw.child_text("wp:comment_approved"),
        comment_type: raw.child_text("wp:comment_type"),
        parent: num(raw, "wp:comment_parent"),
        user_id: num(raw, "wp:comment_user_id"),
        meta: parse_meta(raw.children_named("wp:commentmeta")),
    }
}

/// Parse a WXR export document into the typed structure v1's
/// `wxr-parser.ts` returns. Errors when the XML is not well-formed.
pub fn parse_wxr(xml: &str) -> Result<WxrExport, XmlError> {
    let mut reader = Reader::new(xml);
    let parsed = reader
        .parse_document()
        .map_err(|e| XmlError(format!("WXR file is not valid XML: {e}")))?;
    let rss = if parsed.name == "rss" {
        parsed
    } else {
        return Err(XmlError(
            "WXR file is not valid XML: root element is not <rss>.".to_string(),
        ));
    };
    let channel = rss.child("channel").cloned().unwrap_or_default();

    let authors = channel
        .children_named("wp:author")
        .iter()
        .map(|a| WxrAuthor {
            id: num(a, "wp:author_id"),
            login: a.child_text("wp:author_login"),
            email: a.child_text("wp:author_email"),
            display_name: a.child_text("wp:author_display_name"),
            first_name: a.child_text("wp:author_first_name"),
            last_name: a.child_text("wp:author_last_name"),
        })
        .collect();

    let mut terms: Vec<WxrTermDef> = Vec::new();
    for c in channel.children_named("wp:category") {
        terms.push(WxrTermDef {
            id: num(c, "wp:term_id"),
            taxonomy: "category".to_string(),
            slug: c.child_text("wp:category_nicename"),
            name: c.child_text("wp:cat_name"),
            parent: c.child_text("wp:category_parent"),
            description: c.child_text("wp:category_description"),
        });
    }
    for t in channel.children_named("wp:tag") {
        terms.push(WxrTermDef {
            id: num(t, "wp:term_id"),
            taxonomy: "post_tag".to_string(),
            slug: t.child_text("wp:tag_slug"),
            name: t.child_text("wp:tag_name"),
            parent: String::new(),
            description: t.child_text("wp:tag_description"),
        });
    }
    for t in channel.children_named("wp:term") {
        terms.push(WxrTermDef {
            id: num(t, "wp:term_id"),
            taxonomy: t.child_text("wp:term_taxonomy"),
            slug: t.child_text("wp:term_slug"),
            name: t.child_text("wp:term_name"),
            parent: String::new(),
            description: t.child_text("wp:term_description"),
        });
    }

    let items = channel
        .children_named("item")
        .iter()
        .map(|it| {
            let categories = it
                .children_named("category")
                .iter()
                .map(|c| WxrItemCategory {
                    domain: c.attr("domain").unwrap_or("category").to_string(),
                    nicename: c.attr("nicename").unwrap_or("").to_string(),
                    name: c.text.trim().to_string(),
                })
                .collect();
            WxrItem {
                title: it.child_text("title"),
                link: it.child_text("link"),
                pub_date: it.child_text("pubDate"),
                creator: it.child_text("dc:creator"),
                guid: it.child_text("guid"),
                description: it.child_text("description"),
                content: it.child_text("content:encoded"),
                excerpt: it.child_text("excerpt:encoded"),
                post_id: num(it, "wp:post_id"),
                post_date: it.child_text("wp:post_date"),
                post_date_gmt: it.child_text("wp:post_date_gmt"),
                post_name: it.child_text("wp:post_name"),
                status: it.child_text("wp:post_status"),
                post_type: it.child_text("wp:post_type"),
                post_parent: num(it, "wp:post_parent"),
                menu_order: num(it, "wp:menu_order"),
                post_password: it.child_text("wp:post_password"),
                is_sticky: it.child_text("wp:is_sticky") == "1",
                comment_status: it.child_text("wp:comment_status"),
                ping_status: it.child_text("wp:ping_status"),
                categories,
                meta: parse_meta(it.children_named("wp:postmeta")),
                comments: it
                    .children_named("wp:comment")
                    .iter()
                    .map(|c| parse_comment(c))
                    .collect(),
            }
        })
        .collect();

    Ok(WxrExport {
        title: channel.child_text("title"),
        link: channel.child_text("link"),
        authors,
        terms,
        items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str =
        include_str!("../../../../wordless-cms/test/fixtures/wordpress/wordpress-export.xml");

    #[test]
    fn parses_the_fixture_export() {
        let parsed = parse_wxr(FIXTURE).unwrap();
        assert_eq!(parsed.authors.len(), 2);
        assert_eq!(parsed.terms.len(), 4);
        assert_eq!(parsed.items.len(), 4);
        assert!(parsed.items.iter().any(|i| i.post_type == "revision"));
        let post = parsed.items.iter().find(|i| i.post_id == 10).unwrap();
        assert!(post.content.contains("<!-- wp:paragraph -->"));
        assert_eq!(post.comments.len(), 2);
        assert!(post.categories.iter().any(|c| c.nicename == "news"));
        assert_eq!(parsed.title, "Fixture Site");
    }

    #[test]
    fn rejects_malformed_xml() {
        assert!(parse_wxr("<rss><channel><item>").is_err());
        assert!(parse_wxr("not xml at all").is_err());
        assert!(parse_wxr("<rss><channel></item></channel></rss>").is_err());
    }

    #[test]
    fn handles_cdata_entities_and_self_closing_tags() {
        let doc = parse_wxr(
            r#"<?xml version="1.0"?><rss><channel><title>A &amp; B &#65;</title>
            <wp:author><wp:author_id>1</wp:author_id><wp:author_login>x</wp:author_login>
            <wp:author_email/><wp:author_display_name><![CDATA[Ex & <tag>]]></wp:author_display_name>
            <wp:author_first_name/><wp:author_last_name/></wp:author>
            <item><title>t</title><link/><pubDate/><dc:creator>x</dc:creator><guid/><description/>
            <content:encoded><![CDATA[<p>hi</p>]]></content:encoded><excerpt:encoded/>
            <wp:post_id>1</wp:post_id><wp:post_date>2026-01-01 00:00:00</wp:post_date>
            <wp:post_date_gmt>2026-01-01 00:00:00</wp:post_date_gmt><wp:post_name>t</wp:post_name>
            <wp:post_status>publish</wp:post_status><wp:post_type>post</wp:post_type>
            <wp:post_parent>0</wp:post_parent><wp:menu_order>0</wp:menu_order><wp:post_password/>
            <wp:is_sticky>0</wp:is_sticky><wp:comment_status>open</wp:comment_status>
            <wp:ping_status>open</wp:ping_status></item></channel></rss>"#,
        )
        .unwrap();
        assert_eq!(doc.title, "A & B A");
        assert_eq!(doc.authors[0].display_name, "Ex & <tag>");
        assert_eq!(doc.items[0].content, "<p>hi</p>");
    }
}
