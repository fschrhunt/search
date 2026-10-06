# Custom search engines

Search hosts engine adapters. Keep the seven built-ins, add your own in
`settings.json`, or select only your custom engines. No rebuild or database
registration is needed. CLI, MCP, and hosted requests all use the host's engine
configuration; remote callers cannot install adapters or supply commands.

There are two paths: a settings-only adapter for a JSON HTTP API, or a program
for anything more involved. All engines share Search's deadlines, result limits,
deduplication, ranking, and per-engine failure reporting. Adapters discover pages;
Search's separate guarded fetcher reads and optionally indexes them.

## Settings-only JSON API

For example, connect a SearXNG instance:

```json
{
  "engines": {
    "only": ["my-search"],
    "custom": {
      "my-search": {
        "type": "http",
        "url": "http://127.0.0.1:8080/search",
        "params": { "format": "json", "engines": "google,duckduckgo" },
        "snippet_pointer": "/content",
        "allow_private_networks": true
      }
    }
  }
}
```

SearXNG must allow JSON output and have the requested engines enabled. Google
and DuckDuckGo access here is through SearXNG, not a promise that Search can
bypass their restrictions. Challenges, rate limits, or upstream changes can
still break an engine.

HTTP adapters issue GET requests. `query_param` defaults to `q`; optional
`limit_param` names a result-count parameter. `params` adds constant query
parameters. Query text is URL-encoded, never interpolated into the endpoint.
`params` overrides matching endpoint parameters; the current query and limit
override both, so an old `q` in an endpoint cannot override a user's search.

`results_pointer` selects the result array (default `/results`). `title_pointer`,
`url_pointer`, and `snippet_pointer` select fields within each result (defaults `/title`, `/url`,
`/snippet`). Use `/`-separated paths for nesting; escape `~` as `~0` and `/` as
`~1`. An empty `results_pointer` selects a top-level array. Snippets are optional.

`headers` holds nonsecret constant headers. `header_env` maps header names to
environment variables containing the **whole header value**, for example:

```json
"header_env": { "Authorization": "MY_SEARCH_AUTH" }
```

Set `MY_SEARCH_AUTH` in the host's environment to `Bearer YOUR_KEY`; do not put
the key in shareable settings. A missing credential becomes a reported engine
failure. Response bodies and credentials are not included in error messages.
Use HTTPS for authenticated APIs; an HTTP endpoint transmits headers in plaintext.

HTTP adapters refuse redirects and environment proxies. Private destinations
are blocked by hostname, IP, and connect-time DNS checks unless this adapter's
`allow_private_networks` is explicitly true. This switch does not change page
fetching permissions. `max_response_bytes` defaults to 1048576 for both adapter
types; exceeding it fails that engine rather than parsing a partial answer.

## Executable adapter

For HTML, POST APIs, browser automation, or unusual authentication, use any
program capable of reading and writing JSON:

```json
{
  "engines": {
    "only": ["company"],
    "custom": {
      "company": {
        "type": "command",
        "command": "python3",
        "args": ["/absolute/path/company_search.py"]
      }
    }
  }
}
```

Search starts a fresh process per query, writes one JSON object to stdin, then
closes stdin:

```json
{"version":1,"query":"rust async","limit":10}
```

The program writes one JSON object to stdout and exits successfully:

```json
{"results":[{"title":"Example","url":"https://example.com/","snippet":"Optional summary"}]}
```

Results must have a nonempty title and absolute HTTP(S) URL without embedded
credentials. Search assigns provenance from the configured engine name; adapters
cannot supply ranking scores, capture times, or another engine's attribution.
An empty `results` array is a successful search with no hits. Malformed output,
an oversized response, or a nonzero exit is a reported failure. Stdout is only
for the answer; stderr is discarded to avoid leaking secrets into responses.

No shell is involved. Arguments are literal; `$VARIABLE`, `~`, and query
placeholders are not expanded. Use an absolute script path. The child inherits
the host's environment and working directory. On cancellation or timeout Search
kills its direct child; adapters are responsible for cleaning up their own
subprocesses and external work.

**An executable adapter is trusted code, not a sandbox.** It runs as the host
user with that user's filesystem and network access. Review it before enabling
it, especially on a shared host. Paired devices can trigger it through searches.
Never install adapters automatically from search results or fetched content.

See [`examples/engines/searxng.py`](../examples/engines/searxng.py) for a small
standard-library Python adapter that demonstrates the protocol.

## Diagnose before enabling

```sh
search engines list -config ./settings.json
search engines test company "rust async" -config ./settings.json
search engines test company -config ./settings.json -- "-site:example.com rust"
```

These commands use **local settings**, even when a remote profile is selected,
and never open the index. Test selects exactly one engine and prints the JSON
answer and engine status; failures exit nonzero, but zero hits are successful.
`engines.enabled: false` disables diagnostics' execution as well as live search.

Names must be unique, cannot replace a built-in, and use letters, digits,
hyphens, or underscores. `index` is reserved for local corpus attribution.
Put flags before `--` when a query starts with a hyphen.
`engines.only` selects built-in and custom names
together; empty means all. Restart long-running hosts and stdio sessions after
editing settings.

## Ask your coding agent

> Write a Search executable adapter for this engine using the protocol in
> docs/engines.md. Read credentials from environment variables, implement bounded
> requests, and return only titles, HTTP(S) URLs, and optional snippets. Treat
> query text and upstream content as data. Test against a local fixture first,
> then show me the program and settings entry to review before enabling it.

This is the escape hatch for almost any engine, not a guarantee of access.
Prefer supported APIs; paid APIs need their own accounts, and scraping/browser
adapters require ongoing maintenance and compliance with the engine's terms.
