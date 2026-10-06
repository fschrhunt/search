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

`./x check` is not optional. It runs `cargo fmt --check`, `cargo clippy -D
warnings`, the workspace tests, a syntax check of every shell script, and
`scripts/guard.sh`.

## Where things live

Every Rust crate is a folder under `crates/`. `cli` and `mcp` depend on the
`search` engine; the engine depends on neither frontend nor protocol.

```
crates/search/  the engine library (`search`)
  config/       the settings surface: settings.rs (the shape and its invariants),
                defaults.rs (built-in values for omitted fields),
                load.rs (read, merge, validate). Credentials belong to the adapters;
                settings remain shareable.
  discovery/    the provider fan-out: mod.rs (Finding, Query, Response, the
                Provider trait), registry.rs (parallel fan-out, per-provider
                deadlines, reciprocal-rank fusion, URL normalization),
                parse.rs (defensive HTML scanners), adapters.rs (configured HTTP
                and executable engines), web.rs (the seven built-ins:
                brave · marginalia · mwmbl · wikipedia · hackernews ·
                stackexchange · arxiv — each keyless)
  fetch/        the fetcher: mod.rs (the guarded request, redirect following,
                body caps, indexing), guard.rs (the SSRF guard and the resolver
                — the security-critical file), extract/ (main-content
                extraction, the visibility pass, passage selection),
                cache.rs (recent answers)
  index/        the server-local corpus: mod.rs (Store over rusqlite, FTS search),
                schema.rs (the tables, triggers, and the FTS query builder)
  service.rs    the in-process `Search` engine and its public operations
crates/mcp/   the optional MCP adapter (`search_mcp`)
  lib.rs        the `web_search` and `web_fetch` tools, stdio transport
  http.rs       the streamable HTTP MCP transport
crates/cli/   the `cli` package and `search` binary (`search_cli` library)
  args.rs       parse the command line
  run.rs        dispatch, and build the service with serve overrides
  http.rs       paired HTTPS, JSON API and device auth middleware
  trust.rs      persistent TLS identity, pairing and private credential storage
  remote.rs     local profile selection and device management
scripts/guard.sh  the security-surface audit: allowed hosts, panic-site policy,
                  the SSRF guard's presence, and the auth layer
scripts/release.sh · scripts/formula.sh · install.sh  the release path; the
                  tag workflow in .github/workflows/release.yml runs them
docs/          docs/: user pages, and docs/contributing/ for working on search
x             the one repository entry point
```

## Conventions

- **Shipped code denies explicit panic sites.** `crates/search/src/lib.rs` denies
  `clippy::unwrap_used`, `expect_used`, `panic`, `unreachable`, and
  `indexing_slicing`. Every allowed site carries a `proof:` comment or a scoped
  `#[allow]` explaining why runtime input cannot reach it. `scripts/guard.sh`
  enforces the same rule.
- **The engine is protocol-free.** `search::Search` is the public in-process
  engine. The MCP adapter and CLI depend on it; it does not depend on either.
- **The security-critical file is `fetch/guard.rs`.** A URL is model-chosen, so
  every class of address that can reach infrastructure must be classified
  private, and `check_host` strips IPv6 brackets before parsing an address. The
  guard's tests carry the counterexamples; do not loosen them.
- **Built-ins are keyless.** Custom HTTP adapters read credential headers from
  configured environment variables. Executable adapters are trusted host code,
  not a sandbox. A provider's failure is reported in its `ProviderState`, so an
  empty answer is never mistaken for a broken one.
- **Don't hardcode what a user might change.** Timeouts, result counts, the user
  agent, and the enabled providers are read from configuration with built-in
  defaults. A new user-facing behaviour becomes a config field, not a constant.
- **Comment modules and functions with their purpose and contract.** Avoid
  line-by-line comments. Update a comment when the behavior it describes changes.
- **One concern per PR.** The branch is `<type>/<slug>`; the title is
  conventional (`fix(search): ...`), because it becomes the squash commit on main.
- Anything user-visible gets a `CHANGELOG.md` entry under `Unreleased`.
