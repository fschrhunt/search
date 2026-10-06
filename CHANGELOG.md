# Changelog

## Unreleased

- Make `./x` argument handling consistent: show command help, forward formatting
  arguments, and reject unexpected arguments to shell and guard checks.
- Keep search orchestration opt-in and bounded: only Mwmbl runs by default,
  additional engines require explicit activation, the per-engine deadline is
  five seconds, and unavailable selections produce an explicit engine error.
- Preserve literal square brackets in fetched code and prose while continuing
  to neutralize Markdown link/image targets.
- Show failed and timed-out engines in terminal search output without exposing
  program stderr or credential values.
- Focused fetches with no matching passages return bounded page text rather than
  labelling the opening as a match; preserve truncation metadata for that fallback.
- **Breaking Rust API:** consolidate into one root `search` package and library,
  with the `search` CLI binary. Import `search::core::{Search, Config, Query,
  Answer, Page, Link}`, `search::engines::{Engine, Pool}` and package APIs,
  `search::client::Client`, and `search::mcp::Server` directly; old facade imports
  and separate crate names are removed. Default `cli` includes MCP and hosting;
  minimal and MCP-only library builds are supported. Modules replace compilation
  boundaries; conventions and boundary tests enforce dependency direction.
  Manifest schema 1, command protocol 1, and on-disk formats remain unchanged.
  Binary upgrades do not replace installed engines, settings, or credentials.

- Refresh onboarding, engine integration, hosting, and contributor documentation;
  prefer `search help` and `search version`, clarify setup prerequisites, and
  align CLI help and diagnostics with the primary commands.
- Add `./x help` and make installer help work when read from stdin as well as a
  saved script. Keep existing help/version flag aliases for compatibility.
- Define manifest schema 1 and command protocol 1 as stable engine contracts
  across Search releases. Document the shared store/runtime for shipped and
  custom engines and protect installed custom packages with a compatibility test.
- Bound paired-host responses with `remote.max_response_bytes` (default 64 MiB)
  and align CLI focused reads with the 6000-character MCP default. Explicit CLI
  character limits must be 1–40000; unmatched queries fall back to page text.
- Engine management commands reserve their first word; use `search -- QUERY`
  when a literal query begins with a command name.
- Bound ephemeral fetch caching with `fetch.cache_bytes` (default 32 MiB),
  accounting for string payloads and entry overhead. Zero disables storage;
  oversized pages skip caching, and full caches clear all entries.

- Re-check HTTP redirect destinations before dialing and ignore environment
  proxies in the page reader, so neither can bypass the SSRF guard.
- **Breaking:** remove the persistent local corpus, index/refresh commands, and
  corpus API. Reject `dir`, `search.local_weight`, `fetch.max_stored_chars`, and
  `index` settings; remove `DIR`/`-dir`. Existing files and data are untouched.
  Search now uses live web engines and clean fetch with a transient memory cache.
- **Breaking:** rename user selection/attribution fields from `providers` to
  `engines` and CLI `-providers` to `-engines`. Rename core `discovery` to
  `engines`, `Provider` to `Engine`, `Finding` to `Link`, `Response` to `Answer`,
  `Fetched` to `Page`, `Registry` to `Pool`, and `AdapterProvider` to `Adapter`.
  The service remains `Search`; package transport configuration remains `adapter`.
- Move shared local/remote execution into `search::client`;
  MCP owns only tools and transports. Retain optional paired HTTPS hosting and
  remote routing without local fallback.
- MCP fetch with no query or character limit returns the full clean page under
  the fetch transport body bound. Explicit `max_characters` accepts 1–40000;
  query-focused reads default to 6000 characters per page.
- Reduce the shipped catalog to Mwmbl and SearXNG. Remove vendor API and scraper
  packages and their helpers; existing installed packages/settings are untouched.
  Custom HTTP and command engines still merge concurrently. Document AI/API
  integrations with a tested, copyable authenticated JSON POST starter outside
  the catalog. `search install NAME|./PATH` is the primary installation command;
  `search engines install` remains an alias.
- Document language-independent executable engines, an offline starter, declared
  runtimes/credentials, and native-binary packaging.
- Replace special built-in engines with installable packages and local lifecycle
  commands; shipped assets live under `engines/`. `SEARCH_HOME` selects packages, settings, and
  separate private trust storage. Updates preserve user settings and package
  edits; executables inherit only declared credentials and a minimal environment.
  Release builds target the root package with the default `cli` feature.
- Add configurable JSON HTTP and executable engine transports, bounded output,
  guarded endpoints, ranking/deadlines, and local `search engines list/test`
  diagnostics. Reject missing explicit settings files and invalid zero bounds;
  zero cache/redirect settings disable those features. Require current-user
  ownership of trust storage and remove unused search-cache/log-level settings.

- Add `search pair-code` to renew one-use host pairing codes without restarting.
  Persist only code hashes and admission bounds under a process lock.

- Replace shared HTTP tokens with persistent paired HTTPS identities and revocable
  per-device credentials. Add `remote pair/use/list/off/remove`, `devices`, and
  `revoke`; selected remotes execute CLI and stdio MCP operations without fallback.
- Remove `token`/`token_env` with no compatibility aliases.

- Validate MCP tool inputs, run batch searches concurrently, and return structured
  results. Fetched pages report their fetch time. Runtime settings use
  `settings.json` and consistent snake_case names without compatibility aliases.
- Enforce the configured maximum result count and whole-search deadline, fail startup if the
  guarded HTTP client cannot be built, and make the fetch-cache expiry test deterministic.
- Organize the single `search` package around protocol-free `core`, runtime
  and package `engines`, shared `client`, and optional `mcp`/`cli` modules.
  The engine exposes `search::core::Search`; minimal library builds omit MCP.
- Added theme-aware black and white lockup/logo SVGs, and used the lockup at the
  top of the README.
- Reframed the README around Search's core promise and clarified getting
  started, adoption, configuration, and security.
- Extraction keeps the article and drops the cruft: ads, cookie banners,
  related rails, and hidden text are removed, and links and images are
  neutralized so page content cannot exfiltrate. A page that only redirects
  elsewhere is now followed to its target.
- `web_fetch` supports query-focused passages and optional character limits;
  remove the unused `objective` parameter.
- Fetched pages carry byline, published time, and site for citation.
- A link's `engines` attribution is a JSON array.

## Unreleased (before this change)

- The first Rust implementation: one binary serving multi-provider discovery, a
  hardened fetcher, a private full-text index, and MCP over stdio and HTTP.
- Release tooling: `scripts/release.sh` (name the changelog section, then tag),
  `scripts/formula.sh` (the Homebrew formula), and `install.sh` (the
  checksum-verified installer), with CI and a tag workflow that builds archives,
  attests provenance, updates the formula, and installs the release.
- Documentation and contribution: `CONTRIBUTING.md`, `docs/` for users and
  `docs/contributing/` for the project, issue and pull-request templates, and a
  `./x check` that includes a shell-syntax check.
