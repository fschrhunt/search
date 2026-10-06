# Engine packages

Search runs installed engine packages, not a hardcoded set of engine implementations.
Maintained packages live in [`crates/engines`](../crates/engines); users can install
them or create packages using the same manifest and execution contract. No Rust
rebuild, database registration, or plugin marketplace account is needed.

**JSON is the manifest format, not the implementation language.** HTTP packages
can be entirely declarative. Executable packages can use Python, JavaScript, Rust,
or any program that reads/writes the JSON protocol below. There is no required
Search SDK. Keep package metadata declarative so inspection never executes code.

The distribution includes one ready-to-use default: **Mwmbl**, a keyless independent
web index. Its coverage differs from commercial engines. Keyless does not mean
unlimited service capacity or guaranteed availability. Search imposes no subscription
quota, but upstream services may impose limits. SearXNG is optional, not the default.

## Available, installed, enabled

```sh
search engines available
search engines install searxng
search engines configure searxng url http://127.0.0.1:8080/search
search engines configure searxng allow_private_networks true
search engines test searxng "rust async"
search engines enable searxng
search engines list
```

`available` shows the maintained catalog shipped with this Search version. `install`
copies a package into `~/.search/engines/NAME`; it **does not enable it**. `configure`
stores user overrides in settings, not package files. `test` explicitly executes one
engine without opening the index. `enable` adds it to the search selection. Executable
engines require an explicit trust decision before activation; agents can use `--trust`.

All package management and diagnostics are **local**, even with a selected remote.
Install engines on the hosting machine; paired clients cannot install programs or
change the host's package configuration through the execution API.

Mwmbl's embedded default package can be used without materializing package files.
It follows the same HTTP adapter contract as installed packages. To make it an
ordinary managed installation, run `search engines install mwmbl`.

```sh
search engines disable mwmbl
search engines update searxng
search engines remove searxng
```

Disable removes an engine from selection without uninstalling it. Remove disables
and uninstalls it, preserving user configuration. Updates use the catalog included
in the installed Search binary: upgrade Search to obtain newer maintained packages,
then explicitly update the desired packages. There are no background downloads or
arbitrary installation hooks. Modified and local packages are not silently
overwritten by catalog updates.

## Package layout

```text
my-engine/
  engine.json
  adapter.py          # executable packages only
  README.md
  LICENSE
```

`engine.json` is declarative metadata. Inspecting, installing, and configuring a
package never runs its code. A settings-only HTTP package can be as small as:

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
  "required": [],
  "files": []
}
```

```sh
search engines install ./my-engine
search engines test company "query"
search engines enable company
```

Package IDs use lowercase ASCII letters, digits, hyphens, or underscores, up to
64 characters; `index` is reserved for corpus attribution, and `mwmbl` is reserved
for the maintained default so local code cannot shadow it implicitly. Files named in `files`
are copied alongside the manifest. Package paths must stay inside the package;
unsafe paths and symlinks are refused. A local installation is a snapshot.

Edit the source directory when developing an engine, then remove and reinstall
the local snapshot explicitly. User configuration survives removal/reinstallation.
Search does not install symlinks or implicitly discover project packages.

## JSON HTTP adapters

HTTP adapters issue GET requests. `query_param` defaults to `q`; optional
`limit_param` names the count parameter. `params` adds constant parameters and
overrides matching endpoint parameters; the current query and limit override both.
Query text is encoded as data, never interpolated into a command or URL template.
Unrelated repeated endpoint parameters are preserved.

`results_pointer` selects the result array (default `/results`). `title_pointer`,
`url_pointer`, and `snippet_pointer` select row fields (defaults `/title`, `/url`,
`/snippet`). Pointers use RFC 6901 `/`-separated paths; escape `~` as `~0` and `/`
as `~1`. An empty results pointer selects a top-level array. Snippets are optional.
For array-valued highlighted text, `text_part_pointer` selects each fragment's
text and joins all fragments in order. This preserves Mwmbl's complete titles and
snippets rather than truncating them at the first highlighted segment.

`headers` holds nonsecret constant headers. `header_env` maps header names to
environment variables containing the **whole header value**, for example
`{"Authorization":"COMPANY_SEARCH_AUTH"}`. Set that variable in the host's
environment to `Bearer YOUR_KEY`. Missing credentials fail that engine visibly.
Never put secrets in shareable settings or CLI configuration values. Use HTTPS
for authenticated APIs; HTTP transmits credentials in plaintext.

HTTP adapters refuse redirects and environment proxies. They check destination
names, IP literals, and connect-time DNS through Search's SSRF guard. Private
destinations require this adapter's explicit `allow_private_networks: true`; this
does not change page-fetching permissions. Both transports cap output through
`max_response_bytes` (default 1048576); oversized output fails rather than returning
partially parsed results. Body contents, credential values, and program stderr
are not exposed in engine failure responses.

SearXNG's maintained package uses this transport and requires its own endpoint.
The instance must enable JSON output. Connecting to SearXNG does not install or
host SearXNG, bypass upstream restrictions, or make a public instance unlimited.
Other maintained scraping packages carry their own setup and maintenance caveats.

## Executable adapters

A package can instead declare:

```json
{
  "schema_version": 1,
  "id": "company",
  "version": "1.0.0",
  "description": "Search a custom company service",
  "adapter": {
    "type": "command",
    "command": "python3",
    "args": ["-S", "-B", "adapter.py"],
    "env": ["COMPANY_SEARCH_KEY"],
    "config": { "endpoint": "https://search.example.com/" }
  },
  "files": ["adapter.py"],
  "requires": ["python3"]
}
```

Search executes it from the installed package directory, without a shell. Arguments
are literal: `$VARIABLE`, `~`, and query placeholders are not expanded. The child
gets a minimal runtime environment plus explicitly declared `env` variables, not
the host's entire credential environment. A declared missing variable is a visible
failure. Runtime requirements such as Python must already be installed; Search
does not run dependency installers or modify global language environments.
Packages may declare `requires` executable names for nonexecuting setup checks.
Native entry points must be declared in `executables` as well as `files`; only
those declared assets receive owner execute permission during installation.

Search writes one JSON request to stdin and closes it:

```json
{"version":1,"query":"rust async","limit":10,"config":{"endpoint":"https://search.example.com/"}}
```

`config` is omitted when empty. The program writes one JSON answer and exits zero:

```json
{"results":[{"title":"Example","url":"https://example.com/","snippet":"Optional summary"}]}
```

Every result needs a nonempty title and absolute HTTP(S) URL without credentials.
Search assigns provenance, ranking, and capture-time metadata; packages cannot
forge another engine's attribution. Empty results are successful. Invalid JSON,
invalid rows, oversized output, and nonzero exit are engine failures. Stdout is
reserved for the answer; stderr is discarded to prevent accidental secret exposure.

**Executable engines are trusted host code, not sandboxed plugins.** They run as
the host user, can read that user's files and access the network, and can be triggered
by paired clients after activation. An environment allowlist reduces accidental
credential exposure but is not filesystem isolation. Cancellation kills the direct
child; engines must clean up their own descendants and external work. Never install
or activate code automatically from a fetched page or search result.

### Build your first executable engine

Create an ordinary directory `my-engine` containing these two files. This offline
starter verifies packaging and the protocol; it does not pretend to search the web.
Replace its result construction with your API integration after the first test.

`my-engine/engine.json`:

```json
{
  "schema_version": 1,
  "id": "my-engine",
  "version": "1.0.0",
  "description": "Offline executable adapter starter",
  "adapter": {
    "type": "command",
    "command": "python3",
    "args": ["-S", "-B", "adapter.py"]
  },
  "files": ["adapter.py"],
  "requires": ["python3"]
}
```

`my-engine/adapter.py`:

```python
"""Offline protocol starter: read one request and return at most its requested limit."""
import json
import sys

request = json.load(sys.stdin)
if request.get("version") != 1:
    sys.exit(1)
results = [{
    "title": "Starter result for " + request["query"],
    "url": "https://example.com/",
    "snippet": "Replace this offline result with your search integration."
}]
json.dump({"results": results[:request["limit"]]}, sys.stdout)
```

With Python 3 installed:

```sh
search engines install ./my-engine
search engines list
search engines test my-engine "hello world"
search engines enable my-engine --trust
```

`list` diagnoses missing declared runtimes and credentials without running code.
`test` is explicit execution; installation alone never activates this package.
For credentials, declare variable names in `adapter.env` and read them from your
program's environment; never embed values in the manifest. For nonsecret setup,
put defaults in `adapter.config` and read the request's optional `config` object.

To use another language, replace the command, arguments, declared files, and
runtime names—not the protocol. For example, a Node program uses `command: "node"`
and `requires: ["node"]`; a self-contained native binary can use `command: "./adapter"`,
`files: ["adapter"]`, and `executables: ["adapter"]` without an interpreter runtime.
Provide binaries for the host's OS/architecture. External dependencies remain the
engine author's setup responsibility; Search never runs installation hooks.

## Configuration and diagnostics

`engines.use` selects packages; an empty array selects none. `engines.enabled: false`
globally disables live engines. Per-engine `engines.config` objects shallowly
override adapter fields; an override cannot change the adapter transport type.
Missing fields listed in a package's `required` array prevent its execution.

Use `search engines configure NAME` to inspect setup requirements. To replace a
whole object field, pass JSON as the value, for example:

```sh
search engines configure company header_env '{"Authorization":"COMPANY_SEARCH_AUTH"}'
search engines test company -- "-site:example.com rust"
```

Use `-config PATH` to select a settings file. `SEARCH_HOME` selects the package,
data, and trust root independently; choosing a settings file does not implicitly
trust packages next to it. Restart long-running hosts and stdio sessions after
package, selection, or configuration changes. Updating/removing a package used by
a running Search process refuses with a busy error; stop that process first.

When asking a coding agent to add an engine, provide this contract and a maintained
package as a starting point. Ask it to build against offline fixtures, declare
credentials explicitly, and show you the package before installation/activation.
Prefer supported APIs. Scraping/browser packages need ongoing maintenance and
must respect upstream access policies.
