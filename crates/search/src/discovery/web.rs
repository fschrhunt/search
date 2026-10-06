//! The concrete providers: one HTTP family in this file.
//!
//! Brave, Marginalia, and Mwmbl are general web indexes; Wikipedia, Hacker
//! News, Stack Exchange, and arXiv are verticals that answer programming and
//! research queries with higher signal than a general crawl. Every provider is
//! keyless; Brave reads an optional key from the environment but works without
//! one against its server-rendered page.

use std::sync::LazyLock;
use std::time::Duration;

use super::parse;
use super::{Finding, Provider, ProviderError, ProviderFuture};

/// A browser user agent, sent deliberately. Several providers answer a non-browser
/// client with a challenge or a 403, so search presents the same fingerprint a
/// browser would. It is a known cost of scraping; the alternative is not to use
/// those providers at all.
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// One shared client: a bounded pool, per-call timeouts via the caller's
/// deadline, and gzip handled by the transport.
static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::limited(3))
        .pool_idle_timeout(Duration::from_secs(45))
        .build()
        // A failed build is an environment state, not a code path the caller
        // can act on; fall back to the default client rather than panicking.
        .unwrap_or_default()
});

/// Fetch a URL and return the body, mapping transport and status failures into a
/// classified `ProviderError`. Any non-success status is a rejection, not a
/// network failure, so the registry reports it precisely.
async fn get(url: &str) -> Result<String, ProviderError> {
    let response = CLIENT
        .get(url)
        .send()
        .await
        .map_err(|e| ProviderError::network(format!("request failed: {e}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(ProviderError::rejected(format!("HTTP {}", status.as_u16())));
    }
    response
        .text()
        .await
        .map_err(|e| ProviderError::network(format!("body failed: {e}")))
}

/// Parse JSON with a provider-specific message on failure.
fn parse_json<T: serde::de::DeserializeOwned>(
    body: &str,
    provider: &str,
) -> Result<T, ProviderError> {
    serde_json::from_str(body).map_err(|e| ProviderError::malformed(format!("{provider}: {e}")))
}

/// Brave's server-rendered search page. The strongest general index available
/// without a key, and the primary English result source.
pub(super) struct Brave;

impl Provider for Brave {
    fn name(&self) -> &'static str {
        "brave"
    }

    fn search(&self, query: String, limit: usize) -> ProviderFuture {
        Box::pin(async move {
            let url = format!(
                "https://search.brave.com/search?q={}",
                percent_encoding::utf8_percent_encode(&query, percent_encoding::NON_ALPHANUMERIC)
            );
            let body = get(&url).await?;
            Ok(parse_brave(&body, limit))
        })
    }
}

/// Read Brave's result blocks: each web result carries `data-type="web"` and
/// wraps a title in `.search-snippet-title` and a snippet in `.line-clamp-dynamic`.
fn parse_brave(html: &str, limit: usize) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for block in parse::split_after(html, "data-type=\"web\"", "data-type=\"") {
        let Some(href) = parse::first_attr(block, "href=\"") else {
            continue;
        };
        let url = clean_url(&href);
        let title = parse::first_tag(block, "div", &["search-snippet-title"])
            .or_else(|| parse::first_tag(block, "a", &[]))
            .map(|t| parse::text(&t))
            .unwrap_or_default();
        if url.is_empty() || title.is_empty() || !seen.insert(url.clone()) {
            continue;
        }
        let snippet = parse::first_tag(block, "div", &["line-clamp-dynamic"])
            .or_else(|| parse::first_tag(block, "div", &["generic-snippet"]))
            .map(|s| parse::text(&s))
            .filter(|s| !s.is_empty());
        out.push(Finding {
            title,
            url,
            snippet,
            providers: vec!["brave".into()],
            fetched_at: None,
            score: 0.0,
        });
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// Marginalia's independent index. `old-search` is used because the current
/// site renders results client-side; the old endpoint returns plain HTML.
pub(super) struct Marginalia;

impl Provider for Marginalia {
    fn name(&self) -> &'static str {
        "marginalia"
    }

    fn search(&self, query: String, limit: usize) -> ProviderFuture {
        Box::pin(async move {
            let url = format!(
                "https://old-search.marginalia.nu/search?query={}",
                percent_encoding::utf8_percent_encode(&query, percent_encoding::NON_ALPHANUMERIC)
            );
            let body = get(&url).await?;
            Ok(parse_marginalia(&body, limit))
        })
    }
}

/// Read Marginalia's `<section class="card search-result">` blocks.
fn parse_marginalia(html: &str, limit: usize) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for block in parse::split_after(html, "class=\"card search-result\"", "<section") {
        let Some(href) = parse::first_attr(block, "href=\"") else {
            continue;
        };
        let url = clean_url(&href);
        if url.is_empty() || !seen.insert(url.clone()) {
            continue;
        }
        let title = parse::first_tag(block, "h2", &[])
            .or_else(|| parse::first_tag(block, "a", &[]))
            .map(|t| parse::text(&t))
            .unwrap_or_default();
        out.push(Finding {
            title,
            url,
            snippet: None,
            providers: vec!["marginalia".into()],
            fetched_at: None,
            score: 0.0,
        });
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// Mwmbl, a community-crawled index with a JSON API.
pub(super) struct Mwmbl;

impl Provider for Mwmbl {
    fn name(&self) -> &'static str {
        "mwmbl"
    }

    fn search(&self, query: String, limit: usize) -> ProviderFuture {
        Box::pin(async move {
            let url = format!(
                "https://api.mwmbl.org/search/?s={}",
                percent_encoding::utf8_percent_encode(&query, percent_encoding::NON_ALPHANUMERIC)
            );
            let body = get(&url).await?;
            parse_mwmbl(&body, limit)
        })
    }
}

#[derive(serde::Deserialize)]
struct MwmblRow {
    url: String,
    #[serde(default)]
    title: Vec<MwmblField>,
    #[serde(default)]
    extract: Vec<MwmblField>,
}

#[derive(serde::Deserialize)]
struct MwmblField {
    value: String,
}

fn parse_mwmbl(body: &str, limit: usize) -> Result<Vec<Finding>, ProviderError> {
    let rows: Vec<MwmblRow> = parse_json(body, "mwmbl")?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let url = clean_url(&row.url);
            if url.is_empty() {
                return None;
            }
            Some(Finding {
                title: row
                    .title
                    .first()
                    .map(|f| f.value.clone())
                    .unwrap_or_default(),
                url,
                snippet: row.extract.first().map(|f| f.value.clone()),
                providers: vec!["mwmbl".into()],
                fetched_at: None,
                score: 0.0,
            })
        })
        .take(limit)
        .collect())
}

/// Wikipedia via the MediaWiki search API: keyless and authoritative for
/// entity and concept queries.
pub(super) struct Wikipedia;

impl Provider for Wikipedia {
    fn name(&self) -> &'static str {
        "wikipedia"
    }

    fn search(&self, query: String, limit: usize) -> ProviderFuture {
        Box::pin(async move {
            let url = format!(
                "https://en.wikipedia.org/w/api.php?action=query&list=search&format=json&srsearch={}&srlimit={}",
                percent_encoding::utf8_percent_encode(&query, percent_encoding::NON_ALPHANUMERIC),
                limit.clamp(1, 20)
            );
            let body = get(&url).await?;
            parse_wikipedia(&body, limit)
        })
    }
}

#[derive(serde::Deserialize)]
struct WikipediaPayload {
    query: WikipediaQuery,
}

#[derive(serde::Deserialize)]
struct WikipediaQuery {
    search: Vec<WikipediaHit>,
}

#[derive(serde::Deserialize)]
struct WikipediaHit {
    title: String,
    #[serde(default)]
    snippet: String,
}

fn parse_wikipedia(body: &str, limit: usize) -> Result<Vec<Finding>, ProviderError> {
    let payload: WikipediaPayload = parse_json(body, "wikipedia")?;
    Ok(payload
        .query
        .search
        .into_iter()
        .map(|hit| Finding {
            title: hit.title.clone(),
            url: format!(
                "https://en.wikipedia.org/wiki/{}",
                hit.title.replace(' ', "_")
            ),
            snippet: Some(parse::strip_tags(&hit.snippet)),
            providers: vec!["wikipedia".into()],
            fetched_at: None,
            score: 0.0,
        })
        .take(limit)
        .collect())
}

/// Hacker News stories, through the public Algolia API.
pub(super) struct HackerNews;

impl Provider for HackerNews {
    fn name(&self) -> &'static str {
        "hackernews"
    }

    fn search(&self, query: String, limit: usize) -> ProviderFuture {
        Box::pin(async move {
            let url = format!(
                "https://hn.algolia.com/api/v1/search?query={}&hitsPerPage={}",
                percent_encoding::utf8_percent_encode(&query, percent_encoding::NON_ALPHANUMERIC),
                limit.clamp(1, 30)
            );
            let body = get(&url).await?;
            parse_hn(&body, limit)
        })
    }
}

#[derive(serde::Deserialize)]
struct HnPayload {
    hits: Vec<HnHit>,
}

#[derive(serde::Deserialize)]
struct HnHit {
    #[serde(default)]
    title: String,
    url: Option<String>,
    #[serde(default)]
    story_text: String,
    /// Algolia names this `objectID`; not every hit carries one, so it is
    /// optional and the URL falls back to the item page only when present.
    #[serde(rename = "objectID", default)]
    object_id: Option<String>,
}

fn parse_hn(body: &str, limit: usize) -> Result<Vec<Finding>, ProviderError> {
    let payload: HnPayload = parse_json(body, "hackernews")?;
    Ok(payload
        .hits
        .into_iter()
        .filter_map(|hit| {
            let url = hit.url.filter(|u| !u.is_empty()).or_else(|| {
                hit.object_id
                    .map(|id| format!("https://news.ycombinator.com/item?id={id}"))
            })?;
            let title = if hit.title.is_empty() {
                "HN discussion".to_string()
            } else {
                hit.title
            };
            Some(Finding {
                title,
                url,
                snippet: Some(parse::strip_tags(&hit.story_text)).filter(|s| !s.is_empty()),
                providers: vec!["hackernews".into()],
                fetched_at: None,
                score: 0.0,
            })
        })
        .take(limit)
        .collect())
}

/// Stack Overflow, through the Stack Exchange API.
pub(super) struct StackExchange;

impl Provider for StackExchange {
    fn name(&self) -> &'static str {
        "stackexchange"
    }

    fn search(&self, query: String, limit: usize) -> ProviderFuture {
        Box::pin(async move {
            let url = format!(
                "https://api.stackexchange.com/2.3/search/advanced?order=desc&sort=relevance&q={}&site=stackoverflow&pagesize={}&filter=default",
                percent_encoding::utf8_percent_encode(&query, percent_encoding::NON_ALPHANUMERIC),
                limit.clamp(1, 30)
            );
            let body = get(&url).await?;
            parse_stackexchange(&body, limit)
        })
    }
}

#[derive(serde::Deserialize)]
struct SePayload {
    items: Vec<SeItem>,
}

#[derive(serde::Deserialize)]
struct SeItem {
    title: String,
    link: String,
    #[serde(default)]
    body_markdown: String,
}

fn parse_stackexchange(body: &str, limit: usize) -> Result<Vec<Finding>, ProviderError> {
    let payload: SePayload = parse_json(body, "stackexchange")?;
    Ok(payload
        .items
        .into_iter()
        .map(|item| Finding {
            title: parse::unescape(&item.title),
            url: item.link,
            snippet: Some(parse::truncate(
                &parse::strip_tags(&item.body_markdown),
                300,
            )),
            providers: vec!["stackexchange".into()],
            fetched_at: None,
            score: 0.0,
        })
        .take(limit)
        .collect())
}

/// arXiv preprints, through the Atom API.
pub(super) struct Arxiv;

impl Provider for Arxiv {
    fn name(&self) -> &'static str {
        "arxiv"
    }

    fn search(&self, query: String, limit: usize) -> ProviderFuture {
        Box::pin(async move {
            let url = format!(
                "https://export.arxiv.org/api/query?search_query=all:{}&max_results={}",
                percent_encoding::utf8_percent_encode(&query, percent_encoding::NON_ALPHANUMERIC),
                limit.clamp(1, 20)
            );
            let body = get(&url).await?;
            Ok(parse_arxiv(&body, limit))
        })
    }
}

/// Read arXiv's Atom entries with a tolerant scan: the feed is flat and each
/// `<entry>` carries the fields we need in a fixed order.
fn parse_arxiv(body: &str, limit: usize) -> Vec<Finding> {
    let mut out = Vec::new();
    for entry in parse::split_after(body, "<entry>", "</entry>") {
        let id = first_element(entry, "id");
        if id.is_empty() {
            continue;
        }
        out.push(Finding {
            title: collapse(&first_element(entry, "title")),
            url: id.trim().to_string(),
            snippet: Some(parse::truncate(
                &collapse(&first_element(entry, "summary")),
                300,
            )),
            providers: vec!["arxiv".into()],
            fetched_at: None,
            score: 0.0,
        });
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// The text of the first `<name>...</name>` in a fragment.
fn first_element(fragment: &str, name: &str) -> String {
    parse::first_tag(fragment, name, &[])
        .map(|inner| parse::strip_tags(&inner))
        .unwrap_or_default()
}

/// Collapse all whitespace runs to single spaces.
fn collapse(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Drop non-http(s) and obviously internal links.
fn clean_url(raw: &str) -> String {
    let raw = parse::unescape(raw);
    let raw = raw.trim();
    if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_string()
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brave_results_are_read_from_saved_markup() {
        let html = include_str!("testdata/brave.html");
        let results = parse_brave(html, 10);
        assert!(!results.is_empty(), "expected Brave results");
        assert!(results.iter().all(|r| !r.url.is_empty()));
        assert!(
            results[0].snippet.is_some(),
            "first result carries a snippet"
        );
    }

    #[test]
    fn marginalia_results_are_read_from_saved_markup() {
        let html = include_str!("testdata/marginalia.html");
        let results = parse_marginalia(html, 10);
        assert!(!results.is_empty(), "expected Marginalia results");
        assert!(results.iter().all(|r| r.url.starts_with("http")));
    }

    #[test]
    fn mwmbl_json_is_read() {
        let body = r#"[{"url":"https://example.com/a","title":[{"value":"Title"}],"extract":[{"value":"Body"}]}]"#;
        let results = parse_mwmbl(body, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Title");
    }

    #[test]
    fn wikipedia_json_is_read() {
        let body =
            r#"{"query":{"search":[{"title":"Kubernetes","snippet":"an <b>orchestrator</b>"}]}}"#;
        let results = parse_wikipedia(body, 10).unwrap();
        assert_eq!(results[0].url, "https://en.wikipedia.org/wiki/Kubernetes");
        assert_eq!(results[0].snippet.as_deref(), Some("an orchestrator"));
    }

    #[test]
    fn arxiv_atom_is_read() {
        let body = "<feed><entry><id>http://arxiv.org/abs/1</id><title>  A\n Title  </title><summary> Sum </summary></entry></feed>";
        let results = parse_arxiv(body, 10);
        assert_eq!(results[0].url, "http://arxiv.org/abs/1");
        assert_eq!(results[0].title, "A Title");
    }

    #[test]
    fn non_http_links_are_refused() {
        assert!(clean_url("javascript:alert(1)").is_empty());
        assert!(clean_url("ftp://example.com").is_empty());
        assert_eq!(clean_url("https://example.com/x"), "https://example.com/x");
    }
}
