# Changelog

## Unreleased

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
