# Configuration

Settings are read from `SEARCH_HOME/settings.json` (normally
`~/.search/settings.json`), or the file selected by `-config PATH` or `CONFIG`.
The command-line path takes precedence over `CONFIG`.
Missing explicitly selected files are errors; an absent default file uses built-in
settings. The optional `notes` object holds explanations. Unknown settings are
rejected rather than ignored.

`SEARCH_HOME` selects the application home (default `~/.search`), independently
of the working directory and settings file. It must be absolute; a leading `~/`
is expanded. Packages live under `SEARCH_HOME/engines`, and owner-only pairing
credentials under `SEARCH_HOME/trust`. `ADDRESS` overrides the listener address.
Local CLI and stdio MCP need no pairing; hosting uses [paired HTTPS](remote.md).

## Settings

```json
{
  "notes": { "timeouts": "Timeout and cache_ttl values are milliseconds." },
  "address": "127.0.0.1:8642",
  "search": {
    "max_results": 10,
    "engine_timeout": 5000,
    "timeout": 8000
  },
  "fetch": {
    "timeout": 15000,
    "max_response_bytes": 4194304,
    "max_redirects": 5,
    "cache_ttl": 600000,
    "cache_bytes": 33554432,
    "allow_private_networks": false,
    "max_concurrency": 8
  },
  "engines": {
    "enabled": true,
    "use": ["mwmbl"],
    "config": {}
  },
  "remote": { "timeout": 120000, "max_response_bytes": 67108864 }
}
```

Omitted fields use defaults. Deadlines, result/body limits, and concurrency must
be positive. `cache_ttl: 0` or `cache_bytes: 0` disables the transient in-memory
fetch cache; `max_redirects: 0` follows no redirects.

- `address` selects the listener. `user_agent` overrides page-fetch and HTTP-engine identity;
  by default it identifies Search and its version.
- `search.engine_timeout` bounds one engine; `search.timeout` bounds the whole
  query (defaults: 5000 ms and 8000 ms). `max_results` caps returned results.
- `remote.max_response_bytes` caps a paired host's JSON response (default 64 MiB).
  Raise it for larger full-page batches; exceeding it returns an error, never a
  local fallback. `remote.timeout` bounds the entire request.
- `fetch.timeout` and `max_response_bytes` bound page retrieval. Only tests and
  air-gapped mirrors should set `allow_private_networks: true`, which disables
  the page-fetch SSRF guard.
- `fetch.cache_bytes` bounds approximate cached string payloads (text, URLs,
  metadata, and keys) plus entry structs, defaulting to 32 MiB. Hash-table slack
  and allocator overhead are excluded. The cache clears all entries when an
  insertion would exceed this budget or 2048 entries. Pages larger than the
  budget are returned without being cached; cached pages expire after `cache_ttl`.
- `engines.use` selects installed package IDs; the default is just `mwmbl`.
  An empty list or `engines.enabled: false` returns no discovery results.
  `engines.config` holds per-engine adapter overrides separately from package
  files. See [engine packages](engines.md) for setup and trust.

The remote connection timeout is 10000 ms; the overall deadline and response
cap are controlled by the `remote` settings above.

## Migration

Search now provides live web search and clean fetch, with no persistent local
corpus, index queries, or refresh command. Existing files and data are untouched;
there is no automatic migration or deletion.

Remove `dir`, `search.local_weight`, `fetch.max_stored_chars`, and the entire
`index` object from settings: these fields are rejected. `DIR` and `-dir` are
removed corpus overrides. Use `SEARCH_HOME` for packages, settings, and trust.

Request selection and result attribution use `engines`; replace `providers`
and the CLI `-providers` flag with `engines` and `-engines`. The package manifest
field `adapter` remains transport configuration, not an engine selection field.

Old XDG paths and inline `engines.custom`/`engines.only` settings are not migrated.
The old `directory`, `token`, and `token_env` fields are also rejected. User
adapter overrides stay in settings, so package updates do not overwrite them.

Only Mwmbl and SearXNG remain in the shipped catalog. Already-installed engines
and settings are preserved; maintain other packages as custom engines rather
than expecting catalog updates. Custom engines use the same version-1 manifest,
command protocol, and package store; see
[package compatibility](engines.md#compatibility-across-search-updates).
