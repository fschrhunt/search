# Changelog

## Unreleased

- Name user-facing search source settings `engines` and `search.engine_timeout`;
  retain `Provider` for the internal adapter contract, and remove the unused API
  key setting (the current engines are keyless).
- Add a true `index.enabled` off switch, explanatory JSON `notes`, and home-path
  expansion. Reject missing explicit config files and invalid zero bounds;
  zero cache/redirect settings disable those features. Bound local lookup by
  the query deadline and require current-user ownership of trust storage.
  Remove the unused search-cache and log-level settings rather than expose
  switches with no effect.
- Add `engines.enabled` for local-index-only use and configure the paired-host
  request deadline with `remote.timeout`.

- Add `search pair-code` to renew one-use host pairing codes without restarting.
  Persist only code hashes and admission bounds under a process lock.

- Replace shared HTTP tokens with persistent paired HTTPS identities and revocable
  per-device credentials. Add `remote pair/use/list/off/remove`, `devices`, and
  `revoke`; selected remotes execute CLI and stdio MCP operations without fallback.
- Rename corpus settings and overrides to `dir`, `DIR`, and `-dir`; remove
  `token`/`token_env` with no compatibility aliases.

- Bound MCP fetch output by its character limit, validate tool inputs instead
  of silently dropping them, run batch searches concurrently, and return
  structured MCP results. Search results and fetched pages now expose when a
  locally cached copy was fetched. Runtime settings use `settings.json` and
  consistent snake_case names, without compatibility aliases.
- Enforce the configured maximum result count and whole-search deadline, fail startup if the
  guarded HTTP client cannot be built, and make the fetch-cache expiry test deterministic.
- Reshaped the workspace around the `search` engine, `cli` executable, and
  optional `mcp` adapter. The engine exposes `search::Search` without an MCP
  dependency.
- Added theme-aware black and white lockup/logo SVGs, and used the lockup at the
  top of the README.
- Reframed the README around Search's core promise and clarified getting
  started, adoption, configuration, and security.
- Search results now blend matches from a bounded local corpus with live
  providers. Add one-shot `search`, `fetch`, `index`, and `refresh` CLI commands;
  index use, fetch indexing, corpus age/size limits, and seeded-host refresh are
  configurable.
- Extraction keeps the article and drops the cruft: ads, cookie banners,
  related rails, and hidden text are removed, and links and images are
  neutralized so page content cannot exfiltrate. A page that only redirects
  elsewhere is now followed to its target.
- `web_fetch` takes a `query` and returns the passages that match it instead of
  the whole page, and `max_characters` to bound the answer. The unused
  `objective` parameter is gone.
- Fetched pages carry byline, published time, and site, for citation.
- A finding's `providers` is a JSON array, not a joined string.

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
