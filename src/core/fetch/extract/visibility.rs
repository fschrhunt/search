//! Drop visually hidden text and boilerplate containers from a parsed document.
//!
//! A fetched page is untrusted content. The cheapest place to hide an
//! instruction aimed at a model is text a person never sees: `display:none`,
//! the `hidden` attribute, `aria-hidden`, screen-reader-only classes, zero-size
//! or far off-screen positioning. Readability checks only an element's own
//! inline style, so hidden text is removed here first.
//!
//! Matching is exact, never substring: a class is a list of whole tokens, and
//! boilerplate words must be a whole segment of a token (`ad-slot`, `relatedPosts`),
//! so `overflow-hidden`, `lead-paragraph` or `margin-top:-4px` keep their
//! content. Utility classes that hide an element only at some screen widths
//! (`hidden md:block`, `d-none d-lg-flex`) leave it visible. The document
//! skeleton (`html`, `head`, `body`, `main`) is never removed, and a
//! boilerplate-looking container that holds the `article` or `main` is kept.

use dom_query::{Document, NodeRef};

/// Whole class tokens that hide an element at every screen width.
const HIDDEN_CLASSES: &[&str] = &[
    "sr-only",
    "visually-hidden",
    "visuallyhidden",
    "screen-reader-text",
    "screen-reader-only",
    "element-invisible",
    "offscreen",
    "hidden",
    "is-hidden",
    "hide",
    "d-none",
    "invisible",
];

/// Class/id segments that mark boilerplate the scoring pass may keep.
const BOILERPLATE_SEGMENTS: &[&str] = &[
    "ad",
    "ads",
    "adsbygoogle",
    "advert",
    "adverts",
    "advertisement",
    "advertising",
    "sponsor",
    "sponsored",
    "promo",
    "promoted",
    "outbrain",
    "taboola",
    "related",
    "recommended",
    "recommendations",
    "share",
    "sharing",
    "social",
    "breadcrumb",
    "breadcrumbs",
    "pagination",
    "newsletter",
    "subscribe",
    "subscription",
    "cookie",
    "cookies",
    "consent",
    "gdpr",
    "paywall",
    "modal",
    "popup",
    "disqus",
];

/// ARIA roles that are navigation, landmarks or interruptions, never prose.
const BOILERPLATE_ROLES: &[&str] = &[
    "navigation",
    "menu",
    "menubar",
    "complementary",
    "dialog",
    "alertdialog",
    "banner",
];

/// Elements that carry no readable prose for a model. Navigation is included:
/// Readability keeps `<nav>` on pages it scores as articles, such as listings.
const NON_TEXT: &str = "style, template, noscript, svg, canvas, iframe, object, embed, nav";

/// Remove hidden and boilerplate subtrees, and elements that carry no prose,
/// from `document` in place. Scripts stay: Readability reads JSON-LD metadata
/// from them and removes them itself; see [`remove_code`].
pub(super) fn strip(document: &Document) {
    document.select(NON_TEXT).remove();
    // Collect first, then remove in one selection: removing during the walk
    // would invalidate it.
    let to_drop: Vec<NodeRef<'_>> = document
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

/// Remove scripts, for readings of the whole document that bypass Readability.
pub(super) fn remove_code(document: &Document) {
    document.select("script").remove();
}

/// Remove page chrome — sidebars, the site header and footer, buttons and
/// search forms — for whole-document readings of pages that have no
/// article, such as listings and home pages. Readability does this itself when
/// it finds an article; headers inside an `article` are kept.
pub(super) fn remove_chrome(document: &Document) {
    document
        .select("aside, button, [role=search], body > header, body > footer")
        .remove();
}

/// Whether a node is hidden or boilerplate by its own attributes.
fn should_drop(node: &NodeRef<'_>) -> bool {
    if node.is("html, head, body, main") {
        return false;
    }
    if hidden(node) {
        return true;
    }
    boilerplate(node) && !node.is("article") && !holds_content(node)
}

/// Whether the element is invisible to a sighted reader at every width.
fn hidden(node: &NodeRef<'_>) -> bool {
    if node
        .attr("hidden")
        .is_some_and(|value| !value.trim().eq_ignore_ascii_case("until-found"))
    {
        return true;
    }
    if node
        .attr("aria-hidden")
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("true"))
    {
        return true;
    }
    if node.attr("style").is_some_and(|style| style_hides(&style)) {
        return true;
    }
    let Some(class) = node.class() else {
        return false;
    };
    let tokens: Vec<String> = class
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect();
    tokens
        .iter()
        .any(|token| HIDDEN_CLASSES.contains(&token.as_str()))
        && !tokens.iter().any(|token| shown_at_some_width(token))
}

/// Whether a utility class shows the element at some breakpoint: Tailwind
/// `md:block`, Bootstrap `d-lg-flex` or Bootstrap 3 `visible-sm`.
fn shown_at_some_width(token: &str) -> bool {
    const DISPLAYS: &[&str] = &[
        "block",
        "inline",
        "inline-block",
        "flex",
        "inline-flex",
        "grid",
        "inline-grid",
        "table",
        "table-row",
        "table-cell",
        "contents",
        "flow-root",
        "list-item",
        "visible",
    ];
    if let Some((prefix, display)) = token.rsplit_once(':') {
        return !prefix.is_empty() && DISPLAYS.contains(&display);
    }
    if token.starts_with("visible-") {
        return true;
    }
    ["sm", "md", "lg", "xl", "xxl"].iter().any(|width| {
        token
            .strip_prefix("d-")
            .and_then(|rest| rest.strip_prefix(width))
            .and_then(|rest| rest.strip_prefix('-'))
            .is_some_and(|display| DISPLAYS.contains(&display))
    })
}

/// Whether the element's role, classes or id mark it as page furniture.
fn boilerplate(node: &NodeRef<'_>) -> bool {
    if node
        .attr("role")
        .is_some_and(|role| BOILERPLATE_ROLES.contains(&role.trim().to_ascii_lowercase().as_str()))
    {
        return true;
    }
    let class = node
        .class()
        .map(|class| class.to_string())
        .unwrap_or_default();
    let id = node.attr("id").map(|id| id.to_string()).unwrap_or_default();
    class
        .split_whitespace()
        .chain(std::iter::once(id.as_str()))
        .flat_map(segments)
        .any(|segment| BOILERPLATE_SEGMENTS.contains(&segment.as_str()))
}

/// Split a class or id into lowercase words at `-`, `_`, `:` and camelCase
/// boundaries: `relatedPosts` → `related`, `posts`.
fn segments(token: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut previous_lower = false;
    for ch in token.chars() {
        if matches!(ch, '-' | '_' | ':' | '.' | '/') {
            words.push(std::mem::take(&mut word));
            previous_lower = false;
            continue;
        }
        if ch.is_uppercase() && previous_lower {
            words.push(std::mem::take(&mut word));
        }
        previous_lower = ch.is_lowercase() || ch.is_ascii_digit();
        word.extend(ch.to_lowercase());
    }
    words.push(word);
    words.retain(|word| !word.is_empty());
    words
}

/// Whether an element wraps the page's main content, so removing it as
/// furniture would remove the article with it.
fn holds_content(node: &NodeRef<'_>) -> bool {
    !node.find(&["article"]).is_empty() || !node.find(&["main"]).is_empty()
}

/// Whether an inline `style` hides the element: `display:none`,
/// `visibility:hidden`, zero opacity or font size, a 1px clip box, or text
/// indented or positioned far off-screen. Declarations are parsed, so
/// `margin-top:-4px` is not mistaken for `top:-4px`.
fn style_hides(style: &str) -> bool {
    let declarations: Vec<(String, String)> = style
        .split(';')
        .filter_map(|declaration| {
            let (property, value) = declaration.split_once(':')?;
            let value = value.to_ascii_lowercase().replace("!important", "");
            Some((
                property.trim().to_ascii_lowercase(),
                value.trim().to_string(),
            ))
        })
        .collect();
    let value = |name: &str| {
        declarations
            .iter()
            .rev()
            .find(|(property, _)| property == name)
            .map(|(_, value)| value.as_str())
    };
    let length = |name: &str| value(name).and_then(css_pixels);
    let positioned = matches!(value("position"), Some("absolute" | "fixed"));
    value("display") == Some("none")
        || matches!(value("visibility"), Some("hidden" | "collapse"))
        || value("opacity").and_then(|v| v.parse::<f64>().ok()) == Some(0.0)
        || length("font-size") == Some(0.0)
        || length("text-indent").is_some_and(|indent| indent <= -999.0)
        || (positioned
            && ["left", "top", "right"]
                .iter()
                .any(|side| length(side).is_some_and(|offset| offset <= -999.0)))
        || (length("width").is_some_and(|w| w <= 1.0)
            && length("height").is_some_and(|h| h <= 1.0)
            && value("overflow") == Some("hidden"))
}

/// A CSS length in approximate pixels (`em`/`rem` at 16px), or `None` when the
/// value is not a plain length.
fn css_pixels(value: &str) -> Option<f64> {
    let value = value.trim();
    let digits = value
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '-' | '+' | '.')))
        .unwrap_or(value.len());
    let number: f64 = value.get(..digits)?.parse().ok()?;
    match value.get(digits..)?.trim() {
        "" | "px" => Some(number),
        "em" | "rem" => Some(number * 16.0),
        "%" | "vw" | "vh" => (number == 0.0).then_some(0.0),
        _ => None,
    }
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

    /// Counterexamples from real pages: utility classes and declarations that
    /// merely contain a hiding word must keep their content.
    #[test]
    fn substrings_of_hiding_words_keep_content() {
        let text = visible_text(
            "<div class=\"mx-auto overflow-hidden\"><p>tailwind wrapper</p></div>\
             <p class=\"lead-paragraph\">lead paragraph</p>\
             <p style=\"margin-top:-4px;margin-left:-15px\">negative margins</p>\
             <p style=\"position:relative;left:-15px\">nudged</p>\
             <p style=\"opacity:0.8\">translucent</p>\
             <div class=\"thread-header\">thread header</div>",
        );
        for kept in [
            "tailwind wrapper",
            "lead paragraph",
            "negative margins",
            "nudged",
            "translucent",
            "thread header",
        ] {
            assert!(text.contains(kept), "{kept} should be kept, got {text}");
        }
    }

    /// Content hidden only at some widths, or findable in-page, is visible.
    #[test]
    fn responsive_and_findable_content_is_kept() {
        let text = visible_text(
            "<p class=\"hidden md:block\">desktop only</p>\
             <p class=\"d-none d-lg-flex\">bootstrap desktop</p>\
             <p hidden=\"until-found\">collapsed section</p>\
             <p class=\"hidden\">never shown</p>",
        );
        for kept in ["desktop only", "bootstrap desktop", "collapsed section"] {
            assert!(text.contains(kept), "{kept} should be kept, got {text}");
        }
        assert!(!text.contains("never shown"));
    }

    /// Furniture is matched by whole segments, including camelCase ids, but a
    /// furniture-named container that holds the article is not removed.
    #[test]
    fn boilerplate_segments_never_remove_the_article() {
        let text = visible_text(
            "<body class=\"has-sidebar\"><div class=\"page has-share-bar\">\
             <article><p>the article body</p></article></div>\
             <div id=\"relatedPosts\">more posts</div>\
             <div class=\"ad_slot\">sponsored</div></body>",
        );
        assert!(text.contains("the article body"), "{text}");
        assert!(!text.contains("more posts"));
        assert!(!text.contains("sponsored"));
    }

    /// The canonical screen-reader-only and image-replacement styles hide text.
    #[test]
    fn clip_box_and_indent_hiding_is_removed() {
        let text = visible_text(
            "<p>ok</p><span style=\"position:absolute;width:1px;height:1px;overflow:hidden\">clip</span>\
             <h1 style=\"text-indent:-9999px\">indent</h1>\
             <p style=\"DISPLAY: none !important\">shout</p>",
        );
        assert!(text.contains("ok"));
        for gone in ["clip", "indent", "shout"] {
            assert!(!text.contains(gone), "{gone} should be gone, got {text}");
        }
    }
}
