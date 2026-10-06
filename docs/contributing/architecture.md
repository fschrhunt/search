# Architecture

Search is a Cargo workspace with four packages. `crates/search` is the engine
library and has no terminal, listener, or protocol dependency. `crates/cli` is
the `search` executable and HTTP API. `crates/mcp` is the optional MCP adapter.
Both adapters depend on `search`; the engine depends on neither. `crates/engines`
contains package metadata, the embedded maintained catalog, and local installation;
the core and CLI depend on it. Per-engine behavior lives in package manifests/programs,
not Rust registrations in the core.

```
crates/search/  the engine (`search`)
  config/       the settings surface: settings.rs (the shape), defaults.rs
                (built-in values), load.rs (read, merge, validate)
  discovery/    the provider fan-out: mod.rs (Finding, Query, Response, the
                Provider trait), registry.rs (parallel fan-out, per-provider
                deadlines, reciprocal-rank fusion), adapters.rs (guarded HTTP
                and executable engine transports)
  fetch/        mod.rs (the guarded request, body caps, indexing), guard.rs (the
                SSRF guard and the DNS resolver), extract.rs (HTML to text),
                cache.rs (recent answers)
  index/        mod.rs (Store over SQLite), schema.rs (tables, triggers, the FTS
                query builder)
  service.rs    the public `Search` engine and its operations
crates/mcp/   the MCP tools and transports (`search_mcp`)
  lib.rs        tool definitions and stdio transport
  http.rs       streamable HTTP transport
  backend.rs    exclusive local/remote execution and verified TLS leaf pinning
crates/cli/   the command and HTTP API package (`cli`; binary `search`)
  args.rs       the command line
  run.rs        dispatch, and build the service with overrides
  http.rs       paired HTTPS, JSON API and per-request device authentication
  trust.rs      persistent identity, bounded pairing and private trust files
  remote.rs     local profile selection, device listing and revocation
  engines.rs    local package lifecycle, user settings and diagnostics
crates/engines/  package catalog and installer (`search_engines`)
  src/          manifests, bounded package reads, private staged writes, receipts
  <engine>/     maintained engine.json, programs/docs and offline fixtures
```

The engine stays independent of frontend protocols. Its discovery adapters
support packaged JSON HTTP APIs and a versioned JSON subprocess contract;
the registry has no engine-specific implementations. Adapters discover URLs,
while the fetcher reads pages and owns indexing. CLI and stdio MCP share the
adapter's `Backend`: a selected remote is checked before opening any local
engine, and failures never trigger local execution. The host always opens its
local engine. Pairing verifies a copied public certificate's fingerprint before
sending the one-use code; TLS enforces the exact leaf pin plus standard WebPKI
validation. Host trust stores hashes, client trust stores secrets, and neither
belongs in engine settings. See [remote hosting](../remote.md).

Packages are installed locally from the maintained catalog or an explicit package
directory, never request input, implicit working-directory discovery, or database registration.
`SEARCH_HOME` owns packages/settings/data/trust; `dir` overrides corpus storage only.
Package updates do not overwrite user settings or local edits. HTTP adapters reuse
the SSRF resolver, refuse redirects
and proxies, and have independent explicit private-network permission. Executable
adapters are trusted programs running as the host user; cancellation kills the
direct child, not arbitrary descendants; only declared credentials are inherited.
See [engine packages](../engines.md).

## The rules

- **The engine stays independent.** `search::Search` is the in-process API.
  The CLI and MCP adapter depend on it; the engine does not depend on either.
- **The security-critical file is `fetch/guard.rs`.** Every class of address that
  can reach infrastructure must be classified private there, and
  `check_host` strips IPv6 brackets before parsing. Its tests carry the
  counterexamples; do not loosen them. `scripts/guard.sh` fails the build if the
  guard or its call sites move.
- **Shipped code denies panic sites.** `crates/search/src/lib.rs` denies
  `clippy::unwrap_used`, `expect_used`, `panic`, `unreachable`, and
  `indexing_slicing`. Every allowed site carries a `proof:` comment or a scoped
  `#[allow]` explaining why runtime input cannot reach it.
- **Providers are keyless by default**, and a provider's failure is reported in
  its `ProviderState` rather than failing the query.
- **Don't hardcode what a user might change.** A new user-facing behaviour is a
  config field, not a constant.
