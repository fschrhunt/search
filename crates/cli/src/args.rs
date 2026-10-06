//! The command line: what `search` accepts and how it dispatches.
//!
//! `search QUERY` is the
//! human and script surface; `serve` is the HTTP and MCP surface; a bare
//! `search` with no query is the stdio MCP surface a harness spawns. Keeping
//! "no arguments" as MCP means an existing harness keeps working while a person
//! gets a real search command. `engines` diagnoses local adapters without corpus access.

/// One parsed command.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Inspect or test the local host's configured engine adapters without opening its index.
    Engines {
        name: Option<String>,
        query: Option<String>,
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
        dir: Option<String>,
    },
    /// Search the web, printing results for a person or JSON for a script.
    Search {
        query: String,
        limit: usize,
        json: bool,
        providers: Vec<String>,
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
    /// Search only the selected corpus.
    Index {
        query: String,
        limit: usize,
        json: bool,
        config: Option<String>,
    },
    /// Refresh stale documents on configured index hosts.
    Refresh { config: Option<String> },
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
                dir: flags.dir,
            })
        }
        Some("fetch") => {
            let _ = args.next();
            parse_fetch(args)
        }
        Some("index") => {
            let _ = args.next();
            parse_index(args)
        }
        Some("refresh") => {
            let _ = args.next();
            Ok(Command::Refresh {
                config: parse_config_flag(args)?,
            })
        }
        Some("engines") => {
            let _ = args.next();
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

/// The usage text.
pub fn usage() -> &'static str {
    "search — a self-hosted web search engine

Usage:
  search QUERY [flags]         search the web and print results
  search fetch URL... [flags]  read pages as clean text
  search index QUERY [flags]   search only the selected corpus
  search refresh [flags]       refresh stale configured index hosts
  search engines list [-config PATH]           list enabled local engines
  search engines test NAME QUERY [-config PATH] test one local engine (JSON)
    Use test NAME [-config PATH] -- QUERY for queries starting with a hyphen.
  search serve [flags]         serve the JSON API and MCP over HTTP
  search                       serve MCP over stdio (what an agent spawns)
  search remote pair NAME HTTPS_URL CERT_FILE SHA256  pair (code from stdin)
  search remote use/list/off/remove [NAME]  choose the execution target
  search pair-code             renew the host pairing code without restarting
  search devices               list devices paired with this host
  search revoke DEVICE_ID      revoke a device on this host
  search version               print the version

Serve flags:
  -address HOST:PORT   paired HTTPS listener (default 127.0.0.1:8642)
  -hostname NAME      certificate name clients connect to (for wildcard binds)
  -dir PATH           local corpus and private trust storage
  -config PATH        local settings

Search flags:
  -limit N       results to return (default 10)
  -json          print JSON instead of text
  -providers A,B restrict to these providers
  -config PATH   settings file (default $CONFIG or ~/.config/search/settings.json)

Fetch flags:
  -query TEXT        return only the passages matching TEXT
  -max-chars N       cap the characters returned per page
  -json              print JSON instead of text
  -config PATH       JSON config

Index flags:
  -limit N       results to return (default 10)
  -json          print JSON instead of text
  -config PATH   JSON config"
}

/// Parse local adapter diagnostics; query words may include flags after `--`.
fn parse_engines<I: IntoIterator<Item = String>>(args: I) -> Result<Command, String> {
    let mut args = args.into_iter();
    let action = args.next().ok_or("expected engines list or test")?;
    let mut config = None;
    let mut values = Vec::new();
    let mut literal = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--" if !literal => literal = true,
            "-config" | "--config" if !literal => {
                config = Some(args.next().ok_or("missing value for -config")?);
            }
            flag if !literal && flag.starts_with('-') => {
                return Err(format!("unknown flag {flag:?}"));
            }
            _ => values.push(arg),
        }
    }
    match action.as_str() {
        "list" if values.is_empty() => Ok(Command::Engines {
            name: None,
            query: None,
            config,
        }),
        "test" if values.len() >= 2 => {
            let name = values.remove(0);
            let query = values.join(" ");
            if query.trim().is_empty() {
                return Err("engine test needs a nonempty query".into());
            }
            Ok(Command::Engines {
                name: Some(name),
                query: Some(query),
                config,
            })
        }
        _ => Err("expected engines list or engines test NAME QUERY [-config PATH]".into()),
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
    let mut limit = 10usize;
    let mut json = false;
    let mut providers = Vec::new();
    let mut config = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-limit" | "--limit" => {
                limit = args
                    .next()
                    .ok_or("missing value for -limit")?
                    .parse()
                    .map_err(|_| "-limit needs a number".to_string())?;
            }
            "-json" | "--json" => json = true,
            "-providers" | "--providers" => {
                let value = args.next().ok_or("missing value for -providers")?;
                providers = value
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
        providers,
        config,
    })
}

fn parse_index<I: IntoIterator<Item = String>>(args: I) -> Result<Command, String> {
    let mut query_words = Vec::new();
    let mut limit = 10usize;
    let mut json = false;
    let mut config = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-limit" | "--limit" => {
                limit = args
                    .next()
                    .ok_or("missing value for -limit")?
                    .parse()
                    .map_err(|_| "-limit needs a number".to_string())?;
            }
            "-json" | "--json" => json = true,
            "-config" | "--config" => {
                config = Some(args.next().ok_or("missing value for -config")?)
            }
            other if other.starts_with('-') => return Err(format!("unknown flag {other:?}")),
            word => query_words.push(word.to_string()),
        }
    }
    if query_words.is_empty() {
        return Err("an index search needs a query".into());
    }
    Ok(Command::Index {
        query: query_words.join(" "),
        limit,
        json,
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
            "-dir" | "--dir" => flags.dir = Some(args.next().ok_or("missing value for -dir")?),
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
    pub dir: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn engine_diagnostics_require_a_name_and_query_for_test() {
        assert_eq!(
            parse(args(&[
                "engines", "test", "custom", "hello", "world", "-config", "x.json"
            ]))
            .unwrap(),
            Command::Engines {
                name: Some("custom".into()),
                query: Some("hello world".into()),
                config: Some("x.json".into())
            }
        );
        assert_eq!(
            parse(args(&["engines", "list"])).unwrap(),
            Command::Engines {
                name: None,
                query: None,
                config: None
            }
        );
        assert!(parse(args(&["engines", "test", "custom"])).is_err());
        assert!(parse(args(&["engines", "list", "custom"])).is_err());
        assert_eq!(
            parse(args(&[
                "engines",
                "test",
                "custom",
                "-config",
                "x.json",
                "--",
                "-site:example.com rust"
            ]))
            .unwrap(),
            Command::Engines {
                name: Some("custom".into()),
                query: Some("-site:example.com rust".into()),
                config: Some("x.json".into())
            }
        );
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
                providers: vec![],
                config: None,
            }
        );
    }

    #[test]
    fn serve_reads_its_flags() {
        assert_eq!(
            parse(args(&["serve", "-address", "0.0.0.0:1", "-dir", "/tmp/x",])).unwrap(),
            Command::Serve {
                config: None,
                address: Some("0.0.0.0:1".into()),
                hostname: None,
                dir: Some("/tmp/x".into()),
            }
        );
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

    #[test]
    fn refresh_accepts_config_path() {
        assert_eq!(
            parse(args(&["refresh", "-config", "x.json"])).unwrap(),
            Command::Refresh {
                config: Some("x.json".into())
            }
        );
    }
}
