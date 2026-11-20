//! JSON-LD structured-data builders + a structural validator (P10).
//!
//! Builders return one JSON object (as a string, keys in schema.org
//! order) per type. The graph is assembled in `head.rs`; these
//! builders are pure string functions so they are unit-testable
//! without a database.
//!
//! Types shipped (doc §0.1 F): `WebSite` (+ `SearchAction`), `Article`,
//! `BreadcrumbList`, `Organization`, `Product`, `FAQPage`, `Course`.
//!
//! The validator ([`validate_json_ld`]) is structural, not Google's
//! Rich Results Test: it parses the JSON and checks `@context`,
//! `@type`, and the required fields per type. A clean result means
//! "well-formed and complete per schema.org", never "Google will show
//! a rich result" — that claim is never made anywhere.

/// A JSON string literal, exactly as `JSON.stringify` produces it.
pub fn json_str(s: &str) -> String {
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

/// Format a minor-unit price as a major-unit decimal string
/// ("329700" minor NGN → "3297.00"). Minor units are 1/100 of major
/// throughout the shop (see `wordless-commerce::money`).
pub fn price_decimal(price_minor: i64) -> String {
    let sign = if price_minor < 0 { "-" } else { "" };
    let abs = price_minor.unsigned_abs();
    format!("{sign}{}.{:02}", abs / 100, abs % 100)
}

/// `WebSite` with a `SearchAction` (the site search is `/?s=...`).
pub fn website_json_ld(site_name: &str, site_url: &str) -> String {
    format!(
        "{{\"@type\":\"WebSite\",\"name\":{},\"url\":{},\"potentialAction\":{{\"@type\":\"SearchAction\",\"target\":{{\"@type\":\"EntryPoint\",\"urlTemplate\":{}}},\"query-input\":\"required name=search_term_string\"}}}}",
        json_str(site_name),
        json_str(&format!("{site_url}/")),
        json_str(&format!("{site_url}/?s={{search_term_string}}"))
    )
}

/// `Organization` (name + url; logo only when one is configured).
pub fn organization_json_ld(site_name: &str, site_url: &str, logo_url: Option<&str>) -> String {
    let logo = match logo_url {
        Some(u) if !u.is_empty() => format!(",\"logo\":{}", json_str(u)),
        _ => String::new(),
    };
    format!(
        "{{\"@type\":\"Organization\",\"name\":{},\"url\":{}{}}}",
        json_str(site_name),
        json_str(&format!("{site_url}/")),
        logo
    )
}

/// `Product` with a single `Offer`. `in_stock` maps to
/// `https://schema.org/InStock` / `OutOfStock`; variable products
/// carry the parent's minimum price (the page shows "From …").
pub fn product_json_ld(
    name: &str,
    description: &str,
    url: &str,
    price_minor: i64,
    currency: &str,
    in_stock: bool,
    image_url: Option<&str>,
) -> String {
    let availability = if in_stock {
        "https://schema.org/InStock"
    } else {
        "https://schema.org/OutOfStock"
    };
    let image = match image_url {
        Some(u) if !u.is_empty() => format!(",\"image\":{}", json_str(u)),
        _ => String::new(),
    };
    let desc = if description.is_empty() {
        String::new()
    } else {
        format!(",\"description\":{}", json_str(description))
    };
    format!(
        "{{\"@type\":\"Product\",\"name\":{}{},\"url\":{}{},\"offers\":{{\"@type\":\"Offer\",\"price\":{},\"priceCurrency\":{},\"availability\":{}}}}}",
        json_str(name),
        image,
        json_str(url),
        desc,
        json_str(&price_decimal(price_minor)),
        json_str(currency),
        json_str(availability)
    )
}

/// `FAQPage` from question/answer pairs. `None` when empty — a page
/// with no FAQs carries no FAQ node.
pub fn faq_json_ld(faqs: &[(String, String)]) -> Option<String> {
    if faqs.is_empty() {
        return None;
    }
    let items: Vec<String> = faqs
        .iter()
        .map(|(q, a)| {
            format!(
                "{{\"@type\":\"Question\",\"name\":{},\"acceptedAnswer\":{{\"@type\":\"Answer\",\"text\":{}}}}}",
                json_str(q),
                json_str(a)
            )
        })
        .collect();
    Some(format!(
        "{{\"@type\":\"FAQPage\",\"mainEntity\":[{}]}}",
        items.join(",")
    ))
}

/// `Course` with a provider `Organization` and an optional
/// instructor (`Person`). `hasCourseInstance` is omitted: enrolment
/// is open and P9 has no scheduled instances to describe.
pub fn course_json_ld(
    title: &str,
    description: &str,
    url: &str,
    provider_name: &str,
    provider_url: &str,
    instructor_name: Option<&str>,
) -> String {
    let provider = format!(
        "{{\"@type\":\"Organization\",\"name\":{},\"url\":{}}}",
        json_str(provider_name),
        json_str(provider_url)
    );
    let instructor = match instructor_name {
        Some(n) if !n.is_empty() => format!(
            ",\"instructor\":{{\"@type\":\"Person\",\"name\":{}}}",
            json_str(n)
        ),
        _ => String::new(),
    };
    format!(
        "{{\"@type\":\"Course\",\"name\":{},\"description\":{},\"url\":{},\"provider\":{}{}}}",
        json_str(title),
        json_str(description),
        json_str(url),
        provider,
        instructor
    )
}

/// Structural validation of one JSON-LD document (object or
/// `@graph` array). Returns the list of problems; empty = valid.
///
/// Checks: valid JSON; `@context` is `https://schema.org` (or the
/// document is a graph whose nodes carry no conflicting context);
/// every node has a known `@type`; required fields per type:
/// - Article: `headline`
/// - BreadcrumbList: `itemListElement` with ≥ 1 `ListItem`
///   (`position`, `name`, `item`)
/// - FAQPage: `mainEntity` with ≥ 1 `Question` (`name`,
///   `acceptedAnswer.text`)
/// - Organization: `name`, `url`
/// - Product: `name`, `offers.price`, `offers.priceCurrency`
/// - Course: `name`, `description`, `provider.name`
/// - WebSite: `name`, `url` (and, when `potentialAction` is present,
///   its `target.urlTemplate` must contain `{search_term_string}`)
fn missing(value: &serde_json::Value, field: &str) -> bool {
    match value.get(field) {
        None => true,
        Some(serde_json::Value::String(s)) => s.is_empty(),
        Some(_) => false,
    }
}

fn validate_node(node: &serde_json::Value, problems: &mut Vec<String>) {
    let Some(ty) = node.get("@type").and_then(|t| t.as_str()) else {
        problems.push("A JSON-LD node is missing \"@type\".".to_string());
        return;
    };
    let path = format!("@type \"{ty}\"");
    match ty {
        "Article" => {
            if missing(node, "headline") {
                problems.push(format!("{path} is missing \"headline\"."));
            }
        }
        "BreadcrumbList" => {
            let items = node.get("itemListElement").and_then(|v| v.as_array());
            match items {
                None => problems.push(format!("{path} is missing \"itemListElement\".")),
                Some(list) if list.is_empty() => {
                    problems.push(format!("{path} has an empty \"itemListElement\"."))
                }
                Some(list) => {
                    for (i, item) in list.iter().enumerate() {
                        if missing(item, "name") || missing(item, "item") {
                            problems
                                .push(format!("{path} item {i} is missing \"name\" or \"item\"."));
                        }
                        if item.get("position").and_then(|p| p.as_i64()).is_none() {
                            problems.push(format!("{path} item {i} is missing \"position\"."));
                        }
                    }
                }
            }
        }
        "FAQPage" => {
            let items = node.get("mainEntity").and_then(|v| v.as_array());
            match items {
                None => problems.push(format!("{path} is missing \"mainEntity\".")),
                Some(list) if list.is_empty() => {
                    problems.push(format!("{path} has an empty \"mainEntity\"."))
                }
                Some(list) => {
                    for (i, q) in list.iter().enumerate() {
                        if missing(q, "name") {
                            problems.push(format!("{path} question {i} is missing \"name\"."));
                        }
                        let answer = q
                            .get("acceptedAnswer")
                            .and_then(|a| a.get("text"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("");
                        if answer.is_empty() {
                            problems.push(format!(
                                "{path} question {i} is missing \"acceptedAnswer.text\"."
                            ));
                        }
                    }
                }
            }
        }
        "Organization" => {
            if missing(node, "name") {
                problems.push(format!("{path} is missing \"name\"."));
            }
            if missing(node, "url") {
                problems.push(format!("{path} is missing \"url\"."));
            }
        }
        "Product" => {
            if missing(node, "name") {
                problems.push(format!("{path} is missing \"name\"."));
            }
            let offers = node.get("offers");
            match offers {
                None => problems.push(format!("{path} is missing \"offers\".")),
                Some(o) => {
                    if missing(o, "price") {
                        problems.push(format!("{path} is missing \"offers.price\"."));
                    }
                    if missing(o, "priceCurrency") {
                        problems.push(format!("{path} is missing \"offers.priceCurrency\"."));
                    }
                }
            }
        }
        "Course" => {
            if missing(node, "name") {
                problems.push(format!("{path} is missing \"name\"."));
            }
            if missing(node, "description") {
                problems.push(format!("{path} is missing \"description\"."));
            }
            match node.get("provider") {
                Some(p) if !missing(p, "name") => {}
                _ => problems.push(format!("{path} is missing \"provider.name\".")),
            }
        }
        "WebSite" => {
            if missing(node, "name") {
                problems.push(format!("{path} is missing \"name\"."));
            }
            if missing(node, "url") {
                problems.push(format!("{path} is missing \"url\"."));
            }
            if let Some(action) = node.get("potentialAction") {
                let template = action
                    .get("target")
                    .and_then(|t| t.get("urlTemplate"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                if !template.contains("{search_term_string}") {
                    problems.push(
                        "WebSite SearchAction \"target.urlTemplate\" must contain \"{search_term_string}\"."
                            .to_string(),
                    );
                }
            }
        }
        other => problems.push(format!("Unknown JSON-LD \"@type\": \"{other}\".")),
    }
}

/// Validate a JSON-LD document string (a single node or an
/// `@graph` document). Returns the problems found (empty = valid).
pub fn validate_json_ld(json: &str) -> Vec<String> {
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(json);
    let Ok(doc) = parsed else {
        return vec!["JSON-LD is not valid JSON.".to_string()];
    };
    let mut problems = Vec::new();
    if let Some(ctx) = doc.get("@context").and_then(|c| c.as_str()) {
        if ctx != "https://schema.org" {
            problems.push(format!(
                "\"@context\" is \"{ctx}\", expected \"https://schema.org\"."
            ));
        }
    }
    match doc.get("@graph") {
        Some(serde_json::Value::Array(nodes)) => {
            if nodes.is_empty() {
                problems.push("\"@graph\" is empty.".to_string());
            }
            for node in nodes {
                validate_node(node, &mut problems);
            }
        }
        Some(_) => problems.push("\"@graph\" must be an array.".to_string()),
        None => validate_node(&doc, &mut problems),
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn website_carries_search_action_with_the_site_search_url() {
        let ld = website_json_ld("Wolf Site", "https://example.test");
        assert!(ld.contains("\"@type\":\"WebSite\""));
        assert!(ld.contains("\"@type\":\"SearchAction\""));
        assert!(ld.contains("https://example.test/?s={search_term_string}"));
        assert!(validate_json_ld(&format!(
            "{{\"@context\":\"https://schema.org\",\"@graph\":[{}]}}",
            ld
        ))
        .is_empty());
    }

    #[test]
    fn organization_without_logo_has_no_logo_field() {
        let ld = organization_json_ld("Wolf Site", "https://example.test", None);
        assert!(!ld.contains("logo"));
        assert!(validate_json_ld(&ld).is_empty());
    }

    #[test]
    fn product_marks_stock_and_formats_minor_units() {
        let ld = product_json_ld(
            "Pack Hoodie",
            "A warm hoodie.",
            "https://example.test/shop/pack-hoodie",
            329700,
            "NGN",
            true,
            Some("https://example.test/media/hoodie.png"),
        );
        assert!(ld.contains("\"price\":\"3297.00\""));
        assert!(ld.contains("\"priceCurrency\":\"NGN\""));
        assert!(ld.contains("https://schema.org/InStock"));
        assert!(validate_json_ld(&ld).is_empty());
        let out = product_json_ld(
            "Gone",
            "",
            "https://example.test/shop/gone",
            0,
            "NGN",
            false,
            None,
        );
        assert!(out.contains("https://schema.org/OutOfStock"));
        assert!(out.contains("\"price\":\"0.00\""));
        assert!(validate_json_ld(&out).is_empty());
    }

    #[test]
    fn price_decimal_handles_cents_and_negatives() {
        assert_eq!(price_decimal(329700), "3297.00");
        assert_eq!(price_decimal(5), "0.05");
        assert_eq!(price_decimal(0), "0.00");
        assert_eq!(price_decimal(-150), "-1.50");
    }

    #[test]
    fn faq_is_none_when_empty_and_validates_when_present() {
        assert!(faq_json_ld(&[]).is_none());
        let ld = faq_json_ld(&[(
            "Is the pack real?".to_string(),
            "Yes, the pack is real.".to_string(),
        )])
        .unwrap();
        assert!(ld.contains("\"@type\":\"FAQPage\""));
        assert!(ld.contains("\"@type\":\"Question\""));
        assert!(validate_json_ld(&ld).is_empty());
    }

    #[test]
    fn course_carries_provider_and_instructor() {
        let ld = course_json_ld(
            "Rust 101",
            "Learn Rust.",
            "https://example.test/school/courses/rust-101",
            "Wolf Site",
            "https://example.test/",
            Some("Ada Lovelace"),
        );
        assert!(ld.contains("\"@type\":\"Course\""));
        assert!(ld.contains("\"provider\":{\"@type\":\"Organization\""));
        assert!(ld.contains("\"instructor\":{\"@type\":\"Person\",\"name\":\"Ada Lovelace\"}"));
        assert!(validate_json_ld(&ld).is_empty());
        let no_instructor = course_json_ld(
            "Rust 101",
            "Learn Rust.",
            "https://example.test/school/courses/rust-101",
            "Wolf Site",
            "https://example.test/",
            None,
        );
        assert!(!no_instructor.contains("instructor"));
        assert!(validate_json_ld(&no_instructor).is_empty());
    }

    #[test]
    fn validator_catches_missing_required_fields_and_bad_json() {
        assert_eq!(
            validate_json_ld("not json"),
            vec!["JSON-LD is not valid JSON.".to_string()]
        );
        let no_headline = validate_json_ld("{\"@type\":\"Article\"}");
        assert!(no_headline.iter().any(|p| p.contains("headline")));
        let bad_product = validate_json_ld("{\"@type\":\"Product\",\"name\":\"X\"}");
        assert!(bad_product.iter().any(|p| p.contains("offers")));
        let bad_site = validate_json_ld(
            "{\"@type\":\"WebSite\",\"name\":\"X\",\"url\":\"https://example.test/\",\"potentialAction\":{\"@type\":\"SearchAction\",\"target\":{\"@type\":\"EntryPoint\",\"urlTemplate\":\"https://example.test/?q={q}\"},\"query-input\":\"required name=q\"}}",
        );
        assert!(bad_site.iter().any(|p| p.contains("search_term_string")));
        let unknown = validate_json_ld("{\"@type\":\"Spaceship\",\"name\":\"X\"}");
        assert!(unknown.iter().any(|p| p.contains("Unknown JSON-LD")));
        let wrong_ctx = validate_json_ld(
            "{\"@context\":\"https://example.com\",\"@type\":\"Organization\",\"name\":\"X\",\"url\":\"https://example.test/\"}",
        );
        assert!(wrong_ctx.iter().any(|p| p.contains("@context")));
    }
}
