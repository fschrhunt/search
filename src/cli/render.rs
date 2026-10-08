//! Render responses as control-free terminal text or unchanged machine-readable JSON.

use crate::core::engines::EngineStatus;
use crate::core::text::FocusedPage;
use crate::core::Answer;

/// Remove terminal and Unicode direction controls from human text, retaining
/// newlines and tabs for page layout. Never apply this to machine-readable data.
pub(super) fn terminal_text(value: &str) -> String {
    value
        .chars()
        .filter(|&c| {
            (c == '\n' || c == '\t' || !c.is_control())
                && !matches!(c, '\u{061c}' | '\u{200e}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .collect()
}

/// Print any serializable response as JSON; return a process-style status.
pub fn json<T: serde::Serialize>(value: &T) -> i32 {
    match serde_json::to_string_pretty(value) {
        Ok(output) => {
            println!("{output}");
            0
        }
        Err(message) => error(message.to_string()),
    }
}

/// Render web results and engine health for a person at a terminal: failed,
/// timed-out, and row-skipping engines are reported on stderr.
pub fn search(answer: &Answer) -> i32 {
    for (rank, link) in answer.results.iter().enumerate() {
        println!(
            "{}. {}\n   {}",
            rank + 1,
            terminal_text(&link.title),
            terminal_text(&link.url)
        );
        if let Some(snippet) = &link.snippet {
            println!("   {}", terminal_text(snippet));
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
            terminal_text(&engine.name),
            engine
                .error
                .as_ref()
                .map(|error| format!(": {}", terminal_text(error)))
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
        println!(
            "## {}",
            terminal_text(page.title.as_deref().unwrap_or(&page.url))
        );
        println!(
            "{}\n",
            terminal_text(page.final_url.as_deref().unwrap_or(&page.url))
        );
        if let Some(error) = &page.error {
            println!("Fetch failed: {}\n", terminal_text(error));
            continue;
        }
        match &read.passages {
            Some(passages) => {
                let texts: Vec<String> = passages
                    .iter()
                    .map(|p| terminal_text(p.text.as_str()))
                    .collect();
                println!("{}\n", texts.join("\n\n…\n\n"));
            }
            None => println!("{}\n", terminal_text(&page.text)),
        }
        if let Some(offset) = read.next_offset {
            println!("[continues; read on with -offset {offset}]\n");
        } else if page.truncated == Some(true) {
            println!("[truncated at fetch.max_response_bytes]\n");
        }
    }
}

/// Print a human diagnostic without terminal controls and return failure status.
pub fn error(message: String) -> i32 {
    eprintln!("search: {}", terminal_text(&message));
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// OSC52, CSI, C1 equivalents, and cursor controls lose their control bytes.
    #[test]
    fn terminal_text_neutralizes_terminal_actions() {
        let attack = "title\x1b]52;c;Y2xpcA==\x07\x1b[2J\x1b[H\r\x08\x00\x7f\u{009b}31m\u{009d}52;c;data\u{009c}";
        let rendered = terminal_text(attack);
        assert_eq!(rendered, "title]52;c;Y2xpcA==[2J[H31m52;c;data");
        assert!(!rendered.chars().any(char::is_control));
    }

    /// Unicode direction overrides are removed without damaging readable Unicode or layout.
    #[test]
    fn terminal_text_preserves_text_layout_without_direction_controls() {
        let text = "café 日本語 👩\u{200d}💻\n\tparagraph";
        assert_eq!(terminal_text(text), text);
        assert_eq!(
            terminal_text("a\u{061c}\u{200e}\u{200f}\u{202a}\u{202b}\u{202c}\u{202d}\u{202e}\u{2066}\u{2067}\u{2068}\u{2069}b"),
            "ab"
        );
    }
}
