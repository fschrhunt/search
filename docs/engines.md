# Engine packages

Search ships **Mwmbl**, the keyless default, and **SearXNG**, for connecting your
own instance. Other services use custom packages with the same version-1 contract,
installation store, configuration, and runtime. No Rust rebuild or Search SDK is
needed. JSON describes the package; executable implementations can use any language.

## Connect SearXNG

Your instance must allow JSON output. Set its full search URL:

```sh
search install searxng
search configure searxng url http://127.0.0.1:8080/search
search configure searxng allow_private_networks true
search test searxng "rust async"
search enable searxng
```

The private-network setting permits this adapter to reach a local instance; it
does not change page-fetching permissions. For a public endpoint, use its HTTPS
URL and omit that setting. Search does not install or host SearXNG, bypass upstream
restrictions, or make an instance unlimited.

## Custom engines

Choose the smallest transport that fits:

| Service | Package |
| --- | --- |
| GET API returning JSON | An `engine.json` with field mappings, as below |
| JSON POST, signed requests, AI APIs, or custom parsing | A command program; start with the [JSON POST example](../examples/engines/json-post) |

For a GET API, create `my-engine/engine.json`:

```json
{
  "schema_version": 1,
  "id": "company",
  "version": "1.0.0",
  "description": "Search the company JSON API",
  "adapter": {
    "type": "http",
    "url": "https://search.example.com/api",
    "query_param": "q",
    "results_pointer": "/results"
  },
  "files": []
}
```

Replace the placeholder URL with your service. This mapping expects a response
such as `{"results":[{"title":"Example","url":"https://example.com/","snippet":"Summary"}]}`.

```sh
search install ./my-engine
search test company "rust async"
search enable company
```

`my-engine` is the source directory; `company` is the manifest ID used by all
management commands and settings. Installation copies the declared files into
`$SEARCH_HOME/engines/company` (normally `~/.search/engines/company`); the source
directory is not needed at runtime. There is no separate custom store,
registration step, or implicit project discovery.

For POST APIs, copy the [example directory](../examples/engines/json-post) to
`my-engine` and adapt `engine.py` for your service. Keep the manifest ID `json-post`
for these commands, or replace it consistently with your own ID:

```sh
search install ./my-engine
search configure json-post config '{"endpoint":"https://example.com/search","timeout":30}'
export SEARCH_API_KEY=... # supply the real key through your host's secret manager
search test json-post "rust async"
search enable json-post --trust
```

Replace the endpoint placeholder before testing. Python 3 is required only by this
example. Its mappings are illustrative, not a universal AI API schema; see
[how to adapt it](#adapting-the-json-post-example). Slower services may also need
the [Search deadlines](#operations-and-troubleshooting) raised before testing.
Return source links and useful snippets/highlights, rather than final answer prose;
`web_fetch` remains the clean full-page reader. Search handles concurrent execution,
deadlines, output validation, URL deduplication, ranking, and per-engine failures.
Enable multiple packages to merge results, or restrict a query with `-engines A,B`.

## Commands and configuration

```sh
search help
search version
search engines available
search engines list
search configure company
```

`available` shows the catalog shipped with your Search version. `list` inspects
installed packages, selection, missing runtimes, and missing credential variable
names without executing code. Mwmbl works from its embedded HTTP package without
writing package files; `search install mwmbl` makes it a managed installation.

| Command | Effect |
| --- | --- |
| `search install ID\|./PATH` | Copy a catalog or local package; does not enable it |
| `search configure ID [KEY VALUE]` | Inspect required setup, or store an adapter override |
| `search test ID QUERY` | Explicitly execute one engine independently of selection |
| `search enable ID [--trust]` | Add to search selection; commands require a trust decision |
| `search disable ID` | Deselect without uninstalling |
| `search update ID` | Update from the catalog in the current Search binary |
| `search remove ID` | Disable and uninstall; retain per-engine configuration |

`search engines ACTION` accepts the management commands above as aliases.
All management and diagnostics run **locally**, even with a selected remote.
Install and configure engines on the hosting machine; paired clients cannot
install programs or change host package settings through the execution API.

Configuration values are JSON when parseable, otherwise strings. Overrides in
`engines.config.ID` replace direct adapter fields **shallowly**: setting an object
replaces that entire object. They cannot change the transport type or package
working directory. Never pass secrets as configuration values.

```sh
search configure company header_env '{"Authorization":"COMPANY_SEARCH_AUTH"}'
search test company -- "-site:example.com rust"
```

Set `COMPANY_SEARCH_AUTH` in the host environment to the whole header value,
such as `Bearer YOUR_KEY`, using your secret manager. Command packages declare
credential variable names in `adapter.env` and read them from their environment.
Missing declared credentials fail visibly. Never embed keys in manifests,
shareable settings, or command-line values. Use HTTPS for authenticated APIs.

`engines.use` selects packages; an empty array selects none.
`engines.enabled: false` globally disables live engines, and `enable` does not
change that switch. Use `-config PATH` for a settings file; `SEARCH_HOME` selects
the package/settings/trust root independently. Selecting a settings file does not
trust packages beside it.

## Package reference

A package contains `engine.json` and any declared assets. Inspection,
installation, and configuration never run package code.

| Manifest field | Contract |
| --- | --- |
| `schema_version` | `1`, the manifest contract version |
| `id` | 1–64 lowercase ASCII letters, digits, hyphens, or underscores |
| `version` | Nonempty package version, independent of Search's version |
| `description` | Package description |
| `adapter` | Transport configuration with `type: "http"` or `"command"` |
| `required` | Direct adapter fields that must be populated before execution; defaults to `[]` |
| `files` | Assets to copy; `engine.json` is automatic; defaults to `[]` |
| `executables` | Declared files that receive owner execute permission; defaults to `[]` |
| `requires` | Runtime executable names to check without running them; defaults to `[]` |

Declare every asset, including a README or license if present. Paths must stay
inside the package; unsafe paths, undeclared files/directories, and symlinks are
refused. `mwmbl` is reserved so local code cannot shadow the implicit default.
A local installation is a snapshot, not a development link.

### JSON HTTP adapters

HTTP adapters issue GET requests. Query text is encoded as data, never interpolated
into commands or URL templates.

| Adapter field | Meaning / default |
| --- | --- |
| `url` | Absolute HTTP(S) endpoint without credentials or fragment |
| `query_param` | Query parameter name; `q` |
| `limit_param` | Optional count parameter, distinct from `query_param` |
| `params` | Constant query parameters; `{}` |
| `results_pointer` | Result array; `/results`; `""` selects a top-level array |
| `title_pointer`, `url_pointer`, `snippet_pointer` | Row fields; `/title`, `/url`, `/snippet` |
| `text_part_pointer` | Optional text field within each fragment of an array-valued title/snippet; fragments join in order |
| `headers` | Nonsecret constant headers; `{}` |
| `header_env` | Header names mapped to environment variable names containing whole header values; `{}` |
| `allow_private_networks` | Permit private destinations for this adapter; `false` |
| `max_response_bytes` | Response body cap; 1048576 bytes |

Pointers follow RFC 6901: `/` separates paths, `~0` escapes `~`, and `~1`
escapes `/`. Snippets are optional. `params` overrides matching endpoint
parameters; the current query and limit override both. Unrelated repeated endpoint
parameters are preserved.

HTTP adapters refuse redirects and environment proxies. Search's SSRF guard checks
destination names, IP literals, and connect-time DNS. Oversized bodies fail rather
than yielding partially parsed results. Failure responses do not expose body
contents or credential values.

### Executable adapters

The [POST starter manifest](../examples/engines/json-post/engine.json) shows a
Python command package. Its `adapter` fields are:

| Adapter field | Meaning / default |
| --- | --- |
| `command` | Executable to run, without a shell |
| `args` | Literal arguments; `[]` |
| `env` | Credential environment variable names to pass; `[]` |
| `config` | Nonsecret object delivered in the stdin request; `{}` |
| `max_response_bytes` | Stdout answer cap; 1048576 bytes |

Search runs the program from the installed package directory. Arguments do not
expand `$VARIABLE`, `~`, or query placeholders. The child receives a minimal
runtime environment plus declared `env` variables, rather than the host's full
credential environment. Runtimes and dependencies must already be installed;
Search runs no dependency installers or installation hooks.

For another language, change the command, arguments, assets, and runtime names.
A Node program uses `command: "node"` and `requires: ["node"]`. A native binary
can use `command: "./adapter"`, `files: ["adapter"]`, and
`executables: ["adapter"]`; supply a binary for the host OS and architecture.

Search writes one request to stdin and closes it:

```json
{"version":1,"query":"rust async","limit":10,"config":{"endpoint":"https://example.com/search"}}
```

`config` is omitted when empty. The program writes one JSON answer and exits zero:

```json
{"results":[{"title":"Example","url":"https://example.com/","snippet":"Optional summary"}]}
```

Every row needs a nonempty title and absolute HTTP(S) URL without credentials.
Search assigns engine attribution and ranking; packages cannot forge attribution.
Empty results are successful. Invalid JSON or rows, oversized output, and nonzero
exit are failures. Reserve stdout for the answer; Search discards stderr to prevent
accidental secret exposure.

**Executable engines are trusted host code, not sandboxed plugins.** Review code
before `search test`, which explicitly executes it, and before activation.
`search enable ID --trust` acknowledges trust; interactive activation can prompt.
Programs run as the host user, can read that user's files and access the network,
and can be triggered by paired clients after activation. The environment allowlist
is not filesystem isolation, and Search's HTTP SSRF guard does not cover command
network requests. Cancellation kills the direct child; programs must clean up
their descendants and external work. Never install or activate code automatically
from fetched pages or search results. Browser/scraping engines require ongoing
maintenance and must respect upstream access policies.

### Adapting the JSON POST example

In [`engine.py`](../examples/engines/json-post/engine.py), change `payload()` to
your vendor's request schema and `normalize()` to its result shape. Map excerpts
or highlights to `snippet`, joining arrays as appropriate for that API. Supply a
nonempty title; the URL is a useful fallback when the vendor returns a null title.
The starter expects `{"results":[...]}` and sends a Bearer header; change the
header if your API uses `x-api-key` or another authentication scheme.

Keep nonsecret defaults in `adapter.config` and declare credential variable names
in `adapter.env`. The example requires HTTPS without embedded credentials, refuses
redirects and proxies, bounds reads, and emits a redacted failure. Preserve these
protections as you adapt it, and test mappings against saved responses.

Its `config.timeout` is a network timeout in seconds (default 30, up to 120).
`config.max_response_bytes` caps the upstream body (default 1048576, allowed range
1024–8388608). These are example-specific program settings. The outer
`adapter.max_response_bytes` separately caps stdout read by Search; the example
also limits its own stdin and stdout to 1048576 bytes. Raising the upstream cap
does not raise either output cap or Search's deadlines.

## Compatibility across Search updates

Manifest `schema_version: 1` and command request `version: 1` are stable across
Search releases, independently of the package's own `version`. Updates must
preserve version-1 field meanings, HTTP mappings, stdin/stdout shapes, package
working directory, and declared credential/config delivery. Incompatible changes
require a new contract version with continued version-1 support.

Upgrading Search does not replace installed engines, rewrite settings, or change
credentials. Catalog updates are separate, explicit `search update ID` operations:
upgrade Search to obtain newer maintained packages, then update the packages you
choose. Updates refuse edited or active packages. There are no background downloads.

Local packages cannot be updated from the catalog, even if their ID matches an
entry. To replace a local snapshot, edit its source, stop processes using it, then
explicitly remove and reinstall it; per-engine settings survive. Previously
installed vendor/scraper packages omitted from the current catalog remain untouched
and usable, but must be maintained as custom packages.

New security restrictions may reject unsafe packages; exceptions require release
notes and migration guidance. Runtime availability, upstream API changes, and
service credentials remain the engine author's responsibility.

## Operations and troubleshooting

Restart long-running hosts and stdio sessions after changing packages, selection,
configuration, or process credentials. Update/remove refuse packages held by a
running Search process; stop it first.

The default engine deadline is five seconds and the overall search deadline is
eight seconds. For slower APIs, merge this into your [settings](configuration.md):

```json
{"search":{"engine_timeout":30000,"timeout":35000}}
```

These values are milliseconds. A command's network timeout cannot extend Search's
deadlines; the POST example's `config.timeout` is in seconds. On remote setups,
credentials must be available to the hosting process.

| Symptom | Check / fix |
| --- | --- |
| Missing package or required field | Run `search engines list` and `search configure ID`; install or supply the named field |
| Missing runtime or credential | Install the declared runtime or set the named variable in the host environment; restart the host |
| SearXNG returns HTML or access denied | Check the full `/search` URL and instance JSON-output/access settings |
| Private destination refused | For an intended private HTTP engine, set its `allow_private_networks` to `true` |
| Timeout | Check upstream latency and both Search deadlines; also check the command's own network timeout |
| Invalid or oversized answer | Verify field mappings, titles, HTTP(S) URLs, JSON-only stdout, and the relevant response cap |
| Enabled engine absent from searches | Check `engines.enabled`, selection, query `-engines`, and restart existing sessions |
| Busy or edited package on update | Stop processes holding it; preserve source edits before explicit removal/reinstallation |

Develop custom mappings against saved responses before live testing. The
[engine README](../engines/README.md) documents private-store internals for
contributors.
