//! Drop visually hidden text and residual boilerplate from a parsed document.
//!
//! A fetched page is untrusted content. The cheapest place to hide an
//! instruction aimed at a model is text a person never sees: `display:none`,
//! the `hidden` attribute, `aria-hidden`, screen-reader-only class names,
//! zero-size or off-screen positioning. The leading extractors do not remove
//! this reliably — Readability checks only an element's own inline style and
//! never its ancestors — so it is done here, deliberately erring toward removal.
//! A false removal costs a paragraph; a false keep is an injection channel.
//!
//! Class/id semantics catch the rest: ads, sidebars, related rails and
//! share bars that survived the scoring pass.

use dom_query::Document;

/// Class/id tokens that mark boilerplate the scoring pass may have kept.
const BOILERPLATE_TOKENS: &[&str] = &[
    "advert",
    "ad-",
    "-ad",
    "ads-",
    "sponsor",
    "promo",
    "outbrain",
    "taboola",
    "related",
    "recommend",
    "share",
    "social",
    "breadcrumb",
    "pagination",
    "newsletter",
    "subscribe",
    "cookie",
    "consent",
    "paywall",
    "modal",
    "popup",
    "comment",
    "disqus",
];

/// Class tokens that mark visually hidden text.
const HIDDEN_TOKENS: &[&str] = &[
    "sr-only",
    "visually-hidden",
    "screen-reader",
    "screenreader",
    "hidden",
    "d-none",
    "offscreen",
];

/// Remove hidden and boilerplate subtrees from `document` in place.
pub(super) fn strip(document: &Document) {
    // Collect the nodes to drop first, then remove them in one selection:
    // removing during iteration would invalidate the walk.
    let to_drop: Vec<dom_query::NodeRef<'_>> = document
        .select("*")
        .nodes()
        .iter()
        .filter(|node| should_drop(node))
        .copied()
        .collect();
    if !to_drop.is_empty() {
        let selection: dom_query::Selection<'_> = to_drop.into();
        selection.remove();
    }
}

/// Whether a node is hidden or boilerplate by its own attributes.
fn should_drop(node: &dom_query::NodeRef<'_>) -> bool {
    if node.has_attr("hidden") {
        return true;
    }
    if node.attr("aria-hidden").as_deref() == Some("true") {
        return true;
    }
    // `role` values that are navigation or alerts, never article prose.
    if let Some(role) = node.attr("role") {
        let role = role.to_ascii_lowercase();
        if matches!(
            role.as_ref(),
            "navigation" | "menu" | "menubar" | "complementary" | "alert" | "dialog" | "banner"
        ) {
            return true;
        }
    }
    if let Some(style) = node.attr("style") {
        if style_hides(&style) {
            return true;
        }
    }
    if let Some(class) = node.class() {
        let class = class.to_ascii_lowercase();
        if HIDDEN_TOKENS.iter().any(|t| class.contains(t))
            || BOILERPLATE_TOKENS.iter().any(|t| class.contains(t))
        {
            return true;
        }
    }
    if let Some(id) = node.attr("id") {
        let id = id.to_ascii_lowercase();
        if HIDDEN_TOKENS.iter().any(|t| id.contains(t))
            || BOILERPLATE_TOKENS.iter().any(|t| id.contains(t))
        {
            return true;
        }
    }
    false
}

/// Whether an inline `style` hides the element or pushes it off-screen.
fn style_hides(style: &str) -> bool {
    let compact: String = style.chars().filter(|c| !c.is_whitespace()).collect();
    let compact = compact.to_ascii_lowercase();
    if compact.contains("display:none") || compact.contains("visibility:hidden") {
        return true;
    }
    if compact.contains("opacity:0") && !compact.contains("opacity:0.") {
        return true;
    }
    // Zero font size, or off-screen positioning.
    if compact.contains("font-size:0") || compact.contains("text-indent:-") {
        return true;
    }
    if compact.contains("left:-") || compact.contains("top:-") {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visible_text(html: &str) -> String {
        let document = Document::from(html);
        strip(&document);
        document
            .text()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn hidden_elements_are_removed() {
        let html = "<div><p>keep this</p>\
            <div style=\"display:none\">drop injection</div>\
            <span aria-hidden=\"true\">drop aria</span>\
            <span hidden>drop hidden attr</span>\
            <span class=\"sr-only\">drop sr-only</span></div>";
        let text = visible_text(html);
        assert!(text.contains("keep this"));
        for gone in ["injection", "aria", "hidden attr", "sr-only"] {
            assert!(!text.contains(gone), "{gone} should be gone, got {text}");
        }
    }

    #[test]
    fn boilerplate_classes_are_removed() {
        let html = "<main><p>the article</p>\
            <div class=\"advert\">buy now</div>\
            <aside class=\"related-posts\">you may like</aside>\
            <div id=\"newsletter-signup\">subscribe</div></main>";
        let text = visible_text(html);
        assert!(text.contains("the article"));
        for gone in ["buy now", "you may like", "subscribe"] {
            assert!(!text.contains(gone), "{gone} should be gone, got {text}");
        }
    }

    #[test]
    fn offscreen_positioning_is_removed() {
        let text =
            visible_text("<p>ok</p><div style=\"position:absolute;left:-9999px\">evil</div>");
        assert!(text.contains("ok"));
        assert!(!text.contains("evil"));
    }
}
