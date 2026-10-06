//! The fan-out: run every selected provider concurrently under its own
//! deadline, then merge the answers into one ranking.
//!
//! Failure is interpretive, not fatal: a provider that errors is reported in its
//! `ProviderState` beside the results, so an empty result set is never mistaken
//! for a broken one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::task::JoinSet;

use super::{
    default_providers, FailureCause, Finding, Provider, ProviderError, ProviderState,
    ProviderStatus, Query, Ranked, Response,
};
use crate::config::{EngineSettings, SearchSettings};

/// Holds the enabled providers and the query bounds.
pub struct Registry {
    providers: Vec<Arc<dyn Provider>>,
    settings: SearchSettings,
}

impl Registry {
    /// Build the registry from configuration. A provider whose required key is
    /// missing is dropped rather than failing startup.
    pub fn new(engines: &EngineSettings, search: SearchSettings) -> Self {
        let providers: Vec<Arc<dyn Provider>> = default_providers(engines)
            .into_iter()
            .filter(|p| !p.missing_key())
            .map(Arc::from)
            .collect();
        Registry {
            providers,
            settings: search,
        }
    }

    /// The enabled provider names, for status output.
    pub fn names(&self) -> Vec<&'static str> {
        self.providers.iter().map(|p| p.name()).collect()
    }

    /// The server-side ceiling for one query.
    pub fn overall_timeout(&self) -> Duration {
        self.settings.timeout()
    }

    /// Run the query across every selected provider and merge the answers.
    pub async fn search(&self, query: Query) -> Response {
        let started = Instant::now();
        let limit = self.settings.result_limit(query.limit);
        let per_provider = if query.per_provider == 0 {
            limit
        } else {
            self.settings.result_limit(query.per_provider)
        };

        let selected: Vec<Arc<dyn Provider>> = self.select(&query.providers);
        let per_engine_time = self.settings.engine_timeout();

        let mut set: JoinSet<(usize, ProviderState, Vec<Ranked>)> = JoinSet::new();
        for (index, provider) in selected.iter().enumerate() {
            let provider = Arc::clone(provider);
            let text = query.text.clone();
            set.spawn(async move {
                run_provider(index, provider, text, per_provider, per_engine_time).await
            });
        }

        // Ordered by provider index so output is stable regardless of finish order.
        let mut collected: Vec<Option<(ProviderState, Vec<Ranked>)>> =
            (0..selected.len()).map(|_| None).collect();
        let mut panicked = Vec::new();
        let completed = tokio::time::timeout(self.settings.timeout(), async {
            while let Some(joined) = set.join_next().await {
                match joined {
                    // The index is produced by `enumerate` over `selected`, so it is
                    // always in range; `get_mut` keeps that provable rather than
                    // asserted.
                    Ok((index, state, results)) => {
                        if let Some(slot) = collected.get_mut(index) {
                            *slot = Some((state, results));
                        }
                    }
                    // A panicking provider must not take the query down; it is
                    // reported as a failed provider beside the results.
                    Err(join) => panicked.push(ProviderState {
                        name: "provider".into(),
                        status: ProviderStatus::Error,
                        count: 0,
                        error: Some(format!("provider task failed: {join}")),
                        elapsed_ms: 0,
                    }),
                }
            }
        })
        .await
        .is_ok();

        if !completed {
            set.abort_all();
            while set.join_next().await.is_some() {}
            for (index, provider) in selected.iter().enumerate() {
                if let Some(slot) = collected.get_mut(index) {
                    if slot.is_none() {
                        *slot = Some((
                            ProviderState {
                                name: provider.name().into(),
                                status: ProviderStatus::Timeout,
                                count: 0,
                                error: Some("provider exceeded the overall search deadline".into()),
                                elapsed_ms: started.elapsed().as_millis() as u64,
                            },
                            Vec::new(),
                        ));
                    }
                }
            }
        }

        let mut states = Vec::with_capacity(collected.len());
        let mut merged = Vec::new();
        for slot in collected.into_iter().flatten() {
            let (state, results) = slot;
            states.push(state);
            merged.extend(results);
        }
        states.extend(panicked);
        states.sort_by(|a, b| a.name.cmp(&b.name));

        let results = fuse(&merged, limit);
        Response {
            query: query.text,
            results,
            providers: states,
            duration_ms: started.elapsed().as_millis() as u64,
        }
    }

    /// Restrict to the requested names, preserving registry order. A request for
    /// names that match nothing returns no providers rather than silently
    /// running them all — the caller sees the empty result and its cause.
    fn select(&self, names: &[String]) -> Vec<Arc<dyn Provider>> {
        if names.is_empty() {
            return self.providers.clone();
        }
        self.providers
            .iter()
            .filter(|p| names.iter().any(|n| n == p.name()))
            .cloned()
            .collect()
    }
}

/// Run one provider under its deadline and classify the outcome. Each result is
/// stamped with its rank in this provider's own ordering, which is what fusion
/// scores on — a provider's rank-1 must count as rank-1 no matter where its
/// results land in the merged concatenation.
async fn run_provider(
    index: usize,
    provider: Arc<dyn Provider>,
    query: String,
    limit: usize,
    budget: Duration,
) -> (usize, ProviderState, Vec<Ranked>) {
    let name = provider.name().to_string();
    let started = Instant::now();
    let outcome = tokio::time::timeout(budget, provider.search(query, limit)).await;
    let elapsed_ms = started.elapsed().as_millis() as u64;

    let (status, findings, error) = match outcome {
        Ok(Ok(results)) => (ProviderStatus::Ok, results, None),
        Ok(Err(error)) => {
            let status = match error.cause {
                FailureCause::Timeout => ProviderStatus::Timeout,
                _ => ProviderStatus::Error,
            };
            (status, Vec::new(), Some(error.message))
        }
        Err(_) => (
            ProviderStatus::Timeout,
            Vec::new(),
            Some(ProviderError::network("provider exceeded its deadline").message),
        ),
    };
    // Stamp each finding with its rank in this provider's own ordering; fusion
    // scores on that, not on where it lands in the merged concatenation.
    let results: Vec<Ranked> = findings
        .into_iter()
        .enumerate()
        .map(|(rank, finding)| Ranked::provider(finding, rank + 1))
        .collect();

    let state = ProviderState {
        name,
        status,
        count: results.len(),
        error,
        elapsed_ms,
    };
    (index, state, results)
}

/// Merge provider answers with reciprocal-rank fusion: each provider votes
/// `1/(k + rank)`, so a URL several independent providers rank well rises above
/// one that only a single provider liked. Duplicates collapse by normalized URL.
fn fuse(results: &[Ranked], limit: usize) -> Vec<Finding> {
    const K: f64 = 10.0;
    use std::collections::HashMap;

    struct Aggregate {
        finding: Finding,
        score: f64,
        order: usize,
    }

    let mut seen: HashMap<String, Aggregate> = HashMap::new();
    let mut order = 0usize;
    for ranked in results {
        let key = normalize_url(&ranked.finding.url);
        if key.is_empty() {
            continue;
        }
        // The rank comes from the provider's own ordering, stamped before the
        // merge — not from this concatenation's position, which would penalize
        // whichever provider happened to be appended later. The weight scales
        // the vote, so the local corpus can count for more than one engine.
        let rank = ranked.rank.max(1);
        let vote = ranked.weight.max(0.0) / (K + rank as f64);
        match seen.get_mut(&key) {
            Some(existing) => {
                let incoming = &ranked.finding;
                existing.score += vote;
                if existing.finding.fetched_at.is_none() {
                    existing.finding.fetched_at = incoming.fetched_at;
                }
                if existing.finding.snippet.is_none() {
                    existing.finding.snippet = incoming.snippet.clone();
                }
                if existing.finding.title.is_empty() && !incoming.title.is_empty() {
                    existing.finding.title = incoming.title.clone();
                }
                for provider in &incoming.providers {
                    if !existing.finding.providers.contains(provider) {
                        existing.finding.providers.push(provider.clone());
                    }
                }
            }
            None => {
                seen.insert(
                    key,
                    Aggregate {
                        finding: ranked.finding.clone(),
                        score: vote,
                        order,
                    },
                );
                order += 1;
            }
        }
    }

    let mut out: Vec<Aggregate> = seen.into_values().collect();
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.order.cmp(&b.order))
    });
    out.into_iter()
        .take(limit)
        .map(|mut aggregate| {
            aggregate.finding.score = aggregate.score;
            aggregate.finding
        })
        .collect()
}

/// Blend local corpus hits into a provider response, so a query the corpus
/// already answers is answered partly from the box. A local hit is treated as
/// one more provider named `index`: its BM25 rank becomes a rank, and the
/// corpus's `weight` scales its vote, so a good local page can lead the merged
/// list without the remote results being discarded.
///
/// The blend only ever *adds*: if the corpus has nothing, the response is
/// unchanged, so search still works on a cold box.
pub fn blend(response: &mut Response, local: &[crate::index::Hit], weight: f64, limit: usize) {
    if local.is_empty() {
        return;
    }
    let local_weight = if weight > 0.0 { weight } else { 1.0 };
    let mut ranked: Vec<Ranked> = local
        .iter()
        .enumerate()
        .map(|(rank, hit)| Ranked {
            finding: Finding {
                title: if hit.title.is_empty() {
                    hit.url.clone()
                } else {
                    hit.title.clone()
                },
                url: hit.url.clone(),
                snippet: Some(hit.snippet.clone()).filter(|s| !s.is_empty()),
                fetched_at: Some(hit.fetched_at),
                providers: vec!["index".into()],
                score: hit.score,
            },
            rank: rank + 1,
            weight: local_weight,
        })
        .collect();

    // Existing remote findings, as a ranked source, before fusion. They keep the
    // default weight: the corpus is favored, not the web.
    let remote: Vec<Ranked> = response
        .results
        .iter()
        .enumerate()
        .map(|(rank, finding)| Ranked::provider(finding.clone(), rank + 1))
        .collect();

    ranked.extend(remote);
    response.results = fuse(&ranked, limit.max(1));
    // Report the corpus as a source alongside the live providers.
    if !response.providers.iter().any(|p| p.name == "index") {
        response.providers.push(ProviderState {
            name: "index".into(),
            status: ProviderStatus::Ok,
            count: local.len(),
            error: None,
            elapsed_ms: 0,
        });
        response.providers.sort_by(|a, b| a.name.cmp(&b.name));
    }
}

/// Normalize a URL for deduplication: lowercased scheme and host, no fragment,
/// no tracking parameters, no trailing slash. It never drops a meaningful query.
pub(super) fn normalize_url(raw: &str) -> String {
    let Ok(mut url) = url::Url::parse(raw.trim()) else {
        return String::new();
    };
    if url.scheme() != "http" && url.scheme() != "https" {
        return String::new();
    }
    let _ = url.set_scheme(&url.scheme().to_ascii_lowercase());
    if let Some(host) = url.host_str() {
        let _ = url.set_host(Some(&host.to_ascii_lowercase()));
    }
    url.set_fragment(None);
    let tracking = [
        "utm_source",
        "utm_medium",
        "utm_campaign",
        "utm_term",
        "utm_content",
        "ref",
        "fbclid",
        "gclid",
        "mc_cid",
        "mc_eid",
    ];
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !tracking.contains(&k.as_ref()))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if kept.is_empty() {
        url.set_query(None);
    } else {
        let mut pairs = url.query_pairs_mut();
        pairs.clear();
        for (k, v) in &kept {
            pairs.append_pair(k, v);
        }
    }
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(if path.is_empty() { "/" } else { &path });
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TestProvider {
        limit: Arc<AtomicUsize>,
        delay: Duration,
        count: usize,
    }

    impl Provider for TestProvider {
        fn name(&self) -> &'static str {
            "test"
        }

        fn search(&self, _query: String, limit: usize) -> super::super::ProviderFuture {
            self.limit.store(limit, Ordering::SeqCst);
            let delay = self.delay;
            let count = self.count;
            Box::pin(async move {
                tokio::time::sleep(delay).await;
                Ok((0..count)
                    .map(|i| Finding {
                        title: format!("result {i}"),
                        url: format!("https://example.com/{i}"),
                        snippet: None,
                        fetched_at: None,
                        providers: vec!["test".into()],
                        score: 0.0,
                    })
                    .collect())
            })
        }
    }

    fn registry(provider: TestProvider, settings: SearchSettings) -> Registry {
        Registry {
            providers: vec![Arc::new(provider)],
            settings,
        }
    }

    #[tokio::test]
    async fn configured_result_limit_caps_provider_work_and_output() {
        let asked = Arc::new(AtomicUsize::new(0));
        let settings = SearchSettings {
            max_results: 2,
            ..SearchSettings::default()
        };
        let registry = registry(
            TestProvider {
                limit: Arc::clone(&asked),
                delay: Duration::ZERO,
                count: 5,
            },
            settings,
        );

        let response = registry
            .search(Query {
                text: "test".into(),
                limit: 10,
                per_provider: 100,
                providers: Vec::new(),
            })
            .await;

        assert_eq!(asked.load(Ordering::SeqCst), 2);
        assert_eq!(response.results.len(), 2);
    }

    #[tokio::test]
    async fn overall_deadline_returns_without_waiting_for_provider_deadline() {
        let asked = Arc::new(AtomicUsize::new(0));
        let settings = SearchSettings {
            timeout: 10,
            engine_timeout: 5_000,
            ..SearchSettings::default()
        };
        let registry = registry(
            TestProvider {
                limit: asked,
                delay: Duration::from_secs(5),
                count: 0,
            },
            settings,
        );

        let response = registry
            .search(Query {
                text: "test".into(),
                ..Query::default()
            })
            .await;

        assert_eq!(response.providers.len(), 1);
        assert_eq!(response.providers[0].status, ProviderStatus::Timeout);
        assert_eq!(
            response.providers[0].error.as_deref(),
            Some("provider exceeded the overall search deadline")
        );
    }

    #[test]
    fn blended_results_keep_the_local_copy_fetch_time() {
        let mut response = Response {
            query: "test".into(),
            results: vec![ranked("https://example.com/page", "brave", 1).finding],
            providers: Vec::new(),
            duration_ms: 0,
        };
        let local = [crate::index::Hit {
            url: "https://example.com/page".into(),
            title: "example".into(),
            snippet: "snippet".into(),
            host: "example.com".into(),
            fetched_at: 1_700_000_000,
            score: 1.0,
        }];

        blend(&mut response, &local, 1.0, 10);

        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].fetched_at, Some(1_700_000_000));
        assert!(response.results[0].providers.contains(&"index".into()));
    }

    fn ranked(url: &str, provider: &'static str, rank: usize) -> Ranked {
        Ranked {
            finding: Finding {
                title: url.into(),
                url: url.into(),
                snippet: None,
                fetched_at: None,
                providers: vec![provider.into()],
                score: 0.0,
            },
            rank,
            weight: 1.0,
        }
    }

    /// A URL two providers rank highly must beat one only a single provider
    /// liked, and two providers' rank-1s must score the same regardless of which
    /// provider was merged first — the bug where a later provider's rank-1 was
    /// scored as if it were deep in the list.
    #[test]
    fn fusion_prefers_agreement() {
        let merged = vec![
            ranked("https://a.example/x", "brave", 1),
            ranked("https://a.example/x", "wikipedia", 1),
            ranked("https://b.example/y", "brave", 2),
            ranked("https://c.example/z", "mwmbl", 1),
        ];
        let out = fuse(&merged, 10);
        assert_eq!(out.len(), 3, "duplicates collapse");
        assert_eq!(out[0].url, "https://a.example/x");
        assert!(
            out[0].providers.contains(&"brave".into())
                && out[0].providers.contains(&"wikipedia".into()),
            "both providers are named, as a list"
        );
        // The single-vote rank-1 (mwmbl) must outrank the single-vote rank-2.
        let single_rank_one = out
            .iter()
            .position(|r| r.url == "https://c.example/z")
            .unwrap();
        let single_rank_two = out
            .iter()
            .position(|r| r.url == "https://b.example/y")
            .unwrap();
        assert!(
            single_rank_one < single_rank_two,
            "a later provider's rank-1 must not be penalized by merge order"
        );
    }

    /// Rank is taken from the provider's own ordering, so a rank-1 finding does
    /// not inherit the concatenation position.
    #[test]
    fn fusion_scores_on_provider_rank_not_position() {
        let merged = vec![
            ranked("https://first.example/a", "brave", 5),
            ranked("https://second.example/b", "wikipedia", 1),
        ];
        let out = fuse(&merged, 10);
        assert_eq!(
            out[0].url, "https://second.example/b",
            "the rank-1 finding wins even though it was merged second"
        );
    }

    /// A provider name that is a substring of another must not dedupe as if it
    /// were the same provider — the bug a joined string invited.
    #[test]
    fn provider_names_are_compared_whole() {
        let merged = vec![
            ranked("https://a.example/x", "arxiv", 1),
            ranked("https://a.example/x", "xiv", 2),
        ];
        let out = fuse(&merged, 10);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].providers, vec!["arxiv", "xiv"]);
    }

    /// Tracking parameters and fragments must not fragment a result.
    #[test]
    fn url_normalization_dedupes_noise() {
        assert_eq!(
            normalize_url("https://Example.com/Path/?utm_source=news&id=7#frag"),
            normalize_url("https://example.com/Path?id=7")
        );
        assert!(normalize_url("javascript:alert(1)").is_empty());
    }
}
