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
            json,
            config,
        } => match crate::cli::engines::command(&action, args, json, config).await {
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
            offset,
            json,
            config,
        } => fetch_command(urls, query, max_characters, offset, json, config).await,
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
    let mut request = crate::core::Query {
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
    offset: usize,
    json: bool,
    config_path: Option<String>,
) -> i32 {
    if max_characters == Some(0) {
        return report_error("max-chars must be at least 1".into());
    }
    let service = match build_client(config_path) {
        Ok(service) => service,
        Err(error) => return report_error(error),
    };
    let pages = match service.fetch(&urls).await {
        Ok(pages) => pages,
        Err(error) => return report_error(error),
    };
    let reading = crate::core::text::Focus {
        query,
        max_characters,
        offset,
    };
    let results: Vec<_> = pages
        .into_iter()
        .map(|page| crate::core::text::focus(page, &reading))
        .collect();
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
