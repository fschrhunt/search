# Configuration

Settings are read from `~/.config/search/settings.json`, or the file named by
`CONFIG`. Missing explicitly selected files are errors; only an absent default
file uses built-in settings. The file is ordinary JSON; add explanations in the
optional `notes` object. Unknown settings fail at startup rather than being ignored.

## Environment overrides

- `CONFIG` selects the settings file.
- `ADDRESS` overrides `address`.
- `DIR` overrides `dir`.

Pairing credentials live in owner-only files under `dir/trust`, separate from
settings. Local CLI and stdio MCP need no authentication. `search serve` always
uses paired HTTPS, on loopback and remote interfaces alike. See [remote](remote.md).

## Settings

```json
{
  "notes": {
    "timeouts": "Timeout and cache_ttl numbers are milliseconds.",
    "index.enabled": "False disables all corpus access; it does not delete existing pages."
  },
  "address": "127.0.0.1:8642",
  "dir": "/home/you/.local/share/search",
  "search": {
    "max_results": 10,
    "engine_timeout": 2000,
    "timeout": 8000,
    "local_weight": 1.5
  },
  "fetch": {
    "timeout": 15000,
    "max_response_bytes": 4194304,
    "max_redirects": 5,
    "cache_ttl": 600000,
    "allow_private_networks": false,
    "max_concurrency": 8,
    "max_stored_chars": 40000
  },
  "index": {
    "enabled": true,
    "save_fetched_pages": true,
    "include_in_search": true,
    "max_size_mb": 512,
    "retention_days": 180,
    "refresh_hosts": [],
    "refresh_interval_days": 7
  },
  "engines": {
    "enabled": true,
    "only": ["brave", "wikipedia", "stackexchange"]
  },
  "remote": {
    "timeout": 120000
  }
}
```

Timeouts and cache TTL are milliseconds; fields ending in `_days` use days.
Omit a setting to use its built-in default. Explicit zero values are never
replaced with defaults: `cache_ttl: 0` disables the fetch cache and
`max_redirects: 0` follows no redirects. Deadlines, result and body limits, and
concurrency must be positive. Zero size/retention means no pruning; a zero
refresh interval makes every saved seeded page eligible for explicit refresh.

- `address` is the listener address. `dir` holds the SQLite index.
- `search.engine_timeout` limits one engine; `search.timeout` limits the
  whole query. `max_results` caps results and `local_weight` ranks local hits.
- `fetch.timeout` and `max_response_bytes` bound page retrieval. Set
  `allow_private_networks` only for tests or air-gapped mirrors; it disables the
  SSRF guard.
- `index.enabled: false` disables all database access, including explicit index
  commands, without creating, opening, pruning, or deleting the corpus. HTTPS
  device credentials remain separate and are still stored when hosting/pairing.
- `index.save_fetched_pages` controls local storage, and
  `index.include_in_search` controls blending local hits into web results.
  Disabling either does not delete existing pages; explicit index search remains
  available.
- `engines.enabled: false` disables live search engines, leaving local-index
  search available. `engines.only` optionally restricts engines; an empty
  list enables all when `enabled` is true. The current engines are keyless.
- `remote.timeout` sets the overall deadline for calls to the selected paired
  host. The default is 120000 milliseconds; the connection timeout stays fixed
  at 10000 milliseconds.

`dir` expands a leading `~/` using `HOME`. The old `directory`,
`token`, and `token_env` fields are rejected, with no compatibility aliases.
