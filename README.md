<div align="center">
  <img src="assets/white/lockup.svg#gh-dark-mode-only" alt="Search" height="52">
  <img src="assets/black/lockup.svg#gh-light-mode-only" alt="Search" height="52">

  <h3>Search the web. Read pages cleanly.</h3>

  <p>Self-hosted web search for people and AI agents.<br>
  Find pages across independent engines and read them as clean text.</p>

  <p>
    <a href="docs/install.md">Install</a> ·
    <a href="docs/usage.md">Usage</a> ·
    <a href="docs/configuration.md">Configuration</a>
  </p>
</div>

<br>

## Find and read

- **Find:** run installed engine packages in parallel and merge their results.
  Mwmbl is the single keyless default; a failed engine is reported rather than
  hidden. [Install maintained or custom engines](docs/engines.md) without rebuilding
  Search. Packages live in `~/.search/engines` and use one public adapter contract.
- **Read:** fetch pages through an SSRF-protected reader that strips page
  clutter. Read the full clean page or request passages relevant to a query.
  A transient in-memory cache reuses recent fetches.

Only Mwmbl and SearXNG ship with Search. Bring other services—including AI search
APIs—as [custom HTTP or command engines](docs/engines.md#custom-engines).

## Start

Download the official installer, review it, then run it:

The shell instructions below are for Linux and macOS. On Windows, use the
[PowerShell installer](docs/install.md#windows). All three systems have native
x86_64 and ARM64 CI and release targets.

```sh
curl -fsSL https://raw.githubusercontent.com/fschrhunt/search/main/install.sh -o install-search.sh
less install-search.sh
sh install-search.sh
export PATH="$HOME/.local/bin:$PATH"
```

Search from your terminal—no server required:

```sh
search version
search help
search "rust async runtime"
search fetch https://www.rust-lang.org -query "async"
```

See [installation](docs/install.md) for Homebrew, source builds, and installer
options. To host the optional HTTPS API and MCP server:

```sh
search serve
```

`search serve` creates a persistent TLS identity and prints its fingerprint and
one-use pairing code. It listens on `127.0.0.1:8642` by default. Local CLI and
stdio MCP work without pairing. To share one host, pair each client and select
it with `search remote use NAME`; the same CLI and stdio tools then execute on
that host. Remote failures are errors with no local fallback. See
[remote hosting](docs/remote.md) and [installation](docs/install.md).

## Use it with an agent

With no arguments, `search` serves MCP over stdio. This common `mcpServers`
template is for clients that accept that schema; use your client's documented
configuration file. The client process must be able to find `search` on its
`PATH`, or use the binary's absolute path:

```json
{
  "mcpServers": {
    "search": { "command": "search", "args": [] }
  }
}
```

The server exposes two tools: `web_search` for discovery and `web_fetch` for
clean page reading, with optional query-focused passages. For a shared host,
select a paired remote; the agent configuration stays the same.

## One engine, more surfaces

| Surface | Use it for |
| --- | --- |
| CLI | One-shot search and fetch |
| Rust | Use `search::core::Search` as an in-process engine |
| HTTP | Integrate with scripts and services; includes status, search, and fetch endpoints |
| MCP | Give an agent the `web_search` and `web_fetch` tools |

Search is one Cargo package. Use `search::core::{Search, Config, Query, Answer, Page, Link}` in-process, `search::engines` for runtime and package APIs,
`search::client::Client` for local/remote routing, and `search::mcp::Server` for MCP.
Disable default features for a library without CLI or protocol dependencies:

```toml
[dependencies]
search = { path = "../search", default-features = false }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Use a local checkout at `../search`. This example searches with the configured
engines and prints result links:

```rust
use search::core::{Config, Query, Search};

/// Search the default engines and print their result links.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let search = Search::open(Config::default())?;
    let answer = search
        .search(Query {
            text: "rust async runtime".into(),
            limit: 5,
            ..Query::default()
        })
        .await;
    for link in answer.results {
        println!("{}: {}", link.title, link.url);
    }
    Ok(())
}
```

The default `cli` feature includes MCP and hosting/authentication dependencies;
`default-features = false, features = ["mcp"]` enables MCP only. See the
[architecture guide](docs/contributing/architecture.md) for module boundaries
and the tradeoff in enforcing them.

## Make it yours

Settings live in `SEARCH_HOME/settings.json` (normally `~/.search/settings.json`)
or the file selected by `-config PATH` or `CONFIG`.
`SEARCH_HOME` selects the package, settings, and private trust root, defaulting
to `~/.search`. Packages are never implicitly loaded from the working directory.

```json
{
  "engines": { "use": ["mwmbl"] },
  "fetch": { "cache_ttl": 600000 }
}
```

Install and enable optional engines before selecting them; credentials come from
host environment variables. See [engine packages](docs/engines.md) and
[configuration](docs/configuration.md).

Search has no persistent corpus. Upgrading from the former index-based setup?
See [migration notes](docs/configuration.md#migration) for removed settings and
commands; existing files and data are left untouched.

## API

All API and MCP routes, including `/healthz`, require a paired device bearer
credential over HTTPS. Search clients pin the host certificate. Only `/pair`
is public; it requires an expiring, one-use code.

```text
GET  /healthz                 liveness
GET  /v1/status               engines, version
GET  /v1/search?q=...         search live web engines
POST /v1/fetch {"urls":[...]} fetch clean pages
POST /v1/execute              CLI/stdio operations
POST /mcp                     MCP over streamable HTTP
```

## Security

Fetched URLs are untrusted. Search blocks private and metadata-network
destinations, re-checks redirects, limits response size, and enforces deadlines.
Keep a network-facing instance on a private network, behind authentication; do
not expose it to the public internet. Read the [security notes](docs/security.md)
before deployment.

## Project

Search is early software. See the [docs](docs/README.md), [changelog](CHANGELOG.md),
and [contributing guide](CONTRIBUTING.md). Licensed under MIT.
