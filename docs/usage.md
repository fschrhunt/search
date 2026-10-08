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

Engines are defined in settings; see [engines](engines.md). Mwmbl and SearXNG
are built in. Before selecting SearXNG, run your own instance with JSON output
enabled, then configure and enable it. For an instance already listening on
`127.0.0.1:8080`:

```sh
search configure searxng url http://127.0.0.1:8080/search
search configure searxng allow_private_networks true
search enable searxng
search "rust async runtime" -engines mwmbl,searxng -json
```

That private-network permission applies only to the SearXNG adapter, not page
fetching. With a selected remote, set up engines on the host instead.

Command names are reserved as the first argument. Use `search -- test driven development`
to search for words that would otherwise select a command.
Engine management and diagnostics always run locally, even with a selected remote.

## MCP

With no arguments, Search serves MCP over stdin/stdout. This common template
applies to clients that accept `mcpServers`; consult your client's documentation
for the configuration file location and schema. Ensure its process can find
`search` on `PATH`, or replace `command` with the binary's absolute path:

```json
{ "mcpServers": { "search": { "command": "search", "args": [] } } }
```

- **`web_search`** takes one to five queries, an optional result limit, and an
  optional `engines` list. An omitted limit uses `search.max_results` (normally
  10), which also caps larger requests; no request exceeds 50. It returns ranked
  titles, URLs, and snippets, plus each engine's status, distinguishing
  successful empty results from failures and timeouts.
- **`web_fetch`** takes one to ten URLs and reads HTML, plain text (including
  JSON, XML and source files) and PDF text layers; images and other binaries are
  refused by type. With neither `query` nor `max_characters`, it returns the whole
  clean page; only `fetch.max_response_bytes` (16 MiB) bounds the download, and a
  page cut there reports `truncated`. `max_characters` reads a window instead: a
  page that continues reports `next_offset`, which you pass back as `offset` to
  read on. A `query` returns the matching passages in reading order (6000
  characters per page unless `max_characters` says otherwise); when nothing
  matches, the opening of the page is returned instead.

Fetched pages include `fetched_at`, the Unix timestamp of Search's fetch, distinct
from the page's publication date. Reusing a cached page retains its fetch time.

## Paired HTTPS

```sh
search serve
```

The default listener is `127.0.0.1:8642`; set `address` for a private interface.
The JSON API and MCP endpoint share this listener. [Pair clients](remote.md) to
route CLI and stdio MCP to the host; paired clients use the same JSON API.
Remote failures are errors with no fallback.

All routes below, including health, require a per-device bearer credential over
HTTPS. Search clients pin the host certificate; `/pair` is the only public route:

```text
GET  /healthz                          {"status","version"}
GET  /v1/status                        {"version","engines"}
GET  /v1/search?q=&limit=&engines=a,b  search answer: results and engine status
POST /v1/fetch {"urls":[...]}          {"pages":[...]} in input order
POST /mcp                              MCP over streamable HTTP
```

`q` is required. An omitted `limit` uses the host's `search.max_results`
(normally 10), which also caps larger values; no request exceeds 50. `engines`
is an optional comma-separated list of package IDs. A fetch reports per-URL
failures in each page's `error` field. The host applies its engine
configuration, credentials, and fetch policy.

Every surface (CLI, MCP, and HTTP) shares the same input bounds: a query is
1–512 bytes after trimming, a fetch takes 1–10 URLs of at most 8192 bytes each,
and `web_search` takes 1–5 queries. The HTTP API answers violations with
`400 {"error": "..."}`.

## Clean pages

Extraction finds the article and returns it as Markdown — headings, lists,
tables and fenced code with its indentation intact — without navigation, ads,
cookie banners, related rails, or visually hidden text. Pages without an
article, such as listings, are read whole as plain text without their site
chrome. Text is decoded from the page's declared charset. Links become text and
images become alt text, and link reference definitions are broken, so page
content cannot carry an active image link to a consuming client.
A prompt `<meta http-equiv="refresh">` redirect is followed through the same
fetch guard; scripted redirects are not guessed at. The requested URL stays in
`url` and the retrieved destination is reported in `final_url`. Private and link-local
destinations are refused; see [security](security.md).

From the CLI, `search fetch URL -max-chars N` reads a window and prints where it
continues; `-offset N` reads on from there.
