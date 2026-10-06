//! Reading, merging, and validating configuration.

use std::path::PathBuf;

use super::Config;

/// A configuration problem an operator must fix. It carries a message, not a
/// source error, because every cause is a file the operator owns.
#[derive(Debug)]
pub struct ConfigError(String);

impl ConfigError {
    fn new(message: impl Into<String>) -> Self {
        ConfigError(message.into())
    }
    /// The operator-facing message.
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

/// Load configuration from `path` (or the configured default), apply defaults
/// and environment overrides, and validate the result.
pub fn load(path: Option<PathBuf>) -> Result<Config, ConfigError> {
    let explicit = path.or_else(|| std::env::var_os("CONFIG").map(PathBuf::from));
    let mut config = read(explicit.clone().or_else(default_path), explicit.is_some())?;
    apply_env(&mut config);
    config.dir = expand_home(&config.dir, std::env::var_os("HOME").map(PathBuf::from))?;
    config.validate()?;
    Ok(config)
}

/// Only an absent implicit default file means built-in defaults; parse errors
/// report locations without quoting potentially sensitive setting values.
fn read(path: Option<PathBuf>, explicit: bool) -> Result<Config, ConfigError> {
    let Some(path) = path else {
        return Ok(Config::default());
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if !explicit && error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Config::default())
        }
        Err(error) => {
            return Err(ConfigError::new(format!(
                "read {}: {error}",
                path.display()
            )))
        }
    };
    serde_json::from_str(&text).map_err(|error| {
        // Deserializer messages may quote header values or credential-bearing URLs.
        ConfigError::new(format!(
            "parse {}: invalid settings at line {}, column {}",
            path.display(),
            error.line(),
            error.column()
        ))
    })
}

/// Expand only a leading home component, not shell variables or other users' homes.
fn expand_home(path: &std::path::Path, home: Option<PathBuf>) -> Result<PathBuf, ConfigError> {
    match path.strip_prefix("~") {
        Ok(rest) => home
            .map(|home| home.join(rest))
            .ok_or_else(|| ConfigError::new("dir uses ~ but HOME is not set")),
        Err(_) => Ok(path.to_path_buf()),
    }
}

/// The standard location: `$XDG_CONFIG_HOME/search/settings.json` or
/// `~/.config/search/settings.json`.
fn default_path() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg).join("search").join("settings.json"));
    }
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join(".config")
            .join("search")
            .join("settings.json")
    })
}

/// Environment variables win over the file for the fields that name a place to
/// bind or store.
fn apply_env(config: &mut Config) {
    if let Ok(address) = std::env::var("ADDRESS") {
        if !address.is_empty() {
            config.address = address;
        }
    }
    if let Ok(dir) = std::env::var("DIR") {
        if !dir.is_empty() {
            config.dir = PathBuf::from(dir);
        }
    }
}

impl Config {
    /// Whether the configured address is loopback. Accepts `host:port`, a bare
    /// `:port` (which binds every interface and is therefore NOT loopback), and
    /// `localhost:port`.
    pub fn is_loopback(&self) -> bool {
        match self.address.rsplit_once(':') {
            Some((host, _)) => match host {
                "" => false,
                "localhost" => true,
                host => host
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse::<std::net::IpAddr>()
                    .map(|ip| ip.is_loopback())
                    // A hostname that is not an IP literal: treat as non-loopback,
                    // the frontend must secure every listener.
                    .unwrap_or(false),
            },
            None => false,
        }
    }

    /// Reject malformed listeners and unusable bounds; authentication belongs to adapters.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !valid_address(&self.address) {
            return Err(ConfigError::new(format!(
                "address {:?} is not host:port",
                self.address
            )));
        }
        for (name, value) in [
            ("search.max_results", self.search.max_results as u64),
            ("search.engine_timeout", self.search.engine_timeout),
            ("search.timeout", self.search.timeout),
            ("fetch.timeout", self.fetch.timeout),
            ("fetch.max_response_bytes", self.fetch.max_response_bytes),
            ("fetch.max_concurrency", self.fetch.max_concurrency as u64),
            ("fetch.max_stored_chars", self.fetch.max_stored_chars as u64),
            ("remote.timeout", self.remote.timeout),
        ] {
            if value == 0 {
                return Err(ConfigError::new(format!(
                    "{name} must be greater than zero"
                )));
            }
        }
        if !self.search.local_weight.is_finite() || self.search.local_weight <= 0.0 {
            return Err(ConfigError::new(
                "search.local_weight must be finite and positive",
            ));
        }
        self.engines.validate().map_err(ConfigError::new)?;
        Ok(())
    }
}

/// Whether `address` is a bindable `host:port`, accepting an empty host (`:8642`)
/// and a bracketed IPv6 literal.
fn valid_address(address: &str) -> bool {
    let Some((host, port)) = address.rsplit_once(':') else {
        return false;
    };
    if port.is_empty() || port.parse::<u16>().is_err() {
        return false;
    }
    if host.is_empty() || host == "localhost" {
        return true;
    }
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    bare.parse::<std::net::IpAddr>().is_ok() || is_hostname(bare)
}

/// A conservative hostname check: letters, digits, dots, and hyphens only. A
/// name that passes still resolves at bind time; this only keeps validation from
/// rejecting a legitimate name like `search.internal.example`.
fn is_hostname(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_diagnostics_do_not_quote_adapter_values() {
        let path = std::env::temp_dir().join(format!("search-invalid-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, r#"{"engines":{"custom":{"fixture":{"type":"command","command":"tool","max_response_bytes":"credential-secret"}}}}"#).unwrap();
        let error = read(Some(path.clone()), true).unwrap_err().to_string();
        std::fs::remove_file(path).unwrap();
        assert!(error.contains("invalid settings at line"));
        assert!(!error.contains("credential-secret"));
    }

    #[test]
    fn explicit_missing_settings_fail_instead_of_enabling_defaults() {
        let path = std::env::temp_dir().join(format!("search-missing-{}", uuid::Uuid::new_v4()));
        assert!(read(Some(path.clone()), true).is_err());
        assert!(read(Some(path), false).is_ok());
    }

    #[test]
    fn home_expansion_is_limited_to_a_leading_component() {
        let home = Some(PathBuf::from("/home/example"));
        assert_eq!(
            expand_home(std::path::Path::new("~/.local/share/search"), home.clone()).unwrap(),
            PathBuf::from("/home/example/.local/share/search")
        );
        assert_eq!(
            expand_home(std::path::Path::new("~other/search"), home).unwrap(),
            PathBuf::from("~other/search")
        );
        assert!(expand_home(std::path::Path::new("~/search"), None).is_err());
    }

    #[test]
    fn explicit_zero_switches_survive_deserialization() {
        let config: Config = serde_json::from_str(
            r#"{"fetch":{"cache_ttl":0,"max_redirects":0},"index":{"refresh_interval_days":0}}"#,
        )
        .unwrap();
        assert!(config.validate().is_ok());
        assert_eq!(config.fetch.cache_ttl, 0);
        assert_eq!(config.fetch.max_redirects, 0);
        assert_eq!(config.index.refresh_after(), std::time::Duration::ZERO);
        let bad: Config = serde_json::from_str(r#"{"fetch":{"timeout":0}}"#).unwrap();
        assert!(bad.validate().is_err());
    }

    /// The parsed loopback check must not mistake `:8642` — which binds every
    /// interface — for loopback.
    #[test]
    fn all_interfaces_is_not_loopback() {
        for address in [":8642", "0.0.0.0:8642", "[::]:8642", "100.1.2.3:8642"] {
            let config = Config {
                address: address.into(),
                ..Config::default()
            };
            assert!(!config.is_loopback(), "{address} should not be loopback");
        }
        for address in ["127.0.0.1:8642", "[::1]:8642", "localhost:8642"] {
            let config = Config {
                address: address.into(),
                ..Config::default()
            };
            assert!(config.is_loopback(), "{address} should be loopback");
        }
    }

    #[test]
    fn malformed_address_is_rejected() {
        let config = Config {
            address: "not-an-address".into(),
            ..Config::default()
        };
        assert!(config.validate().is_err());
    }

    /// Removed credential and location names must not silently load defaults.
    #[test]
    fn legacy_settings_are_rejected() {
        for field in ["directory", "token", "token_env", "providers"] {
            let json = serde_json::json!({field: "old"});
            assert!(serde_json::from_value::<Config>(json).is_err());
        }
        assert!(serde_json::from_str::<Config>(r#"{"dir":"/tmp/search"}"#).is_ok());
    }

    #[test]
    fn settings_json_uses_clean_names() {
        let config = serde_json::from_str::<Config>(
            r#"{"notes":{"search.timeout":"milliseconds"},"search":{"timeout":5000},"index":{"include_in_search":false,"save_fetched_pages":false},"engines":{"only":["brave"]}}"#,
        )
        .ok();
        assert_eq!(
            config.map(|c| (
                c.index.should_include_in_search(),
                c.index.should_save_fetched_pages(),
                c.search.timeout,
                c.engines.only,
            )),
            Some((false, false, 5000, vec!["brave".into()]))
        );
    }

    #[test]
    fn live_engines_can_be_disabled_and_remote_timeout_is_configurable() {
        let config: Config =
            serde_json::from_str(r#"{"engines":{"enabled":false},"remote":{"timeout":45000}}"#)
                .unwrap();
        assert!(!config.engines.enabled);
        assert_eq!(config.remote.timeout(), std::time::Duration::from_secs(45));
        assert!(config.validate().is_ok());
    }
}
