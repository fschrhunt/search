//! Dispatch and the serve loop.

use std::sync::Arc;

use search::config;
use search::Search;
use search_mcp::backend::{Backend, Operation};

use crate::{args::Command, http, stdio};

/// Run one parsed command. Returns the process exit code.
pub async fn execute(command: Command) -> i32 {
    match command {
        Command::Remote {
            action,
            args,
            config,
        } => match crate::remote::command(&action, args, config).await {
            Ok(()) => 0,
            Err(error) => report_error(error),
        },
        Command::Version => {
            println!("{}", search::VERSION);
            0
        }
        Command::Help => {
            println!("{}", crate::args::usage());
            0
        }
        Command::Stdio { config } => stdio::serve(config).await,
        Command::Serve {
            config,
            address,
            hostname,
            dir,
        } => http::serve(config, address, hostname, dir).await,
        Command::Search {
            query,
            limit,
            json,
            providers,
            config,
        } => search_command(query, limit, json, providers, config).await,
        Command::Fetch {
            urls,
            query,
            max_characters,
            json,
            config,
        } => fetch_command(urls, query, max_characters, json, config).await,
        Command::Index {
            query,
            limit,
            json,
            config,
        } => index_command(query, limit, json, config).await,
        Command::Refresh { config } => refresh_command(config).await,
    }
}

async fn search_command(
    query: String,
    limit: usize,
    json: bool,
    providers: Vec<String>,
    config_path: Option<String>,
) -> i32 {
    let service = match build_backend(config_path) {
        Ok(service) => service,
        Err(error) => return report_error(error),
    };
    let mut request = search::discovery::Query {
        text: query,
        ..Default::default()
    };
    request.limit = limit;
    if !providers.is_empty() {
        request.providers = providers;
    }
    let response = match service.search(request).await {
        Ok(response) => response,
        Err(error) => return report_error(error),
    };
    if json {
        crate::render::json(&response)
    } else {
        crate::render::search(&response)
    }
}

async fn fetch_command(
    urls: Vec<String>,
    query: Option<String>,
    max_characters: Option<usize>,
    json: bool,
    config_path: Option<String>,
) -> i32 {
    let service = match build_backend(config_path) {
        Ok(service) => service,
        Err(error) => return report_error(error),
    };
    let mut results = match service.fetch(&urls).await {
        Ok(results) => results,
        Err(error) => return report_error(error),
    };
    for result in &mut results {
        if let Some(focus) = query.as_deref() {
            result.text = search::text::select(&result.text, focus, max_characters.unwrap_or(4000))
                .into_iter()
                .map(|passage| passage.text)
                .collect::<Vec<_>>()
                .join("\n\n");
        } else if let Some(max) = max_characters {
            result.text = result.text.chars().take(max).collect();
        }
    }
    if json {
        crate::render::json(&results)
    } else {
        crate::render::fetch(&results)
    }
}

async fn index_command(
    query: String,
    limit: usize,
    json: bool,
    config_path: Option<String>,
) -> i32 {
    let service = match build_backend(config_path) {
        Ok(service) => service,
        Err(error) => return report_error(error),
    };
    match service
        .execute(Operation::Index { query, limit })
        .await
        .and_then(|value| {
            serde_json::from_value::<Vec<search::index::Hit>>(value).map_err(|e| e.to_string())
        }) {
        Ok(results) => {
            if json {
                crate::render::json(&results)
            } else {
                crate::render::hits(&results)
            }
        }
        Err(error) => report_error(error.to_string()),
    }
}

async fn refresh_command(config_path: Option<String>) -> i32 {
    let service = match build_backend(config_path) {
        Ok(service) => service,
        Err(error) => return report_error(error),
    };
    let refreshed = match service.execute(Operation::Refresh).await {
        Ok(value) => value,
        Err(error) => return report_error(error),
    };
    println!("Refreshed {refreshed} document(s)");
    0
}

fn report_error(error: String) -> i32 {
    eprintln!("search: {error}");
    1
}

/// Select a remote before opening local storage; selected failures are errors.
pub(crate) fn build_backend(config_path: Option<String>) -> Result<Backend, String> {
    if let Some(remote) = crate::remote::selected(config_path.clone())? {
        return Ok(remote);
    }
    build_service(config_path, None, None).map(Backend::Local)
}

/// Load configuration and build the service, applying serve overrides before
/// validating the effective settings.
pub(super) fn build_service(
    config_path: Option<String>,
    address: Option<String>,
    dir: Option<String>,
) -> Result<Arc<Search>, String> {
    let path = config_path.map(std::path::PathBuf::from);
    let mut settings = config::load(path).map_err(|e| e.message().to_string())?;
    if let Some(address) = address {
        settings.address = address;
    }
    if let Some(dir) = dir {
        settings.dir = std::path::PathBuf::from(dir);
    }
    // Overrides bypassed validation at load time, so re-check the effective
    // settings before binding.
    settings.validate().map_err(|e| e.message().to_string())?;
    let service = Search::open(settings).map_err(|e| e.to_string())?;
    Ok(Arc::new(service))
}
