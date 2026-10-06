//! Validate configured adapters without resolving credentials or contacting endpoints.

use super::{AdapterSettings, EngineSettings};

/// The built-in identifiers reserved against custom adapter collisions.
pub const BUILTIN_NAMES: &[&str] = &[
    "brave",
    "marginalia",
    "mwmbl",
    "wikipedia",
    "hackernews",
    "stackexchange",
    "arxiv",
];

impl EngineSettings {
    /// Reject ambiguous names, malformed adapters and unknown selections.
    pub(super) fn validate(&self) -> Result<(), &'static str> {
        for (name, adapter) in &self.custom {
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                || BUILTIN_NAMES.contains(&name.as_str())
                || name == "index"
            {
                return Err("invalid or reserved custom engine name");
            }
            match adapter {
                AdapterSettings::Command(settings) => {
                    if settings.command.trim().is_empty() || settings.command.contains('\0') {
                        return Err("custom command must name an executable");
                    }
                    if settings.args.iter().any(|arg| arg.contains('\0')) {
                        return Err("custom command argument contains NUL");
                    }
                    if settings.max_response_bytes == 0 {
                        return Err("custom output cap must be positive");
                    }
                }
                AdapterSettings::Http(settings) => {
                    if settings.query_param.is_empty()
                        || settings
                            .limit_param
                            .as_ref()
                            .is_some_and(|param| param.is_empty() || param == &settings.query_param)
                    {
                        return Err(
                            "custom query and limit parameters must be nonempty and distinct",
                        );
                    }
                    if http_url(&settings.url).is_none() {
                        return Err(
                            "custom endpoint must be http(s) without credentials or fragment",
                        );
                    }
                    if settings.max_response_bytes == 0 {
                        return Err("custom output cap must be positive");
                    }
                    for pointer in [
                        &settings.results_pointer,
                        &settings.title_pointer,
                        &settings.url_pointer,
                        &settings.snippet_pointer,
                    ] {
                        if !valid_pointer(pointer) {
                            return Err("invalid custom JSON pointer");
                        }
                    }
                    for name in settings.headers.keys().chain(settings.header_env.keys()) {
                        if reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_err() {
                            return Err("invalid custom header name");
                        }
                    }
                    for value in settings.headers.values() {
                        if reqwest::header::HeaderValue::from_str(value).is_err() {
                            return Err("invalid custom header value");
                        }
                    }
                    for name in settings.header_env.values() {
                        let mut bytes = name.bytes();
                        if !bytes
                            .next()
                            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
                            || !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
                        {
                            return Err("invalid custom environment variable name");
                        }
                    }
                }
            }
        }
        if self
            .only
            .iter()
            .any(|name| !BUILTIN_NAMES.contains(&name.as_str()) && !self.custom.contains_key(name))
        {
            return Err("engines.only contains an unknown engine");
        }
        Ok(())
    }
}

/// Parse an absolute HTTP URL with a host, no userinfo, and no fragment.
pub(crate) fn http_url(value: &str) -> Option<url::Url> {
    let url = url::Url::parse(value).ok()?;
    let authority = url
        .as_str()
        .split_once("//")?
        .1
        .split(['/', '?', '#'])
        .next()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || authority.contains('@')
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    Some(url)
}

/// RFC 6901 permits the root pointer and only ~0/~1 escapes.
fn valid_pointer(pointer: &str) -> bool {
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return false;
    }
    let mut chars = pointer.chars();
    while let Some(c) = chars.next() {
        if c == '~' && !matches!(chars.next(), Some('0' | '1')) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn custom_defaults_and_selection_keep_builtins_available() {
        let config: Config = serde_json::from_value(serde_json::json!({"engines":{"custom":{"my-engine_2":{"type":"http","url":"https://example.com/"}}}})).unwrap();
        config.validate().unwrap();
        let AdapterSettings::Http(settings) = &config.engines.custom["my-engine_2"] else {
            panic!("HTTP fixture");
        };
        assert_eq!(settings.query_param, "q");
        assert_eq!(settings.results_pointer, "/results");
        assert_eq!(settings.title_pointer, "/title");
        assert_eq!(settings.url_pointer, "/url");
        assert_eq!(settings.snippet_pointer, "/snippet");
        assert_eq!(settings.max_response_bytes, 1 << 20);
        assert!(!settings.allow_private_networks);
        assert_eq!(
            crate::discovery::Registry::new(&config.engines, config.search)
                .names()
                .len(),
            8
        );
    }

    #[test]
    fn malformed_adapters_fail_validation_without_echoing_values() {
        for adapter in [
            serde_json::json!({"type":"command","command":" "}),
            serde_json::json!({"type":"command","command":"tool","max_response_bytes":0}),
            serde_json::json!({"type":"http","url":"https://secret:password@example.com/"}),
            serde_json::json!({"type":"http","url":"https://example.com/#secret"}),
            serde_json::json!({"type":"http","url":"ftp://example.com/"}),
            serde_json::json!({"type":"http","url":"https://example.com/","title_pointer":"/bad~2"}),
            serde_json::json!({"type":"http","url":"https://example.com/","results_pointer":"results"}),
            serde_json::json!({"type":"http","url":"https://example.com/","headers":{"Bad Header":"secret"}}),
            serde_json::json!({"type":"http","url":"https://example.com/","header_env":{"Authorization":"BAD=secret"}}),
            serde_json::json!({"type":"http","url":"https://example.com/","max_response_bytes":0}),
            serde_json::json!({"type":"http","url":"https://example.com/","query_param":""}),
            serde_json::json!({"type":"http","url":"https://example.com/","limit_param":"q"}),
            serde_json::json!({"type":"command","command":"tool","args":["bad\u{0000}arg"]}),
        ] {
            let config: Config = serde_json::from_value(
                serde_json::json!({"engines":{"custom":{"fixture":adapter}}}),
            )
            .unwrap();
            let error = config.validate().unwrap_err().to_string();
            assert!(!error.contains("secret"));
            assert!(crate::Search::open(config).is_err());
        }
    }

    #[test]
    fn names_collisions_and_unknown_selections_are_rejected() {
        for name in ["", "bad.name", "space name", "brave", "index", "é"] {
            let mut config = Config::default();
            config.engines.custom.insert(
                name.into(),
                AdapterSettings::Command(super::super::CommandAdapterSettings {
                    command: "tool".into(),
                    args: vec![],
                    max_response_bytes: 1,
                }),
            );
            assert!(config.validate().is_err());
        }
        let mut config = Config::default();
        config.engines.only.push("unknown".into());
        assert!(config.validate().is_err());
    }
}
