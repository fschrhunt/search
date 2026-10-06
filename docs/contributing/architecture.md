# Architecture

Search is one root Cargo package, with library `search` (`src/lib.rs`) and
CLI binary `search` (`src/main.rs`). There is no workspace, facade, or separate
client/engine crate. Public APIs use the native modules:

| Module | Contract / source |
| --- | --- |
| `search::core` | Protocol-free `Search`, `Config`, `Query`, `Answer`, `Page`, `Link`; `src/core/{config,fetch,text,service}` |
| `search::engines` | `Engine`, `Pool`, runtime adapters, manifest/catalog/install/store APIs; `src/engines/` |
| `search::client` | `Client` for shared local/remote execution and pinned HTTPS; `src/client/mod.rs` |
| `search::mcp` | `Server`, tools and stdio transport in `src/mcp/mod.rs`; streamable HTTP in `src/mcp/http.rs` |
| `search::cli` | Commands and paired HTTPS host; `src/cli/{args,run,http,auth,remote,engines,..}` |

Root `engines/{mwmbl,searxng}/engine.json` holds shipped declarative assets.
Root `build.rs` embeds the catalog and release identity. Installed packages stay
under `SEARCH_HOME/engines` (normally `~/.search/engines`). Integration fixtures
live under `tests/{cli,engines}`; the custom starter remains at
`examples/engines/json-post`.

## Features and dependency direction

| Cargo features | Build |
| --- | --- |
| Default (`cli`) | Library and CLI binary; `cli` includes `mcp` and optional hosting/authentication dependencies |
| `--no-default-features` | Core, engines and client library without protocol or CLI dependencies |
| `--no-default-features --features mcp` | Library plus MCP only; `mcp` enables optional `rmcp` |

Core and engines supply discovery, fetching, configuration and package execution;
client routes local/remote operations; MCP and CLI use these lower modules. Core
must remain independent of MCP and CLI. The single package simplifies builds and
removes cross-crate plumbing, but compilation no longer enforces layer boundaries.
Module conventions and boundary tests enforce this direction instead. `./x check`
checks all features, minimal features, and MCP-only features as well as package
tests, formatting, linting, shell syntax and the security-surface guard.

Rust import changes are breaking independently of engine compatibility. Manifest
schema 1, command protocol 1 and on-disk formats remain unchanged; upgrading the
binary does not replace installed engines, settings or credentials.

## Core

`search::core::Search` is the in-process service. Core exposes `Config`, `Query`,
`Link`, `Answer`, and `Page`. `search::engines` defines `Engine`, `EngineState`,
and `EngineStatus` alongside runtime and package APIs. `Pool` runs selected
engines concurrently with per-engine and whole-query deadlines, normalizes URLs
and merges results using reciprocal-rank fusion. `Adapter` implements the
packaged HTTP and executable transports. `Pool` owns running adapters and package leases; HTTP engines reuse
one client. Engine-specific behavior belongs in manifests and programs.

The fetcher returns `Page`: guarded retrieval, bounded bodies, clean extraction,
and optional passage selection. Its cache is transient and in memory. There is
no persistent corpus, indexing, or refresh. The `core::config` module owns shareable
settings, defaults, and validation; credentials belong to adapters and private
trust storage. `Config` is inert data, without resolved transports or leases.
Transport settings are `core::config::Adapter`, `core::config::Http`, and
`core::config::Command`.

User selection and attribution use `engines`; the CLI flag is `-engines`.
Package `adapter` is a distinct transport field. For removed settings and
commands, see the [migration guide](../configuration.md#migration).

## Routing and trust

CLI and stdio MCP use `search::client::Client` for exclusive local/remote execution.
A selected remote is checked before opening a local engine; errors never trigger
local fallback. Hosting always opens the local engine. Pairing verifies a copied
public certificate's fingerprint before sending the one-use code. HTTPS checks
the exact leaf pin plus standard WebPKI validation. Host trust stores credential
hashes; client trust stores secrets. CLI identity, pairing, and credential storage
live in `src/cli/auth.rs`; on-disk trust filenames are unchanged. See
[remote hosting](../remote.md).

Packages are installed locally from the maintained catalog or an explicit
directory, never from request input or implicit working-directory discovery.
`SEARCH_HOME` owns packages, settings, and private trust. Inspection never runs
code; updates preserve user settings and refuse modified packages or active leases.
The shipped catalog contains only Mwmbl and SearXNG. Shipped and custom packages
share the version-1 manifest and command protocol and the same installation store.
HTTP adapters reuse the SSRF resolver, refuse redirects and proxies, and require
independent permission for private networks. Executable adapters are trusted host
code; only declared credentials are inherited, and cancellation kills the direct
child. See [engine packages](../engines.md).

## Invariants

- `search::core` has no frontend protocol dependency; MCP owns tools and transports.
  Minimal library builds do not compile MCP or CLI dependencies.
- `src/core/fetch/guard.rs` classifies infrastructure addresses as private and strips IPv6
  brackets before parsing. Preserve its counterexample tests and guard call sites.
- Root `src/lib.rs` denies explicit panic sites in production across its modules;
  allowed sites need a scoped explanation or `proof:` comment.
  `scripts/guard.sh` audits this surface.
- Mwmbl is the single keyless default. Each `EngineState` reports an `EngineStatus`.
- User-facing limits and enabled engines come from configuration.
