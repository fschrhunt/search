# CLI and hosting

`search::cli` composes the lower modules into the `search` executable and paired
HTTPS host. It requires the `cli` feature, enabled by default.

| File | Responsibility |
| --- | --- |
| `args.rs` | Command parsing, literal arguments, and help |
| `run.rs` | Dispatch, client selection, and local service construction |
| `render.rs` | Terminal/JSON output and visible engine failures |
| `engines.rs` | Engine listing, selection and configuration in settings, and explicit tests |
| `stdio.rs` | MCP stdio entry point |
| `http.rs` | Paired HTTPS listener, JSON routes, and authenticated MCP mount |
| `auth.rs` | TLS identity, pairing codes, device hashes, and private-file operations |
| `remote.rs` | Saved remotes, exclusive target selection, and local device administration |

Select a remote before opening local engines. `serve` always hosts the local
service, and engine/device management always runs locally. Bare `search` starts
MCP stdio; `search help` is the human-facing entry point.

Only pairing is public on the HTTP host. Protect execution and health routes
with device authentication, reject browser-origin requests, reload revocation
state, and keep credential files private. Preserve certificate pins and existing
on-disk trust filenames when changing module structure. Validate the trust
replacement boundary before accepting absent storage as local mode, and bind
the listener before renewing pairing state. Strip terminal controls only when
rendering human output; JSON retains source data.

From the repository root, run `./x test cli::`,
`./x test --test cli_engines`, and `./x test --test cli_remote`, then `./x check`.
See [usage](../../docs/usage.md), [engines](../../docs/engines.md), and
[remote hosting](../../docs/remote.md) for user-facing commands.
