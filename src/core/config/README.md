# Configuration

`search::core::config` describes settings; it does not own running transports,
credential values, or package leases.

| File | Responsibility |
| --- | --- |
| `settings.rs` | `Config`, adapter types, bounds, and duration helpers |
| `defaults.rs` | Defaults for omitted fields |
| `load.rs` | File loading, Search home resolution, environment overrides, and validation |
| `engines.rs` | Adapter validation and resolving an explicitly selected package's settings |
| `mod.rs` | Public configuration API |

Loading uses an explicit path, then `CONFIG`, then
`SEARCH_HOME/settings.json` (normally `~/.search/settings.json`; Windows uses `%USERPROFILE%/.search/settings.json`). A missing implicit
default uses built-in settings; a missing explicit file is an error. Environment
overrides apply last. Unknown fields and unusable bounds fail visibly; explicit
zero values are not silently replaced with defaults.

Package selection is explicit. `engines.config` supplies shallow adapter overrides;
credential settings contain environment variable names, never secrets. Resolving
adapter settings does not change the transport type or package working directory.
The pool owns the resulting execution state.

Run `./x test core::config` and `./x check` from the repository root.
The [configuration guide](../../../docs/configuration.md) defines the user-facing
fields and migration rules; the [engine guide](../../../docs/engines.md) describes
adapter setup.
