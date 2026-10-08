# Engines

`search::core::engines` runs the engines selected in settings and merges their
answers. It holds no engine-specific code: Mwmbl and SearXNG are adapter presets
in `core::config::engines`, resolved exactly like custom engines.

| File | Responsibility |
| --- | --- |
| `mod.rs` | `Engine`, `EngineState`, `EngineStatus`, `EngineError`, `Found` |
| `adapter.rs` | HTTP and command transports and result-row mapping |
| `pool.rs` | Concurrent execution, deadlines, reciprocal-rank fusion and URL dedup |
| `scratch.rs` | Private command scratch directories |

`Pool::new` resolves each ID in `engines.use` through
`EngineSettings::adapter`, which merges a preset with its `engines.config` entry
(or takes a custom entry as is), validates it, and gives command engines an
absolute `cwd`. Construction reads no credentials and runs no engine code; HTTP
engines build one guarded client each and reuse it. Reciprocal-rank fusion
counts each normalized URL once per engine, using its best rank; duplicate rows
cannot substitute for agreement between engines.

Adapters map an upstream JSON answer into `Found`. An invalid row (empty title,
missing or non-HTTP(S) URL, URL credentials, non-text snippet) is skipped and
counted; the count surfaces as `EngineState.skipped`. A missing results array,
or a non-empty one with no valid row, is a malformed answer. Error messages are
fixed strings that never carry upstream output, URLs, or credential values.

HTTP adapters dial only the configured endpoint, check its host with the SSRF
guard and guard DNS at connect time, refuse redirects and proxies, and cap the
body. Command adapters spawn without a shell, clear the environment except a
minimal runtime set and declared `env` names, write one request to stdin, cap
stdout, discard stderr, and kill the direct child when the query is dropped.
They are trusted host code, not a sandbox: settings that define them are trusted
operator configuration.

`scratch::prepare` creates the command `temp_dir` exported as `TMPDIR`, `TMP` and
`TEMP`. On Unix it walks ancestors with no-follow `openat`, requires root- or
caller-owned ancestors that others cannot write (allowing a root-owned sticky
`/tmp` with private descendants), and creates or accepts only a caller-owned
0700 directory. On Windows it pins ancestors without following reparse points
and creates the directory with a protected current-user DACL (`src/private_fs.rs`).
Scratch files are not removed automatically.

The pool runs every selected engine under `search.engine_timeout` and the whole
query under `search.timeout`. A failed, timed-out, or panicking engine is
reported by name in its `EngineState` beside the other engines' results.

For configuration, adapter fields, credentials, and troubleshooting, see the
[engine guide](../../../docs/engines.md). The
[JSON POST starter](../../../docs/examples/json-post) exercises the command
protocol.

## Validation

Run `./x test core::engines` and `./x test core::config`. CLI integration tests
live under `tests/cli/`; preset mapping fixtures under `tests/engines/fixtures/`.
Repository-wide validation remains `./x check`.
