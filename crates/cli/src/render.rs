//! Render engine responses as compact terminal text or machine-readable JSON.

use search::{
    discovery::{ProviderStatus, Response},
    fetch::Fetched,
    index::Hit,
};

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

/// Render web results and provider health for a person at a terminal.
pub fn search(response: &Response) -> i32 {
    for (rank, finding) in response.results.iter().enumerate() {
        println!("{}. {}\n   {}", rank + 1, finding.title, finding.url);
        if let Some(snippet) = &finding.snippet {
            println!("   {snippet}");
        }
        println!();
    }
    eprintln!(
        "{} results · {} ms",
        response.results.len(),
        response.duration_ms
    );
    for provider in &response.providers {
        if provider.status != ProviderStatus::Ok {
            let status = match provider.status {
                ProviderStatus::Error => "failed",
                ProviderStatus::Timeout => "timed out",
                ProviderStatus::Cancelled => "cancelled",
                ProviderStatus::Ok => continue,
            };
            eprintln!(
                "{}: {status}{}",
                provider.name,
                provider
                    .error
                    .as_ref()
                    .map(|error| format!(": {error}"))
                    .unwrap_or_default()
            );
        }
    }
    0
}

/// Render fetched page text, while keeping fetch failures visible.
pub fn fetch(results: &[Fetched]) -> i32 {
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

/// Render local corpus results.
pub fn hits(results: &[Hit]) -> i32 {
    for (rank, hit) in results.iter().enumerate() {
        println!(
            "{}. {}\n   {}\n   {}\n",
            rank + 1,
            hit.title,
            hit.url,
            hit.snippet
        );
    }
    0
}

fn fail(message: String) -> i32 {
    eprintln!("search: {message}");
    1
}
