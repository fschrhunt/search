# Configuration

`search::core::config` describes settings; it does not own running transports
or credential values.

| File | Responsibility |
| --- | --- |
| `settings.rs` | `Config`, adapter types, bounds, and duration helpers |
| `defaults.rs` | Defaults for omitted fields |
| `load.rs` | File loading, Search home resolution, environment overrides, and validation |
| `engines.rs` | Built-in presets, engine selection checks, and resolving an ID to a validated adapter |
| `mod.rs` | Public configuration API |

Loading uses an explicit path, then `CONFIG`, then
`SEARCH_HOME/settings.json` (normally `~/.search/settings.json`; Windows uses `%USERPROFILE%/.search/settings.json`). A missing implicit
default uses built-in settings; a missing explicit file is an error. Environment
overrides apply last. Unknown fields and unusable bounds fail visibly; explicit
zero values are not silently replaced with defaults.

Settings are trusted operator configuration: `engines.config` can define
executables. `engines.use` selects IDs. A preset ID (`mwmbl`, `searxng`) merges
its entry shallowly over the compiled-in adapter and cannot change `type`; any
other ID needs a complete `http` or `command` adapter. Credential settings
contain environment variable names, never secrets. `EngineSettings::adapter`
resolves one ID without reading credentials; the pool owns the resulting
execution state.

Run `./x test core::config` and `./x check` from the repository root.
The [configuration guide](../../../docs/configuration.md) defines the user-facing
fields and migration rules; the [engine guide](../../../docs/engines.md) describes
adapter setup.
