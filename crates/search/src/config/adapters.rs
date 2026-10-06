//! Validate configured adapters without resolving credentials or contacting endpoints.

use super::{AdapterSettings, EngineSettings};

/// Package IDs are safe path components and cannot shadow the local index.
pub fn valid_engine_id(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name != "index"
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

impl EngineSettings {
    /// Validate settings only; package presence is checked after remote selection.
    pub(super) fn validate(&self) -> Result<(), String> {
        let mut seen = std::collections::BTreeSet::new();
        for name in &self.use_engines {
            if !valid_engine_id(name) {
                return Err("engines.use contains an invalid ID; use lowercase letters, digits, - or _ (max 64), excluding index".into());
            }
            if !seen.insert(name) {
                return Err(format!("engines.use repeats engine {name}"));
            }
        }
        for (name, value) in &self.config {
            if !valid_engine_id(name) {
                return Err("engines.config contains an invalid engine ID".into());
            }
            if !value.is_object() {
                return Err(format!("engines.config.{name} must be an object"));
            }
        }
        Ok(())
    }
}

impl AdapterSettings {
    /// Check a merged transport without reading credentials or running engine code.
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            AdapterSettings::Command(settings) => {
                if settings.command.trim().is_empty() || settings.command.contains('\0') {
                    return Err("command must name an executable");
                }
                if settings.args.iter().any(|arg| arg.contains('\0')) {
                    return Err("args contains NUL");
                }
                if settings.env.iter().any(|name| !valid_env(name)) {
                    return Err("command env must contain valid environment variable names");
                }
                if settings.max_response_bytes == 0 {
                    return Err("max_response_bytes must be positive");
                }
            }
            AdapterSettings::Http(settings) => {
                if settings.query_param.is_empty()
                    || settings
                        .limit_param
                        .as_ref()
                        .is_some_and(|param| param.is_empty() || param == &settings.query_param)
                {
                    return Err("query_param and limit_param must be nonempty and distinct");
                }
                if http_url(&settings.url).is_none() {
                    return Err("url must be http(s) without credentials or fragment");
                }
                if settings.max_response_bytes == 0 {
                    return Err("max_response_bytes must be positive");
                }
                for pointer in [
                    &settings.results_pointer,
                    &settings.title_pointer,
                    &settings.url_pointer,
                    &settings.snippet_pointer,
                ] {
                    if !valid_pointer(pointer) {
                        return Err("results_pointer/title_pointer/url_pointer/snippet_pointer must be valid JSON pointers");
                    }
                }
                if settings
                    .text_part_pointer
                    .as_deref()
                    .is_some_and(|pointer| !valid_pointer(pointer))
                {
                    return Err("text_part_pointer must be a valid JSON pointer");
                }
                for name in settings.headers.keys().chain(settings.header_env.keys()) {
                    if reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_err() {
                        return Err("headers/header_env must use valid HTTP header names");
                    }
                }
                for value in settings.headers.values() {
                    if reqwest::header::HeaderValue::from_str(value).is_err() {
                        return Err("headers contains an invalid HTTP header value");
                    }
                }
                for name in settings.header_env.values() {
                    if !valid_env(name) {
                        return Err("header_env must reference valid environment variable names");
                    }
                }
            }
        }
        Ok(())
    }
}

/// Resolve only the selected enabled packages; discard any stale internal adapters.
pub fn resolve_engines(config: &mut super::Config) -> Result<(), super::ConfigError> {
    config.validate()?;
    config.engines.adapters.clear();
    config.engines.leases.clear();
    if !config.engines.enabled {
        return Ok(());
    }
    let mut resolved = std::collections::BTreeMap::new();
    let mut leases = Vec::new();
    for id in &config.engines.use_engines {
        let package = search_engines::resolve(&config.home, id).map_err(|_| {
            super::ConfigError::new(format!(
                "engine {id}: package unavailable; run search engines install {id} or disable it"
            ))
        })?;
        let adapter = resolve_adapter(&package, config.engines.config.get(id))?;
        if let Some(lease) = &package.lease {
            leases.push(lease.clone());
        }
        resolved.insert(id.clone(), adapter);
    }
    config.engines.adapters = resolved;
    config.engines.leases = leases;
    Ok(())
}

/// Merge direct adapter fields and validate setup without executing the package.
/// Shared with CLI enable/configure; never quotes configured values in errors.
pub fn resolve_adapter(
    package: &search_engines::Installed,
    overrides: Option<&serde_json::Value>,
) -> Result<AdapterSettings, super::ConfigError> {
    let id = &package.manifest.id;
    if !valid_engine_id(id) {
        return Err(super::ConfigError::new("invalid package ID"));
    }
    let error = |message: &str| super::ConfigError::new(format!("engine {id}: {message}"));
    let mut merged = package
        .manifest
        .adapter
        .as_object()
        .cloned()
        .ok_or_else(|| error("adapter must be an object"))?;
    if let Some(overrides) = overrides {
        let overrides = overrides
            .as_object()
            .ok_or_else(|| error("config must be an object"))?;
        if overrides
            .get("type")
            .is_some_and(|value| Some(value) != merged.get("type"))
        {
            return Err(error("config.type cannot change the package adapter type"));
        }
        merged.extend(overrides.clone());
    }
    for field in &package.manifest.required {
        if merged.get(field).is_none_or(|value| match value {
            serde_json::Value::Null => true,
            serde_json::Value::String(value) => value.trim().is_empty(),
            serde_json::Value::Array(value) => value.is_empty(),
            serde_json::Value::Object(value) => value.is_empty(),
            _ => false,
        }) {
            // Only safe field identifiers are reflected, never arbitrary manifest strings.
            if !field.is_empty()
                && field
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                return Err(error(&format!("required config field {field} is missing; run search engines configure {id} {field} VALUE")));
            }
            return Err(error("required adapter field is missing"));
        }
    }
    let mut adapter: AdapterSettings = serde_path_to_error::deserialize(serde_json::Value::Object(merged)).map_err(|failure| {
        let field = safe_field_path(failure.path());
        error(&format!("invalid adapter field {field}; check engines.config.{id} and the package adapter schema"))
    })?;
    adapter.validate().map_err(error)?;
    if let AdapterSettings::Command(settings) = &mut adapter {
        if !package.path.is_absolute() {
            return Err(error("command package root must be absolute"));
        }
        settings.cwd = package.path.clone();
    }
    Ok(adapter)
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

/// Environment references are names, never assignments or shell expressions.
fn valid_env(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Reflect schema field names only; arbitrary map keys may themselves contain secrets.
pub(crate) fn safe_field_path(path: &serde_path_to_error::Path) -> String {
    let known = [
        "notes",
        "address",
        "dir",
        "user_agent",
        "search",
        "fetch",
        "engines",
        "remote",
        "index",
        "max_results",
        "engine_timeout",
        "timeout",
        "local_weight",
        "max_response_bytes",
        "max_redirects",
        "cache_ttl",
        "allow_private_networks",
        "max_concurrency",
        "max_stored_chars",
        "enabled",
        "save_fetched_pages",
        "include_in_search",
        "max_size_mb",
        "retention_days",
        "refresh_hosts",
        "refresh_interval_days",
        "use",
        "config",
        "type",
        "url",
        "query_param",
        "limit_param",
        "params",
        "headers",
        "header_env",
        "results_pointer",
        "title_pointer",
        "url_pointer",
        "snippet_pointer",
        "text_part_pointer",
        "command",
        "args",
        "env",
    ];
    let mut fields = Vec::new();
    for segment in path {
        match segment {
            serde_path_to_error::Segment::Map { key } if known.contains(&key.as_str()) => {
                fields.push(key.as_str())
            }
            _ => break,
        }
    }
    if fields.is_empty() {
        "settings".into()
    } else {
        fields.join(".")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use serde_json::json;

    /// Metadata fixture requires no installation or executable invocation.
    fn package(adapter: serde_json::Value) -> search_engines::Installed {
        search_engines::Installed {
            manifest: search_engines::Manifest {
                schema_version: 1,
                id: "fixture".into(),
                version: "1".into(),
                description: "test".into(),
                adapter,
                required: vec![],
                files: vec![],
                executables: vec![],
                requires: vec![],
            },
            path: std::env::temp_dir().join("search-package-fixture"),
            lease: None,
            source: "local".into(),
            digest: String::new(),
        }
    }

    #[test]
    fn validation_does_not_require_local_packages_or_resolve_overrides() {
        let mut config: Config = serde_json::from_value(json!({"engines":{
            "use":["missing-package"],
            "config":{"missing-package":{"url":"invalid-secret-url"}}
        }}))
        .unwrap();
        config.home =
            std::env::temp_dir().join(format!("search-validation-{}", uuid::Uuid::new_v4()));
        config.dir = config.home.join("data");
        config.validate().unwrap();
        assert!(config.engines.adapters.is_empty());
        let mut disabled = config.clone();
        disabled.engines.enabled = false;
        resolve_engines(&mut disabled).unwrap();
        assert!(disabled.engines.adapters.is_empty());
        let error = crate::Search::open(config).err().unwrap().to_string();
        assert!(error.contains("missing-package"));
        assert!(error.contains("install"));
        assert!(!error.contains("invalid-secret-url"));
    }

    #[test]
    fn selection_is_explicit_and_settings_cannot_inject_execution_adapters() {
        let defaults: Config = serde_json::from_value(json!({})).unwrap();
        assert_eq!(
            defaults.engines.use_engines,
            [search_engines::DEFAULT_ENGINE]
        );
        let mut empty: Config = serde_json::from_value(json!({"engines":{"use":[]}})).unwrap();
        resolve_engines(&mut empty).unwrap();
        assert!(empty.engines.adapters.is_empty());
        assert!(serde_json::from_value::<Config>(json!({"engines":{"custom":{}}})).is_err());
        for selection in [
            json!(["index"]),
            json!(["../escape"]),
            json!(["Upper"]),
            json!(["fixture", "fixture"]),
        ] {
            let config: Config =
                serde_json::from_value(json!({"engines":{"use":selection}})).unwrap();
            assert!(config.validate().is_err());
        }
        let config: Config =
            serde_json::from_value(json!({"engines":{"config":{"fixture":[]}}})).unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn overrides_merge_shallowly_and_cannot_change_transport() {
        let package = package(
            json!({"type":"command", "command":"./run", "args":["literal"], "config":{"old":true}}),
        );
        let overrides = json!({"config":{"new":"$HOME"},"env":["FIXTURE_TOKEN"]});
        let AdapterSettings::Command(adapter) =
            resolve_adapter(&package, Some(&overrides)).unwrap()
        else {
            panic!("command fixture")
        };
        assert_eq!(adapter.cwd, package.path);
        assert_eq!(adapter.args, ["literal"]);
        assert_eq!(
            adapter.config,
            std::collections::BTreeMap::from([("new".into(), json!("$HOME"))])
        );
        assert_eq!(adapter.env, ["FIXTURE_TOKEN"]);
        assert!(resolve_adapter(&package, Some(&json!({"type":"http"}))).is_err());
        assert!(resolve_adapter(&package, Some(&json!({"cwd":"/secret"}))).is_err());
    }

    #[test]
    fn required_setup_and_invalid_transport_errors_identify_engine_without_values() {
        let mut package = package(json!({"type":"http","url":""}));
        package.manifest.required = vec!["url".into()];
        let error = resolve_adapter(&package, None).unwrap_err().to_string();
        assert!(error.contains("engine fixture"));
        assert!(error.contains("configure fixture url"));
        assert!(resolve_adapter(&package, Some(&json!({"url":"https://example.com/"}))).is_ok());
        for override_value in [
            json!({"url":"https://user:secret@example.com/"}),
            json!({"url":"https://example.com/","max_response_bytes":0}),
        ] {
            let error = resolve_adapter(&package, Some(&override_value))
                .unwrap_err()
                .to_string();
            assert!(error.contains("engine fixture"));
            assert!(!error.contains("secret"));
            assert!(!error.contains("example.com"));
        }
    }

    #[test]
    fn transports_reject_bad_bounds_pointers_headers_and_environment_names() {
        for adapter in [
            json!({"type":"command","command":" "}),
            json!({"type":"command","command":"tool","max_response_bytes":0}),
            json!({"type":"command","command":"tool","args":["bad\u{0000}arg"]}),
            json!({"type":"command","command":"tool","env":["TOKEN=secret"]}),
            json!({"type":"http","url":"https://secret:password@example.com/"}),
            json!({"type":"http","url":"https://example.com/#secret"}),
            json!({"type":"http","url":"ftp://example.com/"}),
            json!({"type":"http","url":"https://example.com/","title_pointer":"/bad~2"}),
            json!({"type":"http","url":"https://example.com/","results_pointer":"results"}),
            json!({"type":"http","url":"https://example.com/","headers":{"Bad Header":"secret"}}),
            json!({"type":"http","url":"https://example.com/","header_env":{"Authorization":"BAD=secret"}}),
            json!({"type":"http","url":"https://example.com/","max_response_bytes":0}),
            json!({"type":"http","url":"https://example.com/","query_param":""}),
            json!({"type":"http","url":"https://example.com/","limit_param":"q"}),
        ] {
            let error = resolve_adapter(&package(adapter), None)
                .unwrap_err()
                .to_string();
            assert!(!error.contains("secret"));
            assert!(error.contains("engine fixture"));
        }
    }
}
