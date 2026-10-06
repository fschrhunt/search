# Usage

Search runs selected web engines concurrently, merges their results, and reads
pages through a guarded fetcher. Recent pages are cached transiently in memory.
Install Search and make it available on your `PATH` first; see
[installation](install.md). Local commands need no server or pairing.

```sh
search help
search version
search "rust async runtime"
search fetch https://www.rust-lang.org
search fetch https://www.rust-lang.org -query "async"
```

Install optional engines with `search install NAME`; see [engine packages](engines.md).
Only Mwmbl and SearXNG ship. Before selecting SearXNG, run your own instance
with JSON output enabled, then install, configure, and enable its package. For
an instance already listening on `127.0.0.1:8080`:

```sh
search install searxng
search configure searxng url http://127.0.0.1:8080/search
search configure searxng allow_private_networks true
search enable searxng
search "rust async runtime" -engines mwmbl,searxng -json
```

That private-network permission applies only to the SearXNG adapter, not page
fetching. With a selected remote, set up engines on the host instead.

Command names are reserved as the first argument. Use `search -- test driven development`
to search for words that would otherwise select a command.
Package management and diagnostics always run locally, even with a selected remote.

## MCP

With no arguments, Search serves MCP over stdin/stdout. This common template
applies to clients that accept `mcpServers`; consult your client's documentation
for the configuration file location and schema. Ensure its process can find
`search` on `PATH`, or replace `command` with the binary's absolute path:

```json
{ "mcpServers": { "search": { "command": "search", "args": [] } } }
```

- **`web_search`** takes one to five queries, a result limit, and an optional
  `engines` list. It returns ranked titles, URLs, and snippets, plus each engine's
  status, distinguishing successful empty results from failures and timeouts.
- **`web_fetch`** takes one to ten URLs. With neither `query` nor `max_characters`,
  it returns the full clean page under the configured fetch transport body bound
  (`fetch.max_response_bytes`). An explicit `max_characters` must be 1–40000.
  A `query` selects relevant passages, defaulting to 6000 characters per page
  unless an explicit limit is supplied.

Fetched pages include `fetched_at`, the Unix timestamp of Search's fetch, distinct
from the page's publication date. Reusing a cached page retains its fetch time.

## Paired HTTPS

```sh
search serve
```

The default listener is `127.0.0.1:8642`; set `address` for a private interface.
The JSON API and MCP endpoint share this listener. [Pair clients](remote.md) to
route CLI and stdio MCP to the host. Remote failures are errors with no fallback.

All routes below, including health, require a per-device bearer credential over
HTTPS. Search clients pin the host certificate; `/pair` is the only public route:

```text
GET  /healthz                 liveness
GET  /v1/status               engines and version
GET  /v1/search?q=...         live web search
POST /v1/fetch {"urls":[...]} clean pages
POST /v1/execute              CLI/stdio search and fetch operations
POST /mcp                     MCP over streamable HTTP
```

`GET /v1/search` also accepts `limit` (clamped to 1–50) and `engines`
(comma-separated names). The host applies its engine configuration, credentials,
and fetch policy; `search.max_results` can further cap the result count.

## Clean pages

Extraction removes ads, cookie banners, related rails, comment sections, and
visually hidden text. Links become text and images become alt text, preventing
page content from carrying active image links to a consuming client. Pages that
only redirect elsewhere are followed through the same fetch guard. Private and
link-local destinations are refused; see [security](security.md).
