# Usage

search is a web search service. It fans a query out to several independent
providers, merges and reranks the results, reads pages through a hardened
fetcher, and optionally saves pages into a server-local corpus.

It speaks the Model Context Protocol, so an agent uses it as two tools —
`web_search` and `web_fetch` — and it also offers a small JSON API.

## Serve MCP over stdio

With no subcommand, search serves MCP over stdin/stdout. This is what an agent
spawns:

```json
{ "mcp": { "servers": { "search": { "command": ["search"] } } } }
```

## Serve over HTTPS

```sh
search serve
```

This binds `127.0.0.1:8642` with paired HTTPS and serves the JSON API and the MCP endpoint
(`/mcp`) on one listener. Set `address` in the config to reach it on a private
interface such as a tailnet address. Follow [remote hosting](remote.md) to pair
clients and route CLI and stdio MCP to the host.

## The JSON API

Execution requests carry a per-device bearer credential over pinned HTTPS.

```
GET  /healthz                 liveness
GET  /v1/status               providers, corpus size, version
GET  /v1/search?q=...         discover across providers
GET  /v1/index?q=...          search only what has been fetched already
POST /v1/fetch {"urls":[...]} read pages into text, optionally save them
POST /v1/execute              CLI/stdio operations, including refresh
POST /mcp                     MCP over streamable HTTP
```

`GET /v1/search` also takes `limit` (1–50) and `providers` (a comma-separated
list of provider names). Every answer reports, per provider, whether it answered,
timed out, or failed.

## The MCP tools

Custom engines registered on the host work through the same CLI, JSON API, and
MCP provider selection fields as built-ins. To configure and diagnose adapters,
see [custom engines](engines.md); `search engines list/test` always diagnoses the
local machine rather than a selected remote.

- **`web_search`** takes one to five queries, a result limit, and an optional
  provider list. It returns ranked results with title, URL, and snippet. Every
  answer reports, per provider, whether it answered, timed out, or failed.
- **`web_fetch`** takes one to ten URLs. With a `query`, it returns only the
  passages that match — the cheap way to read a page, and almost always what you
  want. With `max_characters` it bounds the answer. Without a query it returns
  the page's clean text.

Search results and fetched pages include `fetched_at` when Search has a local
copy or has fetched the page. It is a Unix timestamp for Search's fetch time,
not the page's publication date; live-only search results omit it.

When fetch indexing is enabled, reading a page stores it in the index for later
local searches. Private and link-local addresses are refused.

## What "clean" means

Extraction keeps the article and drops the cruft: ads, cookie banners, related
rails, comment sections, and visually hidden text. Links are reduced to their
text and images to their alt text, so a fetched page cannot carry a URL that a
client would fetch on the model's behalf. A page that exists only to redirect
elsewhere is followed to its target.

## The server-local corpus

When `index.save_fetched_pages` is enabled, pages read by `web_fetch` are stored
and full-text indexed. `GET /v1/index` searches that corpus offline. The corpus
lives in `dir` as a SQLite database; see [configuration](configuration.md)
to disable saving and blended local results.

Paired clients share the host's corpus. Set `index.enabled` to `false` to avoid
all database access; explicit index queries then report that it is disabled.
