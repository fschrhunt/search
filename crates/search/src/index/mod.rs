//! The server-local, on-disk corpus: saved pages searchable offline,
//! shared by clients authorized to use this instance.
//!
//! It is deliberately separate from discovery. Discovery finds URLs anywhere on
//! the web; the index makes content already fetched instant and independent of
//! any upstream provider. Storage is SQLite in WAL mode through a bundled
//! engine, so the binary carries its database with no system dependency.

mod schema;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::Connection;

use crate::config::IndexSettings;

pub use schema::Stats;

/// A stored page.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Doc {
    pub url: String,
    pub title: String,
    pub text: String,
    pub host: String,
    pub fetched_at: i64,
}

/// A full-text hit, ranked by BM25.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Hit {
    pub url: String,
    pub title: String,
    pub snippet: String,
    pub host: String,
    pub fetched_at: i64,
    pub score: f64,
}

/// A store failure. Every variant names what could not be done, so the caller
/// can decide whether to surface or log it.
#[derive(Debug)]
pub struct StoreError(String);

impl StoreError {
    /// Explicit local operations cannot access a disabled corpus.
    pub(crate) fn disabled() -> Self {
        StoreError("local index is disabled".into())
    }
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        StoreError(error.to_string())
    }
}

/// Owns the SQLite connection and its schema. The connection is `Send` but not
/// `Sync`, so it is held under a mutex: the service is shared across handler
/// tasks, and SQLite serializes writes anyway.
pub struct Store {
    connection: Mutex<Connection>,
    path: PathBuf,
    settings: IndexSettings,
}

impl Store {
    /// Open the store under `dir`, creating the dir and schema, and apply
    /// the corpus's hygiene: prune what has aged out before serving.
    pub fn open(dir: &Path, settings: IndexSettings) -> Result<Self, StoreError> {
        std::fs::create_dir_all(dir)
            .map_err(|e| StoreError(format!("create data dir {}: {e}", dir.display())))?;
        let path = dir.join("search.db");
        let connection = Connection::open(&path)?;
        // WAL gives concurrent readers beside one writer; NORMAL sync is the
        // durable-enough default for a cache-like corpus.
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.pragma_update(None, "busy_timeout", 5000)?;
        schema::apply(&connection)?;
        let store = Store {
            connection: Mutex::new(connection),
            path,
            settings,
        };
        // Startup is the natural moment to age the corpus out.
        store.prune()?;
        Ok(store)
    }

    /// The database file path, for status output.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Upsert one document. The caller has already capped its text; the store
    /// prunes after a write so it cannot grow past its ceiling.
    pub fn put(&self, doc: &Doc) -> Result<(), StoreError> {
        let connection = self.lock();
        connection.execute(
            "INSERT INTO pages (url, title, text, host, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(url) DO UPDATE SET
                 title = excluded.title,
                 text = excluded.text,
                 host = excluded.host,
                 fetched_at = excluded.fetched_at",
            rusqlite::params![doc.url, doc.title, doc.text, doc.host, doc.fetched_at],
        )?;
        drop(connection);
        self.prune()?;
        Ok(())
    }

    /// Keep the corpus within its age and size bounds: drop documents older than
    /// the maximum age, then evict the least recently touched until the stored
    /// text fits the byte ceiling. Both bounds default on, so a long-lived
    /// service cannot grow without limit.
    pub fn prune(&self) -> Result<(), StoreError> {
        let connection = self.lock();
        if let Some(max_age) = self.settings.max_age() {
            let cutoff = now_secs().saturating_sub(max_age.as_secs().min(i64::MAX as u64) as i64);
            connection.execute("DELETE FROM pages WHERE fetched_at < ?1", [cutoff])?;
        }
        let ceiling = self.settings.max_size_bytes();
        if ceiling == 0 {
            return Ok(());
        }
        // Evict oldest-first until the stored bytes fit. Each pass asks SQLite
        // for the current total, so the loop terminates exactly at the ceiling.
        loop {
            let bytes: i64 = connection.query_row(
                "SELECT coalesce(sum(length(CAST(text AS BLOB))), 0) FROM pages",
                [],
                |row| row.get(0),
            )?;
            if bytes < 0 || bytes as u64 <= ceiling {
                return Ok(());
            }
            let removed = connection.execute(
                "DELETE FROM pages WHERE url = (
                    SELECT url FROM pages ORDER BY fetched_at ASC LIMIT 1
                )",
                [],
            )?;
            if removed == 0 {
                return Ok(());
            }
        }
    }

    /// Full-text search over the corpus, ranked by BM25 (higher score first).
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Hit>, StoreError> {
        let limit = if limit == 0 { 10 } else { limit };
        let Some(match_expression) = schema::fts_query(query) else {
            return Ok(Vec::new());
        };
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT p.url, p.title, p.text, p.host, p.fetched_at, bm25(pages_fts) AS rank
             FROM pages_fts
             JOIN pages p ON p.rowid = pages_fts.rowid
             WHERE pages_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;
        let rows =
            statement.query_map(rusqlite::params![match_expression, limit as i64], |row| {
                let text: String = row.get(2)?;
                let rank: f64 = row.get(5)?;
                Ok(Hit {
                    url: row.get(0)?,
                    title: row.get(1)?,
                    // The snippet is the passage that matched the query, not the
                    // document's opening: a snippet without the query terms says
                    // nothing about why the page was returned.
                    snippet: snippet(&text, query),
                    host: row.get(3)?,
                    fetched_at: row.get(4)?,
                    // bm25 returns smaller-is-better negative values; invert for
                    // a higher-is-better score callers can sort on.
                    score: -rank,
                })
            })?;
        let mut hits = Vec::new();
        for row in rows {
            hits.push(row?);
        }
        Ok(hits)
    }

    /// Corpus counts for status output.
    pub fn stats(&self) -> Result<Stats, StoreError> {
        let connection = self.lock();
        let (documents, hosts, bytes, oldest, newest): (i64, i64, i64, i64, i64) = connection
            .query_row(
            "SELECT count(*), count(DISTINCT host), coalesce(sum(length(CAST(text AS BLOB))), 0),
                        coalesce(min(fetched_at), 0), coalesce(max(fetched_at), 0)
                 FROM pages",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )?;
        Ok(Stats {
            documents: documents.max(0) as usize,
            hosts: hosts.max(0) as usize,
            bytes: bytes.max(0) as u64,
            oldest: if oldest > 0 { Some(oldest) } else { None },
            newest: if newest > 0 { Some(newest) } else { None },
        })
    }

    /// Documents on the seeded refresh hosts older than the freshness window,
    /// newest-first, so a caller can re-fetch them to keep the corpus true.
    pub fn stale_seeded(&self, hosts: &[String], fresh_for: std::time::Duration) -> Vec<String> {
        if hosts.is_empty() {
            return Vec::new();
        }
        let cutoff = now_secs() - fresh_for.as_secs() as i64;
        let connection = self.lock();
        let placeholders = (1..=hosts.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let tail = hosts.len() + 1;
        let sql = format!(
            "SELECT url FROM pages
             WHERE host IN ({placeholders}) AND fetched_at < ?{tail}
             ORDER BY fetched_at ASC LIMIT 50"
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = hosts
            .iter()
            .map(|h| Box::new(h.clone()) as Box<dyn rusqlite::ToSql>)
            .collect();
        params.push(Box::new(cutoff));
        let refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let Ok(mut statement) = connection.prepare(&sql) else {
            return Vec::new();
        };
        let rows = statement.query_map(refs.as_slice(), |row| row.get::<_, String>(0));
        match rows {
            Ok(rows) => rows.filter_map(Result::ok).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Lock the connection, recovering from a poisoned mutex: a panic in one
    /// handler must not wedge the store for every later request.
    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.connection.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The passage of `text` that best matches `query`, or the opening when the
/// query matches nothing. Reuses the shared passage scorer so the index and the
/// tool agree on what "the relevant part" means.
fn snippet(text: &str, query: &str) -> String {
    const WINDOW: usize = 240;
    let passages = crate::text::select(text, query, WINDOW);
    if let Some(first) = passages.first() {
        let mut out = first.text.split_whitespace().collect::<Vec<_>>().join(" ");
        if out.chars().count() > WINDOW {
            out = out.chars().take(WINDOW).collect::<String>() + "…";
        }
        return out;
    }
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(WINDOW).collect::<String>()
}

/// Extract the host from a URL, for grouping and display.
pub fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default()
}

/// Cap text at `chars` characters on a character boundary. The corpus stores a
/// finder's worth of a page, not the whole of it.
pub fn cap_chars(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        return text.to_string();
    }
    text.chars().take(chars).collect()
}

/// The current Unix time in seconds.
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open(
            &std::env::temp_dir().join(format!("search-test-{}", uuid::Uuid::new_v4())),
            crate::config::IndexSettings::default(),
        )
        .expect("open store")
    }

    #[test]
    fn a_stored_page_is_findable_offline() {
        let store = store();
        store
            .put(&Doc {
                url: "https://example.com/p".into(),
                title: "Unique Title".into(),
                text: "a distinctive phrase about widgets".into(),
                host: "example.com".into(),
                fetched_at: now_secs(),
            })
            .unwrap();
        let hits = store.search("distinctive widgets", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://example.com/p");
    }

    #[test]
    fn punctuation_in_a_query_does_not_error() {
        let store = store();
        store
            .put(&Doc {
                url: "https://example.com/p".into(),
                title: "T".into(),
                text: "body".into(),
                host: "example.com".into(),
                fetched_at: now_secs(),
            })
            .unwrap();
        assert!(store.search("a: b* OR", 5).is_ok());
    }

    #[test]
    fn stats_count_documents_and_hosts() {
        let store = store();
        for (url, host) in [
            ("https://a.example/1", "a.example"),
            ("https://b.example/2", "b.example"),
        ] {
            store
                .put(&Doc {
                    url: url.into(),
                    title: "t".into(),
                    text: "body".into(),
                    host: host.into(),
                    fetched_at: now_secs(),
                })
                .unwrap();
        }
        let stats = store.stats().unwrap();
        assert_eq!(stats.documents, 2);
        assert_eq!(stats.hosts, 2);
    }

    /// A document older than the age bound is pruned, so the corpus cannot
    /// accumulate stale pages forever.
    #[test]
    fn aged_documents_are_pruned() {
        let dir = std::env::temp_dir().join(format!("search-age-{}", uuid::Uuid::new_v4()));
        let settings = crate::config::IndexSettings {
            save_fetched_pages: None,
            include_in_search: None,
            retention_days: 1,
            ..Default::default()
        };
        let store = Store::open(&dir, settings).expect("open");
        store
            .put(&Doc {
                url: "https://old.example/p".into(),
                title: "old".into(),
                text: "stale content".into(),
                host: "old.example".into(),
                fetched_at: now_secs() - 3 * 86_400,
            })
            .unwrap();
        assert_eq!(store.stats().unwrap().documents, 0, "old doc pruned");
    }

    /// The corpus stays under its byte ceiling by evicting the oldest first.
    #[test]
    fn the_byte_ceiling_evicts_oldest_first() {
        let dir = std::env::temp_dir().join(format!("search-size-{}", uuid::Uuid::new_v4()));
        let settings = crate::config::IndexSettings {
            save_fetched_pages: None,
            include_in_search: None,
            max_size_mb: 1,
            enabled: true,
            retention_days: 0,
            refresh_hosts: Vec::new(),
            refresh_interval_days: 7,
        };
        let store = Store::open(&dir, settings).expect("open");
        // Two documents, each about 0.6 MB of text, exceed the 1 MB ceiling.
        for i in 0..2 {
            store
                .put(&Doc {
                    url: format!("https://e.example/{i}"),
                    title: "t".into(),
                    text: format!(
                        "{} {}",
                        "filler ".repeat(90_000),
                        if i == 1 { "newer" } else { "older" }
                    ),
                    host: "e.example".into(),
                    fetched_at: now_secs() + i,
                })
                .unwrap();
        }
        let stats = store.stats().unwrap();
        assert!(stats.bytes <= 1024 * 1024, "over ceiling: {}", stats.bytes);
        // The newer document survives; the older is evicted.
        assert_eq!(stats.documents, 1);
        assert_eq!(store.search("newer", 5).unwrap().len(), 1);
        assert!(store.search("older", 5).unwrap().is_empty());
    }

    /// Text longer than the cap is stored from its opening only.
    #[test]
    fn stored_text_is_capped() {
        assert_eq!(cap_chars("abcdef", 3), "abc");
        assert_eq!(cap_chars("ab", 10), "ab");
        // On a character boundary, not a byte one.
        assert_eq!(cap_chars("héllo", 2), "hé");
    }
}
