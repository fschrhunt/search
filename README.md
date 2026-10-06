<div align="center">
  <img src="assets/white/lockup.svg#gh-dark-mode-only" alt="Search" height="52">
  <img src="assets/black/lockup.svg#gh-light-mode-only" alt="Search" height="52">

  <h3>Search the web. Build your own index as you go.</h3>

  <p>Self-hosted web search for people and AI agents.<br>
  Find pages across independent providers, read them cleanly, and keep what you fetch in a private index.</p>

  <p>
    <a href="docs/install.md">Install</a> ·
    <a href="docs/usage.md">Usage</a> ·
    <a href="docs/configuration.md">Configuration</a>
  </p>
</div>

<br>

## Find, read, keep

- **Find:** run installed engine packages in parallel and merge their results.
  Mwmbl is the single keyless default; a failed engine is reported rather than
  hidden. [Install maintained or custom engines](docs/engines.md) without rebuilding
  Search. Packages live in `~/.search/engines` and use one public adapter contract.
- **Read:** fetch pages through an SSRF-protected reader that strips page
  clutter. Ask for passages relevant to a query instead of a whole article.
- **Keep:** fetched pages join a local SQLite full-text index. Search blends
  local matches with live results, so useful pages remain searchable when
  providers are unavailable. Size and age limits keep the index bounded.

Local search and saving fetched pages can each be switched off. Seeded hosts
refresh only when configured, and only through the explicit refresh command—
Search does not crawl the web on its own.

## Start

Build from source with Rust:

```sh
git clone https://github.com/fschrhunt/search
cd search
./x build --release
```

Search from your terminal—no server required:

```sh
./target/release/search "rust async runtime"
./target/release/search fetch https://www.rust-lang.org -query "async"
./target/release/search index "async runtime" -json
```

Or host the HTTPS API and MCP server:

```sh
./target/release/search serve
```

`search serve` creates a persistent TLS identity and prints its fingerprint and
one-use pairing code. It listens on `127.0.0.1:8642` by default. Local CLI and
stdio MCP work without pairing. To share one host, pair each client and select
it with `search remote use NAME`; the same CLI and stdio tools then execute on
that host. Remote failures are errors with no local fallback. See
[remote hosting](docs/remote.md) and [installation](docs/install.md).

## Use it with an agent

With no arguments, `search` serves MCP over stdio. Add it to your MCP client:

```json
{
  "mcp": {
    "servers": {
      "search": { "command": ["search"] }
    }
  }
}
```

The server exposes two tools: `web_search` for discovery and `web_fetch` for
clean, query-focused reading. For a shared host, select a paired remote; the agent configuration stays the same.

## One engine, more surfaces

| Surface | Use it for |
| --- | --- |
| CLI | One-shot search, fetch, and local-index queries |
| Rust | Use `search::Search` as an in-process engine |
| HTTP | Integrate with scripts and services; includes status, search, index, and fetch endpoints |
| MCP | Give an agent the `web_search` and `web_fetch` tools |

The engine API lives in the `search` workspace crate. The `cli` and `mcp`
workspace packages are separate adapters; MCP dependencies are not part of the
engine crate. In Rust, start with `use search::{Config, Search};` and open a
configured engine with `Search::open(config)`.

## Make it yours

Settings live in `~/.search/settings.json` or the file named by `CONFIG`.
`SEARCH_HOME` selects the Search home, including packages, data, and private trust
storage; it defaults to `~/.search`. Defaults are useful; turn features off or tune
them as needed. No packages are implicitly loaded from the working directory.

```json
{
  "fetch": { "max_stored_chars": 40000 },
  "index": {
    "include_in_search": true,
    "save_fetched_pages": true,
    "max_size_mb": 512,
    "retention_days": 180,
    "refresh_hosts": [],
    "refresh_interval_days": 7
  }
}
```

Set `index.include_in_search` or `index.save_fetched_pages` to `false` to
disable that behavior. Set `max_size_mb` or `retention_days` to `0` for no limit.
See the full [configuration reference](docs/configuration.md).

Set `index.enabled` to `false` for no local database access at all. This leaves
existing indexed pages untouched; paired HTTPS credentials are stored separately.
Set `engines.enabled` to `false` to run without live web engines, or tune
`remote.timeout` for slower paired hosts.

To stop blending local pages into web results and stop saving newly fetched
pages, set both options to `false`:

```json
{
  "index": { "include_in_search": false, "save_fetched_pages": false }
}
```

## API

Every execution request uses a paired device credential over pinned HTTPS.
Pairing alone is public and requires an expiring, one-use code.

```text
GET  /healthz                 liveness
GET  /v1/status               providers, corpus size, version
GET  /v1/search?q=...         search providers and local index
GET  /v1/index?q=...          search only the local index
POST /v1/fetch {"urls":[...]} fetch pages and optionally index them
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
