//! Read Stack Exchange questions through the public API.
//!
//! Stack Overflow and its sibling sites refuse scripted page reads, so a
//! question URL (`/questions/ID` or `/q/ID` on a Stack Exchange host) is read
//! from `fetch.stack_exchange` instead: one keyless API call returns the
//! question and every answer, which are rendered as one HTML document for the
//! normal extractor. Any other URL, including answer links, is not matched.

/// The API filter selecting question title, link, score and body, plus each
/// answer's body, score and acceptance. Created once with `/filters/create`
/// (`base=none`, `unsafe=false`): bodies arrive as the site's sanitized HTML.
const FILTER: &str = "!T.hvPg9V1OEZbGG*z8aPrnvQq(NgsX";

/// Hosts whose own domain is a Stack Exchange site; `*.stackexchange.com` and
/// subdomains of these (`ru.stackoverflow.com`, `meta.superuser.com`) are too.
const SITES: &[&str] = &[
    "stackoverflow.com",
    "superuser.com",
    "serverfault.com",
    "askubuntu.com",
    "mathoverflow.net",
    "stackapps.com",
];

/// The API request for a Stack Exchange question URL, or `None` when `raw` is
/// not one or `base` (the configured API root) is empty.
pub(super) fn api_url(base: &str, raw: &str) -> Option<url::Url> {
    if base.is_empty() {
        return None;
    }
    let page = url::Url::parse(raw).ok()?;
    let host = page.host_str()?.to_ascii_lowercase();
    let site = host.strip_prefix("www.").unwrap_or(&host);
    let known = SITES
        .iter()
        .any(|root| site == *root || site.ends_with(&format!(".{root}")))
        || (site.ends_with(".stackexchange.com") && site != "api.stackexchange.com");
    if !known {
        return None;
    }
    let mut segments = page.path_segments()?;
    let id = match (segments.next(), segments.next()) {
        (Some("questions" | "q"), Some(id)) if id.bytes().all(|b| b.is_ascii_digit()) => id,
        _ => return None,
    };
    if id.is_empty() {
        return None;
    }
    let mut api =
        url::Url::parse(&format!("{}/questions/{id}", base.trim_end_matches('/'))).ok()?;
    api.query_pairs_mut()
        .append_pair("site", site)
        .append_pair("filter", FILTER);
    Some(api)
}

#[derive(serde::Deserialize)]
struct Response {
    #[serde(default)]
    items: Vec<Question>,
    error_message: Option<String>,
}

#[derive(serde::Deserialize)]
struct Question {
    title: String,
    link: String,
    #[serde(default)]
    score: i64,
    #[serde(default)]
    body: String,
    #[serde(default)]
    answers: Vec<Answer>,
}

#[derive(serde::Deserialize)]
struct Answer {
    #[serde(default)]
    body: String,
    #[serde(default)]
    score: i64,
    #[serde(default)]
    is_accepted: bool,
}

/// A question rendered for the extractor: its canonical link and an HTML
/// document with the question, then answers by score, each under a heading.
pub(super) struct Rendered {
    pub link: String,
    pub html: String,
}

/// Parse an API body. An API error or a missing question is the caller-facing reason.
pub(super) fn render(body: &[u8]) -> Result<Rendered, String> {
    let response: Response = serde_json::from_slice(body)
        .map_err(|_| "Stack Exchange API returned unreadable JSON".to_string())?;
    let Some(mut question) = response.items.into_iter().next() else {
        return Err(match response.error_message {
            Some(message) => format!("Stack Exchange API: {message}"),
            None => "Stack Exchange question not found".into(),
        });
    };
    question
        .answers
        .sort_by_key(|answer| std::cmp::Reverse(answer.score));
    // Title and bodies are already HTML-escaped by the API's safe filter.
    let mut html = format!(
        "<html><head><title>{title}</title></head><body><article><h1>{title}</h1>\
         <h2>Question (score {score})</h2>{body}",
        title = question.title,
        score = question.score,
        body = question.body,
    );
    for answer in &question.answers {
        let accepted = if answer.is_accepted { ", accepted" } else { "" };
        html.push_str(&format!(
            "<h2>Answer (score {}{accepted})</h2>{}",
            answer.score, answer.body
        ));
    }
    html.push_str("</article></body></html>");
    Ok(Rendered {
        link: question.link,
        html,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://api.stackexchange.com/2.3";

    /// Question links on every Stack Exchange host map to the API, keyed by that host.
    #[test]
    fn question_urls_map_to_the_api() {
        for (raw, site, id) in [
            (
                "https://stackoverflow.com/questions/1789945/how-to",
                "stackoverflow.com",
                "1789945",
            ),
            (
                "https://www.stackoverflow.com/q/11227809",
                "stackoverflow.com",
                "11227809",
            ),
            (
                "https://unix.stackexchange.com/questions/1",
                "unix.stackexchange.com",
                "1",
            ),
            (
                "https://ru.stackoverflow.com/questions/5",
                "ru.stackoverflow.com",
                "5",
            ),
            (
                "https://askubuntu.com/questions/7/x?tab=votes",
                "askubuntu.com",
                "7",
            ),
        ] {
            let api = api_url(BASE, raw).unwrap();
            assert_eq!(api.path(), format!("/2.3/questions/{id}"));
            assert!(api
                .query_pairs()
                .any(|(key, value)| key == "site" && value == site));
        }
    }

    /// Other pages, answer links, lookalike hosts and a disabled reader are left to the normal fetch.
    #[test]
    fn other_urls_are_not_matched() {
        for raw in [
            "https://stackoverflow.com/questions",
            "https://stackoverflow.com/questions/tagged/rust",
            "https://stackoverflow.com/a/123",
            "https://stackoverflow.com.evil.example/questions/1",
            "https://notstackoverflow.com/questions/1",
            "https://api.stackexchange.com/questions/1",
            "https://example.com/questions/1",
        ] {
            assert!(api_url(BASE, raw).is_none(), "{raw}");
        }
        assert!(api_url("", "https://stackoverflow.com/questions/1").is_none());
    }

    /// Answers follow the question highest score first, marked when accepted.
    #[test]
    fn answers_render_by_score() {
        let body = br#"{"items":[{"title":"Q &amp; A","link":"https://stackoverflow.com/questions/1/q","score":3,"body":"<p>ask</p>",
            "answers":[{"body":"<p>low</p>","score":1,"is_accepted":true},{"body":"<p>high</p>","score":9,"is_accepted":false}]}]}"#;
        let rendered = render(body).unwrap();
        assert_eq!(rendered.link, "https://stackoverflow.com/questions/1/q");
        let high = rendered.html.find("high").unwrap();
        let low = rendered.html.find("low").unwrap();
        assert!(high < low);
        assert!(rendered.html.contains("Answer (score 1, accepted)"));
    }

    /// An API error surfaces its message; an empty result is a missing question.
    #[test]
    fn errors_name_the_reason() {
        let throttled = br#"{"items":[],"error_message":"too many requests from this IP"}"#;
        assert_eq!(
            render(throttled).err().unwrap(),
            "Stack Exchange API: too many requests from this IP"
        );
        assert_eq!(
            render(br#"{"items":[]}"#).err().unwrap(),
            "Stack Exchange question not found"
        );
    }
}
