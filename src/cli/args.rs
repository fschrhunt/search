//! The command line: what `search` accepts and how it dispatches.
//!
//! `search QUERY` is the
//! human and script surface; `serve` is the HTTP and MCP surface; a bare
//! `search` with no query is the stdio MCP surface a harness spawns. Keeping
//! "no arguments" as MCP means an existing harness keeps working while a person
//! gets a real search command. Package verbs manage local engines; `engines`
//! inspects installed packages and the catalog.

/// One parsed command.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Manage or explicitly test local engine packages.
    Engines {
        action: String,
        args: Vec<String>,
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
        Some("install" | "enable" | "disable" | "update" | "remove" | "configure" | "test") => {
            parse_engines(args)
        }
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
        // Flags with no subcommand: default to stdio so harness configs that
        // pass `-config` still work.
        Some("-config") | Some("--config") => Ok(Command::Stdio {
            config: parse_config_flag(args)?,
        }),
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
  search engines available       inspect the shipped catalog (Mwmbl, SearXNG)
  search engines list            inspect installed engines and setup requirements
  search install ID|./PATH        install a catalog or local package; do not enable
  search configure ID [KEY VALUE] inspect or change adapter settings
  search enable ID [--trust]      select an engine; commands require explicit trust
  search disable ID              deselect an engine
  search update ID               update an unedited catalog package
  search remove ID               disable and remove an installed package
  search test ID QUERY            run one engine explicitly and print JSON
  Engine commands always run locally. search engines ACTION aliases still work.
  Shipped and custom packages share ~/.search/engines; SEARCH_HOME changes the root.
  Use env/header_env for credentials, never values in shareable settings.

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
  -max-chars N                    per-page character limit (1–40000)
  -json                          print JSON instead of text
  Without query or character limit, return the full clean page.
  Focused reads default to 6000 characters per page.

Settings:
  -config PATH                    settings file; place after the command/query
  Default: $CONFIG or $SEARCH_HOME/settings.json (SEARCH_HOME defaults to ~/.search).
  Double-dash flag forms are accepted. Use -- before literal query/value arguments.
  Guide: https://github.com/fschrhunt/search/tree/main/docs"
}

/// Parse package commands, extracting settings flags before literal `--` arguments.
fn parse_engines<I: IntoIterator<Item = String>>(args: I) -> Result<Command, String> {
    let mut args = args.into_iter();
    let action = args
        .next()
        .ok_or("expected an engines command; see search help")?;
    let mut config = None;
    let mut values = Vec::new();
    let mut literal = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--" if !literal => literal = true,
            "-config" | "--config" if !literal => {
                config = Some(args.next().ok_or("missing value for -config")?);
            }
            "--trust" if !literal && action == "enable" => values.push(arg),
            flag if !literal && flag.starts_with('-') => {
                return Err(format!("unknown engine flag {flag:?}"));
            }
            _ => values.push(arg),
        }
    }
    let valid = match action.as_str() {
        "available" | "list" => values.is_empty(),
        "install" | "update" | "remove" | "disable" => values.len() == 1,
        "configure" => values.len() == 1 || values.len() == 3,
        "enable" => {
            values
                .iter()
                .filter(|arg| arg.as_str() != "--trust")
                .count()
                == 1
                && values
                    .iter()
                    .filter(|arg| arg.as_str() == "--trust")
                    .count()
                    <= 1
        }
        "test" => values.len() >= 2 && !values.iter().skip(1).all(|s| s.trim().is_empty()),
        _ => false,
    };
    if !valid {
        return Err("invalid engines command or arguments; see search help".into());
    }
    Ok(Command::Engines {
        action,
        args: values,
        config,
    })
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
    let mut limit = 10usize;
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
    fn engine_commands_extract_config_and_preserve_literal_query() {
        assert_eq!(
            parse(args(&[
                "engines",
                "test",
                "fixture",
                "--config",
                "x.json",
                "--",
                "-site:example.com",
                "rust"
            ]))
            .unwrap(),
            Command::Engines {
                action: "test".into(),
                args: vec!["fixture".into(), "-site:example.com".into(), "rust".into()],
                config: Some("x.json".into())
            }
        );
        for values in [
            vec!["engines", "test", "fixture"],
            vec!["engines", "list", "fixture"],
            vec!["engines", "install", "fixture", "--trust"],
            vec!["engines", "configure", "fixture", "url"],
            vec!["engines", "enable", "fixture", "--trust", "--trust"],
            vec!["engines", "list", "--home", "/tmp"],
        ] {
            assert!(parse(args(&values)).is_err());
        }
        assert!(parse(args(&["engines", "enable", "fixture", "--trust"])).is_ok());
    }

    #[test]
    fn top_level_engine_commands_route_like_their_aliases() {
        for values in [
            vec!["install", "searxng"],
            vec!["enable", "fixture", "--trust"],
            vec!["disable", "fixture"],
            vec!["update", "fixture"],
            vec!["remove", "fixture"],
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

    #[test]
    fn literal_search_queries_can_contain_command_names_and_flags() {
        assert_eq!(
            parse(args(&["--", "install", "-engines", "literal"])).unwrap(),
            Command::Search {
                query: "install -engines literal".into(),
                limit: 10,
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
