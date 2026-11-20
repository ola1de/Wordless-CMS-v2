//! Social sharing (port of v1 `social.ts`) — plain share links, no
//! API calls.
//!
//! Share links are just URLs (X, Facebook, WhatsApp, Telegram) that
//! open the provider's share sheet with the page URL + title. There
//! is no share counting, no API key, and no fake "12 shares"
//! anywhere — share counts are not measured by Wordless at all.

use serde::Serialize;

use crate::util::percent_encode;

/// Share links for one page (v1 `ShareLinks`).
#[derive(Debug, Clone, Serialize)]
pub struct ShareLinks {
    pub x: String,
    pub facebook: String,
    pub whatsapp: String,
    pub telegram: String,
}

/// Build the four share links for a page URL + title. Pure function.
pub fn share_links(page_url: &str, title: &str) -> ShareLinks {
    let url = percent_encode(page_url);
    let text = percent_encode(title);
    ShareLinks {
        x: format!("https://twitter.com/intent/tweet?text={text}&url={url}"),
        facebook: format!("https://www.facebook.com/sharer/sharer.php?u={url}"),
        whatsapp: format!("https://wa.me/?text={text}%20{url}"),
        telegram: format!("https://t.me/share/url?url={url}&text={text}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_share_links_for_the_four_networks() {
        let links = share_links("https://example.com/hello-world", "Hello World");
        assert_eq!(
            links.x,
            "https://twitter.com/intent/tweet?text=Hello%20World&url=https%3A%2F%2Fexample.com%2Fhello-world"
        );
        assert_eq!(
            links.facebook,
            "https://www.facebook.com/sharer/sharer.php?u=https%3A%2F%2Fexample.com%2Fhello-world"
        );
        assert_eq!(
            links.whatsapp,
            "https://wa.me/?text=Hello%20World%20https%3A%2F%2Fexample.com%2Fhello-world"
        );
        assert_eq!(
            links.telegram,
            "https://t.me/share/url?url=https%3A%2F%2Fexample.com%2Fhello-world&text=Hello%20World"
        );
    }
}
