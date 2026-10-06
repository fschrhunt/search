# Working on search

Instructions for an agent editing this repo.

## Build and check

```sh
./x build          # fast dev build
./x test           # the whole behavioral contract
./x check          # format, lint, tests, shell syntax, and the security-surface guard
./x shell          # shell-script syntax alone
./x guard          # the security-surface audit alone
./x serve          # the JSON API and MCP over HTTP (dev build)
./x stdio          # MCP over stdio (what an agent spawns)
```

`./x check` is not optional. It runs formatting and linting, Cargo checks for
all features, minimal features, and MCP-only features, package tests, a syntax
check of every shell script, and `scripts/guard.sh`.

## Where things live

Search is one root Cargo package, with library `search` and CLI binary `search`.
`search::core` provides protocol-free live discovery and clean fetch. CLI and MCP
share local/remote execution through `search::client::Client`; MCP owns only tools
and transports. `search::engines` combines runtime engines with package metadata
and installation, without per-engine Rust registrations. Module conventions and
boundary tests preserve dependency direction; separate crate compilation no
longer enforces it.

```
Cargo.toml     one package; default feature cli includes mcp
build.rs       embeds the engine catalog and release identity
src/lib.rs     public core, engines, client, mcp and cli modules; production panic deny
src/main.rs    CLI binary entry point (requires cli)
src/core/      protocol-free engine (`search::core::Search`)
  config/      inert settings, defaults, loading and validation; Adapter/Http/Command
  fetch/       guarded requests, redirects, body caps; Page is the clean result
    guard.rs   the SSRF guard and resolver — the security-critical file
    extract/   main content, visibility filtering and passage selection
    cache.rs   transient in-memory pages
  text.rs      text processing and passage selection
  service.rs   the in-process Search service
src/engines/   Engine, Pool, Adapter; manifests, catalog, store and package leases
src/client/mod.rs  shared local/remote execution and pinned HTTPS
src/mcp/       mod.rs tools and stdio transport; http.rs streamable HTTP transport
src/cli/       CLI and paired HTTPS host
  args.rs      command parsing
  run.rs       dispatch and serve overrides
  http.rs      JSON API and device authentication
  auth.rs      persistent TLS identity, pairing and private credential storage
  remote.rs    profile selection and device management
  engines.rs   local package lifecycle, settings and diagnostics
engines/       shipped declarative assets: mwmbl/engine.json, searxng/engine.json
  README.md    package and private-store contracts
tests/        integration tests and fixtures under cli/ and engines/
examples/engines/json-post/  custom executable starter, outside the catalog
scripts/guard.sh  security-surface audit: hosts, panic sites, SSRF and auth
scripts/release.sh · scripts/formula.sh · install.sh  release tooling
docs/          user pages and docs/contributing/ for contributors
x              repository entry point
```

`--no-default-features` builds core, engines and client without protocol or CLI
dependencies. `--no-default-features --features mcp` adds MCP only. The default
`cli` feature includes `mcp` and optional hosting/authentication dependencies;
`mcp` enables optional `rmcp`. Installed packages remain in `~/.search/engines`.

## Conventions

- **Shipped code denies explicit panic sites.** `src/lib.rs` denies
  `clippy::unwrap_used`, `expect_used`, `panic`, `unreachable`, and
  `indexing_slicing`. Every allowed site carries a `proof:` comment or a scoped
  `#[allow]` explaining why runtime input cannot reach it. `scripts/guard.sh`
  enforces the same rule.
- **The core is protocol-free.** `search::core::Search` is the public in-process
  engine. MCP and CLI depend on core; core must not depend on either. Enforce
  this module direction through conventions and boundary tests.
- **The security-critical file is `src/core/fetch/guard.rs`.** A URL is model-chosen, so
  every class of address that can reach infrastructure must be classified
  private, and `check_host` strips IPv6 brackets before parsing an address. The
  guard's tests carry the counterexamples; do not loosen them.
- **All engines are packages.** Mwmbl is the single embedded keyless default.
  Catalog and local packages use the same store and execution contract. Preserve
  manifest schema 1 and command protocol 1 across Search releases; incompatible
  changes require a new contract version with continued version-1 support.
  Binary upgrades must not overwrite packages or settings. Document any security
  exception and its migration.
  HTTP adapters read credential headers from
  configured environment variables. Executable adapters are trusted host code,
  not a sandbox; only declared environment credentials are passed through. An
  engine's `EngineState` reports its `EngineStatus`, so an empty answer is never
  mistaken for a broken one.
- **Never discover project plugins implicitly.** `SEARCH_HOME` selects the root,
  defaulting to `~/.search`; explicit installs are the trust boundary.
  Management is local-only. Package inspection never runs code; updates preserve
  user settings and refuse edits or active package leases. Trust stays under home/trust,
  separate from shareable settings.
- **Don't hardcode what a user might change.** Timeouts, result counts, the user
  agent, and the enabled engines are read from configuration with built-in
  defaults. A new user-facing behaviour becomes a config field, not a constant.
- **Keep the vocabulary consistent.** `search::core` exposes `Search`, `Config`,
  `Query`, `Answer`, `Page`, and `Link`. `search::engines` exposes `Engine`,
  `Pool`, runtime status types, and catalog/install package APIs. Fetch returns
  `Page`; user fields and CLI selection use `engines` and `-engines`. Package
  `adapter` fields describe transport configuration. Use native module imports,
  without old facade aliases or separate client/engine crate names.
- **Separate settings from execution.** `Config` is inert data, without resolved
  transports or leases. Transport settings are `core::config::Adapter`,
  `core::config::Http`, and `core::config::Command`. `Pool` owns running adapters
  and package leases; HTTP engines reuse one client. CLI authentication lives in
  `src/cli/auth.rs`; on-disk trust filenames stay unchanged.
- **No persistent corpus.** Search and fetch use live engines and a transient
  fetch cache. Old corpus settings are rejected; existing files/data are untouched.
- **Comment modules and functions with their purpose and contract.** Avoid
  line-by-line comments. Update a comment when the behavior it describes changes.
- **One concern per PR.** The branch is `<type>/<slug>`; the title is
  conventional (`fix(search): ...`), because it becomes the squash commit on main.
- Anything user-visible gets a `CHANGELOG.md` entry under `Unreleased`.
