//! Render engine responses as compact terminal text or machine-readable JSON.

use crate::core::{Answer, Page};
use crate::engines::EngineStatus;

/// Print any serializable response as JSON; return a process-style status.
pub fn json<T: serde::Serialize>(value: &T) -> i32 {
    match serde_json::to_string_pretty(value) {
        Ok(output) => {
            println!("{output}");
            0
        }
        Err(error) => fail(error.to_string()),
    }
}

/// Render web results and engine health for a person at a terminal.
pub fn search(answer: &Answer) -> i32 {
    for (rank, link) in answer.results.iter().enumerate() {
        println!("{}. {}\n   {}", rank + 1, link.title, link.url);
        if let Some(snippet) = &link.snippet {
            println!("   {snippet}");
        }
        println!();
    }
    eprintln!(
        "{} results · {} ms",
        answer.results.len(),
        answer.duration_ms
    );
    for engine in &answer.engines {
        let status = match engine.status {
            EngineStatus::Error => "failed",
            EngineStatus::Timeout => "timed out",
            EngineStatus::Ok => continue,
        };
        eprintln!(
            "{}: {status}{}",
            engine.name,
            engine
                .error
                .as_ref()
                .map(|error| format!(": {error}"))
                .unwrap_or_default()
        );
    }
    0
}

/// Render fetched page text, while keeping fetch failures visible.
pub fn fetch(results: &[Page]) -> i32 {
    for page in results {
        println!("## {}", page.title.as_deref().unwrap_or(&page.url));
        println!("{}\n", page.final_url.as_deref().unwrap_or(&page.url));
        if let Some(error) = &page.error {
            println!("Fetch failed: {error}\n");
        } else {
            println!("{}\n", page.text);
        }
    }
    if results.iter().any(|page| page.error.is_some()) {
        1
    } else {
        0
    }
}

fn fail(message: String) -> i32 {
    eprintln!("search: {message}");
    1
}
