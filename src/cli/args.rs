//! The command line: what `search` accepts and how it dispatches.
//!
//! `search QUERY` is the
//! human and script surface; `serve` is the HTTP and MCP surface; a bare
//! `search` with no query is the stdio MCP surface a harness spawns. Keeping
//! "no arguments" as MCP means an existing harness keeps working while a person
//! gets a real search command. Engine verbs edit and test the engines defined in
//! settings. A leading `-config PATH` applies to whatever command follows.

/// One parsed command.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// List, select, configure or explicitly test settings-defined engines.
    Engines {
        action: String,
        args: Vec<String>,
        json: bool,
        config: Option<String>,
    },
    /// Serve MCP over stdio (the default with no arguments, and what a harness
    /// invokes).
    Stdio { config: Option<String> },
    /// Serve the JSON API and MCP over HTTP.
    Serve {
        config: Option<String>,
        address: Option<String>,
        hostname: Option<String>,
    },
    /// Search the web, printing results for a person or JSON for a script.
    Search {
        query: String,
        /// Zero lets the selected service use its configured result maximum.
        limit: usize,
        json: bool,
        engines: Vec<String>,
        config: Option<String>,
    },
    /// Read one or more URLs.
    Fetch {
        urls: Vec<String>,
        query: Option<String>,
        max_characters: Option<usize>,
        /// Character to start reading at, from an earlier continuation.
        offset: usize,
        json: bool,
        config: Option<String>,
    },
    /// Manage paired hosts or devices on this host.
    Remote {
        action: String,
        args: Vec<String>,
        config: Option<String>,
    },
    /// Print the version.
    Version,
    /// Print usage.
    Help,
}

/// Parse arguments. An unknown first token that is not a flag is treated as a
/// search query, so `search "rust async"` works; only a recognized verb takes a
/// subcommand's flags.
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Command, String> {
    let mut args = args.into_iter().peekable();
    if matches!(
        args.peek().map(String::as_str),
        Some("-config" | "--config")
    ) {
        let _ = args.next();
        let path = args.next().ok_or("missing value for -config")?;
        let rest: Vec<String> = args.collect();
        // Bare `search -config PATH` keeps harness configs on stdio MCP.
        if rest.is_empty() {
            return Ok(Command::Stdio { config: Some(path) });
        }
        let mut command = parse(rest)?;
        if let Some(config) = command.config_mut() {
            if config.is_some() {
                return Err("-config given twice".into());
            }
            *config = Some(path);
        }
        return Ok(command);
    }
    match args.peek().map(String::as_str) {
        None => Ok(Command::Stdio { config: None }),
        Some("stdio") => {
            let _ = args.next();
            Ok(Command::Stdio {
                config: parse_config_flag(args)?,
            })
        }
        Some("serve") => {
            let _ = args.next();
            let flags = parse_serve_flags(args)?;
            Ok(Command::Serve {
                config: flags.config,
                address: flags.address,
                hostname: flags.hostname,
            })
        }
        Some("fetch") => {
            let _ = args.next();
            parse_fetch(args)
        }
        Some("engines") => {
            let _ = args.next();
            parse_engines(args)
        }
        Some("enable" | "disable" | "configure" | "test") => parse_engines(args),
        // Package commands were removed; say so instead of searching for the words.
        Some(verb @ ("install" | "update" | "remove")) => Err(format!(
            "search {verb} was removed: engines are defined in settings; see search engines \
             and docs/engines.md (use search -- {verb} ... to search for these words)"
        )),
        Some("remote") | Some("devices") | Some("revoke") | Some("pair-code") => {
            let verb = args.next().ok_or("missing command")?;
            let action = if verb == "remote" {
                args.next().ok_or("missing remote command")?
            } else {
                verb
            };
            let mut config = None;
            let mut values = Vec::new();
            while let Some(value) = args.next() {
                if value == "--config" || value == "-config" {
                    config = Some(args.next().ok_or("missing config path")?);
                } else {
                    values.push(value);
                }
            }
            Ok(Command::Remote {
                action,
                args: values,
                config,
            })
        }
        Some("version") | Some("--version") | Some("-version") => Ok(Command::Version),
        Some("help") | Some("--help") | Some("-h") => Ok(Command::Help),
        Some(flag) if flag.starts_with('-') && flag != "--" => parse_search(args),
        // Anything else is a search query.
        Some(_) => parse_search(args),
    }
}

/// Concise command reference; examples use primary verbs rather than flag aliases.
pub fn usage() -> &'static str {
    "search — web search and clean page reading

Start:
  search \"rust async\"            search with the keyless Mwmbl default
  search fetch https://example.com/  read a public page as clean text
  search help                    show this reference
  search version                 print the release version
  search -- test driven development  search a literal query starting with a command

Engines:
  search engines [list] [-json]   built-in and configured engines, selection and setup status
  search configure ID             show an engine's effective adapter and status
  search configure ID FIELD VALUE set one adapter field in engines.config.ID
  search enable ID                select a ready engine
  search disable ID               deselect an engine
  search test ID QUERY [-json]    run one engine explicitly and print its results
  Built-in: mwmbl (keyless default), searxng (needs url). Any other ID is a custom
  http or command adapter in engines.config. Engine commands always run locally.
  Settings are trusted: a command engine runs that program as you. Reference
  credentials by environment variable name (env/header_env), never by value.

Hosting:
  search serve [flags]            host the paired HTTPS JSON API and MCP endpoint
  search stdio [-config PATH]     serve MCP over stdio (also the no-argument default)
  search remote pair NAME HTTPS_URL CERT_FILE SHA256  pair; code read from stdin
  search remote use NAME          route CLI and stdio MCP to a paired host
  search remote list              inspect saved hosts
  search remote off               return to local execution
  search remote remove NAME       forget a saved host
  search pair-code               renew this host's one-use pairing code
  search devices                 list this host's paired devices
  search revoke DEVICE_ID         revoke a device on this host

Serve flags:
  -address HOST:PORT              listener (default 127.0.0.1:8642)
  -hostname NAME                  certificate name; required for wildcard binds

Search flags:
  -limit N                       result count (default from settings, normally 10)
  -engines A,B                    restrict to selected engine IDs
  -json                          print JSON instead of text

Fetch flags:
  -query TEXT                    relevant passages; fall back to text if none match
  -max-chars N                    per-page character limit (default: whole page)
  -offset N                       continue reading at character N
  -json                          print JSON instead of text
  Without query or character limit, return the full clean page.
  Focused reads default to 6000 characters per page.

Settings:
  -config PATH                    settings file; before or after the command/query
  Default: $CONFIG or $SEARCH_HOME/settings.json (SEARCH_HOME defaults to ~/.search).
  Double-dash flag forms are accepted. Use -- before literal query/value arguments.
  Guide: https://github.com/fschrhunt/search/tree/main/docs"
}

/// Parse engine commands, extracting flags before literal `--` arguments.
/// A bare `search engines` lists engines.
fn parse_engines<I: IntoIterator<Item = String>>(args: I) -> Result<Command, String> {
    let mut args = args.into_iter().peekable();
    let action = match args.peek().map(String::as_str) {
        None => "list".to_owned(),
        Some(flag) if flag.starts_with('-') => "list".to_owned(),
        Some(_) => args.next().ok_or("expected an engines command")?,
    };
    let mut config = None;
    let mut json = false;
    let mut values = Vec::new();
    let mut literal = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--" if !literal => literal = true,
            "-config" | "--config" if !literal => {
                config = Some(args.next().ok_or("missing value for -config")?);
            }
            "-json" | "--json" if !literal && matches!(action.as_str(), "list" | "test") => {
                json = true
            }
            flag if !literal && flag.starts_with('-') => {
                return Err(format!("unknown engine flag {flag:?}"));
            }
            _ => values.push(arg),
        }
    }
    let valid = match action.as_str() {
        "list" => values.is_empty(),
        "enable" | "disable" => values.len() == 1,
        "configure" => values.len() == 1 || values.len() == 3,
        "test" => values.len() >= 2 && !values.iter().skip(1).all(|s| s.trim().is_empty()),
        _ => false,
    };
    if !valid {
        return Err("invalid engines command or arguments; see search help".into());
    }
    Ok(Command::Engines {
        action,
        args: values,
        json,
        config,
    })
}

impl Command {
    /// The settings-path slot of commands that read settings.
    fn config_mut(&mut self) -> Option<&mut Option<String>> {
        match self {
            Command::Engines { config, .. }
            | Command::Stdio { config }
            | Command::Serve { config, .. }
            | Command::Search { config, .. }
            | Command::Fetch { config, .. }
            | Command::Remote { config, .. } => Some(config),
            Command::Version | Command::Help => None,
        }
    }
}

fn parse_config_flag<I: IntoIterator<Item = String>>(args: I) -> Result<Option<String>, String> {
    let mut config = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-config" | "--config" => {
                config = Some(args.next().ok_or("missing value for -config")?);
            }
            other => return Err(format!("unknown flag {other:?}")),
        }
    }
    Ok(config)
}

fn parse_search<I: IntoIterator<Item = String>>(args: I) -> Result<Command, String> {
    let mut query_words = Vec::new();
    let mut limit = 0usize;
    let mut json = false;
    let mut engines = Vec::new();
    let mut config = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--" => {
                query_words.extend(args);
                break;
            }
            "-limit" | "--limit" => {
                limit = args
                    .next()
                    .ok_or("missing value for -limit")?
                    .parse()
                    .map_err(|_| "-limit needs a number".to_string())?;
            }
            "-json" | "--json" => json = true,
            "-engines" | "--engines" => {
                let value = args.next().ok_or("missing value for -engines")?;
                engines = value
                    .split(',')
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect();
            }
            "-config" | "--config" => {
                config = Some(args.next().ok_or("missing value for -config")?)
            }
            other if other.starts_with('-') => return Err(format!("unknown flag {other:?}")),
            word => query_words.push(word.to_string()),
        }
    }
    if query_words.is_empty() {
        return Err("a search needs a query".into());
    }
    Ok(Command::Search {
        query: query_words.join(" "),
        limit,
        json,
        engines,
        config,
    })
}

fn parse_fetch<I: IntoIterator<Item = String>>(args: I) -> Result<Command, String> {
    let mut urls = Vec::new();
    let mut query = None;
    let mut max_characters = None;
    let mut offset = 0;
    let mut json = false;
    let mut config = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-query" | "--query" => query = Some(args.next().ok_or("missing value for -query")?),
            "-max-chars" | "--max-chars" => {
                max_characters = Some(
                    args.next()
                        .ok_or("missing value for -max-chars")?
                        .parse()
                        .map_err(|_| "-max-chars needs a number".to_string())?,
                );
            }
            "-offset" | "--offset" => {
                offset = args
                    .next()
                    .ok_or("missing value for -offset")?
                    .parse()
                    .map_err(|_| "-offset needs a number".to_string())?;
            }
            "-json" | "--json" => json = true,
            "-config" | "--config" => {
                config = Some(args.next().ok_or("missing value for -config")?)
            }
            other if other.starts_with('-') => return Err(format!("unknown flag {other:?}")),
            url => urls.push(url.to_string()),
        }
    }
    if urls.is_empty() {
        return Err("fetch needs at least one URL".into());
    }
    Ok(Command::Fetch {
        urls,
        query,
        max_characters,
        offset,
        json,
        config,
    })
}

fn parse_serve_flags<I: IntoIterator<Item = String>>(args: I) -> Result<ServeFlags, String> {
    let mut flags = ServeFlags::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-config" | "--config" => {
                flags.config = Some(args.next().ok_or("missing value for -config")?)
            }
            "-address" | "--address" => {
                flags.address = Some(args.next().ok_or("missing value for -address")?)
            }
            "-hostname" | "--hostname" => {
                flags.hostname = Some(args.next().ok_or("missing value for -hostname")?)
            }
            other => return Err(format!("unknown flag {other:?}")),
        }
    }
    Ok(flags)
}

/// The serve-only flags, named so the parse signature stays readable.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ServeFlags {
    pub config: Option<String>,
    pub address: Option<String>,
    pub hostname: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn engine_commands_extract_flags_and_preserve_literal_query() {
        assert_eq!(
            parse(args(&[
                "engines",
                "test",
                "fixture",
                "--config",
                "x.json",
                "-json",
                "--",
                "-site:example.com",
                "rust"
            ]))
            .unwrap(),
            Command::Engines {
                action: "test".into(),
                args: vec!["fixture".into(), "-site:example.com".into(), "rust".into()],
                json: true,
                config: Some("x.json".into())
            }
        );
        for values in [
            vec!["engines", "test", "fixture"],
            vec!["engines", "list", "fixture"],
            vec!["engines", "configure", "fixture", "url"],
            vec!["engines", "enable", "fixture", "-json"],
            vec!["engines", "install", "fixture"],
            vec!["engines", "list", "--home", "/tmp"],
        ] {
            assert!(parse(args(&values)).is_err(), "{values:?}");
        }
        for listing in [vec!["engines"], vec!["engines", "-json"]] {
            assert!(matches!(
                parse(args(&listing)).unwrap(),
                Command::Engines { action, .. } if action == "list"
            ));
        }
    }

    #[test]
    fn top_level_engine_commands_route_like_their_aliases() {
        for values in [
            vec!["enable", "fixture"],
            vec!["disable", "fixture"],
            vec!["configure", "fixture", "url", "https://example.com"],
            vec!["test", "fixture", "--", "-config", "literal"],
        ] {
            let command = parse(args(&values)).unwrap();
            assert!(matches!(&command, Command::Engines { action, .. } if action == values[0]));
            let mut alias = vec!["engines"];
            alias.extend(&values);
            assert_eq!(command, parse(args(&alias)).unwrap());
        }
    }

    /// Removed package commands explain themselves instead of becoming searches,
    /// and `--` still searches for the words.
    #[test]
    fn removed_package_commands_are_errors() {
        for verb in ["install", "update", "remove"] {
            let error = parse(args(&[verb, "searxng"])).unwrap_err();
            assert!(error.contains("removed"), "{error}");
        }
        assert!(matches!(
            parse(args(&["--", "install", "rust"])).unwrap(),
            Command::Search { query, .. } if query == "install rust"
        ));
    }

    /// A leading `-config PATH` applies to the command after it.
    #[test]
    fn leading_config_applies_to_the_following_command() {
        assert_eq!(
            parse(args(&["-config", "s.json", "fetch", "https://a.example"])).unwrap(),
            Command::Fetch {
                urls: vec!["https://a.example".into()],
                query: None,
                max_characters: None,
                offset: 0,
                json: false,
                config: Some("s.json".into()),
            }
        );
        assert!(matches!(
            parse(args(&["--config", "s.json", "rust", "async"])).unwrap(),
            Command::Search { query, config: Some(path), .. } if query == "rust async" && path == "s.json"
        ));
        assert!(parse(args(&["-config", "a.json", "engines", "-config", "b.json"])).is_err());
    }

    #[test]
    fn literal_search_queries_can_contain_command_names_and_flags() {
        assert_eq!(
            parse(args(&["--", "test", "-engines", "literal"])).unwrap(),
            Command::Search {
                query: "test -engines literal".into(),
                limit: 0,
                json: false,
                engines: vec![],
                config: None,
            }
        );
    }

    #[test]
    fn search_selection_uses_engine_flags() {
        assert!(matches!(
            parse(args(&["rust", "-engines", "mwmbl, custom"])).unwrap(),
            Command::Search { engines, .. } if engines == vec!["mwmbl", "custom"]
        ));
        assert!(parse(args(&["rust", "-providers", "mwmbl"])).is_err());
    }

    #[test]
    fn no_arguments_means_stdio() {
        assert_eq!(parse(args(&[])).unwrap(), Command::Stdio { config: None });
    }

    #[test]
    fn a_query_is_a_search() {
        assert_eq!(
            parse(args(&["rust", "async", "-limit", "5"])).unwrap(),
            Command::Search {
                query: "rust async".into(),
                limit: 5,
                json: false,
                engines: vec![],
                config: None,
            }
        );
    }

    #[test]
    fn search_limits_default_to_settings_and_preserve_explicit_values() {
        assert!(matches!(
            parse(args(&["rust"])).unwrap(),
            Command::Search { limit: 0, .. }
        ));
        for limit in ["0", "1", "75"] {
            assert!(matches!(
                parse(args(&["rust", "-limit", limit])).unwrap(),
                Command::Search { limit: parsed, .. } if parsed == limit.parse::<usize>().unwrap()
            ));
        }
    }

    #[test]
    fn serve_reads_its_flags() {
        assert_eq!(
            parse(args(&["serve", "-address", "0.0.0.0:1"])).unwrap(),
            Command::Serve {
                config: None,
                address: Some("0.0.0.0:1".into()),
                hostname: None,
            }
        );
    }

    #[test]
    fn serve_accepts_both_hostname_flags_and_requires_a_value() {
        for flag in ["-hostname", "--hostname"] {
            assert!(
                matches!(parse(args(&["serve", flag, "search.example"])).unwrap(),
                Command::Serve { hostname: Some(name), .. } if name == "search.example")
            );
            assert!(parse(args(&["serve", flag])).is_err());
        }
    }

    #[test]
    fn fetch_takes_urls_and_a_focus() {
        assert_eq!(
            parse(args(&["fetch", "https://a.example", "-query", "x y"])).unwrap(),
            Command::Fetch {
                urls: vec!["https://a.example".into()],
                query: Some("x y".into()),
                max_characters: None,
                offset: 0,
                json: false,
                config: None,
            }
        );
    }

    #[test]
    fn a_lone_flag_defaults_to_stdio() {
        assert_eq!(
            parse(args(&["-config", "/etc/settings.json"])).unwrap(),
            Command::Stdio {
                config: Some("/etc/settings.json".into())
            }
        );
    }

    #[test]
    fn a_search_without_words_is_an_error() {
        // A recognized verb with flags but no query is a mistake.
        assert!(parse(args(&["fetch"])).is_err());
        // A bare query word is a search.
        assert!(matches!(
            parse(args(&["hello"])).unwrap(),
            Command::Search { .. }
        ));
    }

    #[test]
    fn fetch_without_urls_is_an_error() {
        assert!(parse(args(&["fetch"])).is_err());
    }
}
