//! Render engine responses as compact terminal text or machine-readable JSON.

use crate::core::engines::EngineStatus;
use crate::core::text::FocusedPage;
use crate::core::Answer;

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

/// Render web results and engine health for a person at a terminal: failed,
/// timed-out, and row-skipping engines are reported on stderr.
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
            EngineStatus::Ok if engine.skipped > 0 => "ok",
            EngineStatus::Ok => continue,
        };
        let skipped = match engine.skipped {
            0 => String::new(),
            count => format!(", skipped {count} invalid results"),
        };
        eprintln!(
            "{}: {status}{skipped}{}",
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

/// Render fetched page text. A failed URL is printed in place, so its result stays
/// with its position; the caller decides the exit status.
pub fn fetch(results: &[FocusedPage]) {
    for read in results {
        let page = &read.page;
        println!("## {}", page.title.as_deref().unwrap_or(&page.url));
        println!("{}\n", page.final_url.as_deref().unwrap_or(&page.url));
        if let Some(error) = &page.error {
            println!("Fetch failed: {error}\n");
            continue;
        }
        match &read.passages {
            Some(passages) => {
                let texts: Vec<&str> = passages.iter().map(|p| p.text.as_str()).collect();
                println!("{}\n", texts.join("\n\n…\n\n"));
            }
            None => println!("{}\n", page.text),
        }
        if let Some(offset) = read.next_offset {
            println!("[continues; read on with -offset {offset}]\n");
        } else if page.truncated == Some(true) {
            println!("[truncated at fetch.max_response_bytes]\n");
        }
    }
}

fn fail(message: String) -> i32 {
    eprintln!("search: {message}");
    1
}
