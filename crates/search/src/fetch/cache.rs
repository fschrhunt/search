//! An in-memory cache of fetch results, so repeated reads of one URL are
//! instant and an upstream server is spared. The durable corpus is the index;
//! this only holds recent answers.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::Fetched;

/// The most entries kept before the oldest are dropped.
const MAX_ENTRIES: usize = 2048;

/// A small TTL cache keyed by the requested URL. Errors are never stored, so a
/// transient failure is retried on the next call.
pub(super) struct TtlCache {
    ttl: Duration,
    entries: Mutex<HashMap<String, (Fetched, Instant)>>,
}

impl TtlCache {
    pub(super) fn new(ttl: Duration) -> Self {
        TtlCache {
            ttl,
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub(super) fn get(&self, key: &str) -> Option<Fetched> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        match entries.get(key) {
            Some((value, at)) if at.elapsed() <= self.ttl => Some(value.clone()),
            Some(_) => {
                entries.remove(key);
                None
            }
            None => None,
        }
    }

    pub(super) fn put(&self, key: &str, value: &Fetched) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if entries.len() >= MAX_ENTRIES {
            entries.clear();
        }
        entries.insert(key.to_string(), (value.clone(), Instant::now()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fetched() -> Fetched {
        Fetched {
            url: "https://example.com".into(),
            final_url: None,
            status: 200,
            content_type: "text/html".into(),
            title: None,
            byline: None,
            published: None,
            site: None,
            text: "body".into(),
            truncated: None,
            indexed: None,
            redirect: None,
            error: None,
        }
    }

    #[test]
    fn a_fresh_entry_is_returned_and_expiry_clears_it() {
        let cache = TtlCache::new(Duration::from_secs(60));
        cache.put("k", &fetched());
        assert!(cache.get("k").is_some());
        let mut entries = cache.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, inserted)) = entries.get_mut("k") {
            *inserted = Instant::now() - Duration::from_secs(61);
        }
        drop(entries);
        assert!(cache.get("k").is_none());
    }
}
