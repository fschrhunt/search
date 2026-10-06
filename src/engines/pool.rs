//! The fan-out: run every selected engine concurrently under its own
//! deadline, then merge the answers into one ranking.
//!
//! Failure is interpretive, not fatal: an engine that errors is reported in its
//! `EngineState` beside the results, so an empty result set is never mistaken
//! for a broken one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::task::JoinSet;

use super::{
    Answer, Engine, EngineError, EngineState, EngineStatus, FailureCause, Link, Query, Ranked,
};
use crate::core::config::SearchSettings;

/// Owns enabled engine transports, package leases, and query bounds.
pub struct Pool {
    engines: Vec<Arc<dyn Engine>>,
    settings: SearchSettings,
    _leases: Vec<Arc<std::fs::File>>,
}

impl Pool {
    /// Resolve selected trusted packages and build transports without reading credentials.
    /// Package leases remain held for the lifetime of this pool.
    pub fn new(
        config: &crate::core::config::Config,
    ) -> Result<Self, crate::core::config::ConfigError> {
        config.validate()?;
        let mut engines: Vec<Arc<dyn Engine>> = Vec::new();
        let mut leases = Vec::new();
        if config.engines.enabled {
            for id in &config.engines.use_engines {
                let package = crate::engines::resolve(&config.home, id).map_err(|_| {
                    crate::core::config::ConfigError::new(format!(
                        "engine {id}: package unavailable; run search install {id} or disable it"
                    ))
                })?;
                let settings =
                    crate::core::config::resolve_adapter(&package, config.engines.config.get(id))?;
                let adapter =
                    super::adapter::Adapter::new(id.clone(), settings, &config.user_agent)
                        .map_err(|_| {
                            crate::core::config::ConfigError::new(format!(
                                "engine {id}: custom HTTP client failed"
                            ))
                        })?;
                if let Some(lease) = package.lease {
                    leases.push(lease);
                }
                engines.push(Arc::new(adapter));
            }
        }
        Ok(Self {
            engines,
            settings: config.search.clone(),
            _leases: leases,
        })
    }

    /// The enabled engine names, for status output.
    pub fn names(&self) -> Vec<String> {
        self.engines.iter().map(|p| p.name().to_owned()).collect()
    }

    /// The server-side ceiling for one query.
    pub fn overall_timeout(&self) -> Duration {
        self.settings.timeout()
    }

    /// Run the query across every selected engine and merge the answers.
    pub async fn search(&self, query: Query) -> Answer {
        let started = Instant::now();
        let limit = self.settings.result_limit(query.limit);
        let selected: Vec<Arc<dyn Engine>> = self.select(&query.engines);
        let per_engine_time = self.settings.engine_timeout();

        let mut set: JoinSet<(usize, EngineState, Vec<Ranked>)> = JoinSet::new();
        for (index, engine) in selected.iter().enumerate() {
            let engine = Arc::clone(engine);
            let text = query.text.clone();
            set.spawn(async move { run_engine(index, engine, text, limit, per_engine_time).await });
        }

        // Ordered by engine index so output is stable regardless of finish order.
        let mut collected: Vec<Option<(EngineState, Vec<Ranked>)>> =
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
                    // A panicking engine must not take the query down; it is
                    // reported as a failed engine beside the results.
                    Err(_) => panicked.push(EngineState {
                        name: "engine".into(),
                        status: EngineStatus::Error,
                        count: 0,
                        error: Some("engine task failed".into()),
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
            for (index, engine) in selected.iter().enumerate() {
                if let Some(slot) = collected.get_mut(index) {
                    if slot.is_none() {
                        *slot = Some((
                            EngineState {
                                name: engine.name().into(),
                                status: EngineStatus::Timeout,
                                count: 0,
                                error: Some("engine exceeded the overall search deadline".into()),
                                elapsed_ms: started.elapsed().as_millis() as u64,
                            },
                            Vec::new(),
                        ));
                    }
                }
            }
        }

        let mut states: Vec<EngineState> = query
            .engines
            .iter()
            .filter(|name| {
                !self
                    .engines
                    .iter()
                    .any(|engine| engine.name() == name.as_str())
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(|name| EngineState {
                name: name.clone(),
                status: EngineStatus::Error,
                count: 0,
                error: Some("engine is not enabled".into()),
                elapsed_ms: 0,
            })
            .collect();
        let mut merged = Vec::new();
        for slot in collected.into_iter().flatten() {
            let (state, results) = slot;
            states.push(state);
            merged.extend(results);
        }
        states.extend(panicked);
        states.sort_by(|a, b| a.name.cmp(&b.name));

        let results = fuse(&merged, limit);
        Answer {
            query: query.text,
            results,
            engines: states,
            duration_ms: started.elapsed().as_millis() as u64,
        }
    }

    /// Restrict to the requested names, preserving pool order. A request for
    /// names that match nothing returns no engines rather than silently
    /// running them all — the caller sees the empty result and its cause.
    fn select(&self, names: &[String]) -> Vec<Arc<dyn Engine>> {
        if names.is_empty() {
            return self.engines.clone();
        }
        self.engines
            .iter()
            .filter(|p| names.iter().any(|n| n == p.name()))
            .cloned()
            .collect()
    }
}

/// Run one engine under its deadline and classify the outcome. Each result is
/// stamped with its rank in this engine's own ordering, which is what fusion
/// scores on — an engine's rank-1 must count as rank-1 no matter where its
/// results land in the merged concatenation.
async fn run_engine(
    index: usize,
    engine: Arc<dyn Engine>,
    query: String,
    limit: usize,
    budget: Duration,
) -> (usize, EngineState, Vec<Ranked>) {
    let name = engine.name().to_string();
    let started = Instant::now();
    let outcome = tokio::time::timeout(budget, engine.search(query, limit)).await;
    let elapsed_ms = started.elapsed().as_millis() as u64;

    let (status, links, error) = match outcome {
        Ok(Ok(results)) => (EngineStatus::Ok, results, None),
        Ok(Err(error)) => {
            let status = match error.cause {
                FailureCause::Timeout => EngineStatus::Timeout,
                _ => EngineStatus::Error,
            };
            (status, Vec::new(), Some(error.message))
        }
        Err(_) => (
            EngineStatus::Timeout,
            Vec::new(),
            Some(EngineError::network("engine exceeded its deadline").message),
        ),
    };
    // Stamp each link with its rank in this engine's own ordering; fusion
    // scores on that, not on where it lands in the merged concatenation.
    let results: Vec<Ranked> = links
        .into_iter()
        .enumerate()
        .map(|(rank, link)| Ranked::engine(link, rank + 1))
        .collect();

    let state = EngineState {
        name,
        status,
        count: results.len(),
        error,
        elapsed_ms,
    };
    (index, state, results)
}

/// Merge engine answers with reciprocal-rank fusion: each engine votes
/// `1/(k + rank)`, so a URL several independent engines rank well rises above
/// one that only a single engine liked. Duplicates collapse by normalized URL.
fn fuse(results: &[Ranked], limit: usize) -> Vec<Link> {
    const K: f64 = 10.0;
    use std::collections::HashMap;

    struct Aggregate {
        link: Link,
        score: f64,
        order: usize,
    }

    let mut seen: HashMap<String, Aggregate> = HashMap::new();
    let mut order = 0usize;
    for ranked in results {
        let key = normalize_url(&ranked.link.url);
        if key.is_empty() {
            continue;
        }
        // The rank comes from the engine's own ordering, stamped before the
        // merge — not from this concatenation's position, which would penalize
        // whichever engine happened to be appended later.
        let rank = ranked.rank.max(1);
        let vote = 1.0 / (K + rank as f64);
        match seen.get_mut(&key) {
            Some(existing) => {
                let incoming = &ranked.link;
                existing.score += vote;
                if existing.link.snippet.is_none() {
                    existing.link.snippet = incoming.snippet.clone();
                }
                if existing.link.title.is_empty() && !incoming.title.is_empty() {
                    existing.link.title = incoming.title.clone();
                }
                for engine in &incoming.engines {
                    if !existing.link.engines.contains(engine) {
                        existing.link.engines.push(engine.clone());
                    }
                }
            }
            None => {
                seen.insert(
                    key,
                    Aggregate {
                        link: ranked.link.clone(),
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
            aggregate.link.score = aggregate.score;
            aggregate.link
        })
        .collect()
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
    impl Pool {
        /// Build fixture transports without bypassing package resolution in production.
        pub(in crate::engines) fn from_adapters(
            adapters: Vec<(&str, crate::core::config::Adapter)>,
            settings: SearchSettings,
        ) -> Self {
            Self {
                engines: adapters
                    .into_iter()
                    .map(|(name, settings)| {
                        Arc::new(
                            crate::engines::adapter::Adapter::new(
                                name.into(),
                                settings,
                                "search-test",
                            )
                            .unwrap(),
                        ) as Arc<dyn Engine>
                    })
                    .collect(),
                settings,
                _leases: Vec::new(),
            }
        }
    }

    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TestEngine {
        limit: Arc<AtomicUsize>,
        delay: Duration,
        count: usize,
    }

    impl Engine for TestEngine {
        fn name(&self) -> &str {
            "test"
        }

        fn search(&self, _query: String, limit: usize) -> super::super::EngineFuture {
            self.limit.store(limit, Ordering::SeqCst);
            let delay = self.delay;
            let count = self.count;
            Box::pin(async move {
                tokio::time::sleep(delay).await;
                Ok((0..count)
                    .map(|i| Link {
                        title: format!("result {i}"),
                        url: format!("https://example.com/{i}"),
                        snippet: None,
                        engines: vec!["test".into()],
                        score: 0.0,
                    })
                    .collect())
            })
        }
    }

    fn pool(engine: TestEngine, settings: SearchSettings) -> Pool {
        Pool {
            engines: vec![Arc::new(engine)],
            _leases: Vec::new(),
            settings,
        }
    }

    #[tokio::test]
    async fn configured_result_limit_caps_engine_work_and_output() {
        let asked = Arc::new(AtomicUsize::new(0));
        let settings = SearchSettings {
            max_results: 2,
            ..SearchSettings::default()
        };
        let pool = pool(
            TestEngine {
                limit: Arc::clone(&asked),
                delay: Duration::ZERO,
                count: 5,
            },
            settings,
        );

        let response = pool
            .search(Query {
                text: "test".into(),
                limit: 10,
                engines: Vec::new(),
            })
            .await;

        assert_eq!(asked.load(Ordering::SeqCst), 2);
        assert_eq!(response.results.len(), 2);
    }

    #[tokio::test]
    async fn unknown_engine_selection_is_reported_without_running_other_engines() {
        let asked = Arc::new(AtomicUsize::new(0));
        let pool = pool(
            TestEngine {
                limit: Arc::clone(&asked),
                delay: Duration::ZERO,
                count: 1,
            },
            SearchSettings::default(),
        );
        let answer = pool
            .search(Query {
                text: "query".into(),
                engines: vec!["missing".into()],
                ..Default::default()
            })
            .await;
        assert_eq!(asked.load(Ordering::SeqCst), 0);
        assert!(answer.results.is_empty());
        assert_eq!(answer.engines[0].name, "missing");
        assert_eq!(answer.engines[0].status, EngineStatus::Error);
    }

    #[tokio::test]
    async fn overall_deadline_returns_without_waiting_for_engine_deadline() {
        let asked = Arc::new(AtomicUsize::new(0));
        let settings = SearchSettings {
            timeout: 10,
            engine_timeout: 5_000,
            ..SearchSettings::default()
        };
        let pool = pool(
            TestEngine {
                limit: asked,
                delay: Duration::from_secs(5),
                count: 0,
            },
            settings,
        );

        let response = pool
            .search(Query {
                text: "test".into(),
                ..Query::default()
            })
            .await;

        assert_eq!(response.engines.len(), 1);
        assert_eq!(response.engines[0].status, EngineStatus::Timeout);
        assert_eq!(
            response.engines[0].error.as_deref(),
            Some("engine exceeded the overall search deadline")
        );
    }

    fn ranked(url: &str, engine: &'static str, rank: usize) -> Ranked {
        Ranked {
            link: Link {
                title: url.into(),
                url: url.into(),
                snippet: None,
                engines: vec![engine.into()],
                score: 0.0,
            },
            rank,
        }
    }

    /// A URL two engines rank highly must beat one only a single engine
    /// liked, and two engines' rank-1s must score the same regardless of which
    /// engine was merged first — the bug where a later engine's rank-1 was
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
            out[0].engines.contains(&"brave".into())
                && out[0].engines.contains(&"wikipedia".into()),
            "both engines are named, as a list"
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
            "a later engine's rank-1 must not be penalized by merge order"
        );
    }

    /// Rank is taken from the engine's own ordering, so a rank-1 link does
    /// not inherit the concatenation position.
    #[test]
    fn fusion_scores_on_engine_rank_not_position() {
        let merged = vec![
            ranked("https://first.example/a", "brave", 5),
            ranked("https://second.example/b", "wikipedia", 1),
        ];
        let out = fuse(&merged, 10);
        assert_eq!(
            out[0].url, "https://second.example/b",
            "the rank-1 link wins even though it was merged second"
        );
    }

    /// A engine name that is a substring of another must not dedupe as if it
    /// were the same engine — the bug a joined string invited.
    #[test]
    fn engine_names_are_compared_whole() {
        let merged = vec![
            ranked("https://a.example/x", "arxiv", 1),
            ranked("https://a.example/x", "xiv", 2),
        ];
        let out = fuse(&merged, 10);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].engines, vec!["arxiv", "xiv"]);
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
