//! Dispatch and the serve loop.

use std::sync::Arc;

use crate::client::Client;
use crate::core::config;
use crate::core::Search;

use crate::cli::{args::Command, http, stdio};

/// Run one parsed command. Returns the process exit code.
pub async fn execute(command: Command) -> i32 {
    match command {
        Command::Engines {
            action,
            args,
            config,
        } => match crate::cli::engines::command(&action, args, config).await {
            Ok(()) => 0,
            Err(error) => report_error(error),
        },
        Command::Remote {
            action,
            args,
            config,
        } => match crate::cli::remote::command(&action, args, config).await {
            Ok(()) => 0,
            Err(error) => report_error(error),
        },
        Command::Version => {
            println!("{}", crate::VERSION);
            0
        }
        Command::Help => {
            println!("{}", crate::cli::args::usage());
            0
        }
        Command::Stdio { config } => stdio::serve(config).await,
        Command::Serve {
            config,
            address,
            hostname,
        } => http::serve(config, address, hostname).await,
        Command::Search {
            query,
            limit,
            json,
            engines,
            config,
        } => search_command(query, limit, json, engines, config).await,
        Command::Fetch {
            urls,
            query,
            max_characters,
            json,
            config,
        } => fetch_command(urls, query, max_characters, json, config).await,
    }
}

async fn search_command(
    query: String,
    limit: usize,
    json: bool,
    engines: Vec<String>,
    config_path: Option<String>,
) -> i32 {
    let service = match build_client(config_path) {
        Ok(service) => service,
        Err(error) => return report_error(error),
    };
    let mut request = crate::engines::Query {
        text: query,
        ..Default::default()
    };
    request.limit = limit;
    if !engines.is_empty() {
        request.engines = engines;
    }
    let response = match service.search(request).await {
        Ok(response) => response,
        Err(error) => return report_error(error),
    };
    if json {
        crate::cli::render::json(&response)
    } else {
        crate::cli::render::search(&response)
    }
}

async fn fetch_command(
    urls: Vec<String>,
    query: Option<String>,
    max_characters: Option<usize>,
    json: bool,
    config_path: Option<String>,
) -> i32 {
    if max_characters.is_some_and(|limit| limit == 0 || limit > 40_000) {
        return report_error("max-chars must be between 1 and 40000".into());
    }
    let service = match build_client(config_path) {
        Ok(service) => service,
        Err(error) => return report_error(error),
    };
    let mut results = match service.fetch(&urls).await {
        Ok(results) => results,
        Err(error) => return report_error(error),
    };
    for result in &mut results {
        if let Some(focus) = query.as_deref().filter(|query| !query.trim().is_empty()) {
            let budget = max_characters.unwrap_or(crate::core::text::DEFAULT_BUDGET);
            let found = crate::core::text::select(&result.text, focus, budget);
            if !found.iter().any(|passage| passage.score > 0.0) {
                if result.text.chars().count() > budget {
                    result.truncated = Some(true);
                }
                result.text = result.text.chars().take(budget).collect();
                continue;
            }
            result.text = found
                .into_iter()
                .map(|passage| passage.text)
                .collect::<Vec<_>>()
                .join("\n\n");
        } else if let Some(max) = max_characters {
            if result.text.chars().count() > max {
                result.truncated = Some(true);
            }
            result.text = result.text.chars().take(max).collect();
        }
    }
    if json {
        crate::cli::render::json(&results)
    } else {
        crate::cli::render::fetch(&results)
    }
}

fn report_error(error: String) -> i32 {
    eprintln!("search: {error}");
    1
}

/// Select a remote before opening the local engine; selected failures are errors.
pub(crate) fn build_client(config_path: Option<String>) -> Result<Client, String> {
    if let Some(remote) = crate::cli::remote::selected(config_path.clone())? {
        return Ok(remote);
    }
    build_service(config_path, None).map(Client::Local)
}

/// Load configuration and build the service, applying serve overrides before
/// validating the effective settings.
pub(super) fn build_service(
    config_path: Option<String>,
    address: Option<String>,
) -> Result<Arc<Search>, String> {
    let path = config_path.map(std::path::PathBuf::from);
    let mut settings = config::load(path).map_err(|e| e.message().to_string())?;
    if let Some(address) = address {
        settings.address = address;
    }
    // Overrides bypassed validation at load time, so re-check the effective
    // settings before binding.
    settings.validate().map_err(|e| e.message().to_string())?;
    let service = Search::open(settings).map_err(|e| e.to_string())?;
    Ok(Arc::new(service))
}
