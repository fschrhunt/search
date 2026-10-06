# Contributing

search is a self-hosted web search service in Rust. This is how to change it.

## Getting set up

Rust 1.98 is pinned in `rust-toolchain.toml`; `rustup` will install it on first
build. Python 3 is needed only for the UI-free parts of the checks — nothing
here needs it.

```sh
./x hooks     # once per clone: gofmt-equivalent and clippy before each commit
./x check     # what a pull request must pass
```

`./x check` runs, in order:

- `./x fmt --check` — `cargo fmt --all --check`
- `./x lint` — `cargo clippy --all-targets -- -D warnings`
- `./x test` — the workspace tests
- `./x guard` — `scripts/guard.sh`, the security-surface audit

CI runs the same commands, so your machine and CI never disagree about green.

## What a change needs

- **One concern per pull request.** If the description says "also", split it.
- **A test for each behavior change.** The regression test should fail against
  the unfixed code, for the reason the fix names.
- **A `CHANGELOG.md` entry** under `Unreleased` for anything user-visible.
- **Updated docs and comments.** A stale comment is worse than none.

`scripts/guard.sh` pins promises search makes — the network hosts it may reach,
the panic-site policy, the SSRF guard, the auth layer. A change that legitimately
moves one of those boundaries updates the guard in the same diff, where review
can see it. Never work around it.

## Where things go

The file map and the conventions are in `AGENTS.md`. In short: `crates/search`
is the engine library and names no terminal, listener, or protocol. The `cli`
and `mcp` packages depend on it, not the reverse. `search::Search` is the
in-process API used by both adapters.
`crates/engines` owns maintained package directories and the local catalog/installer;
the core and CLI depend on its metadata API. Add engines there using the same
manifest/adapter contract as user packages, never a special core registration.

## Branches, commits and pull requests

- Before the first push, name the branch `<type>/<slug>`, for example
  `fix/rebinding-guard` or `feat/owned-index`. Never push `main` directly.
- Conventional commit titles, plain language: `fix(search): close the DNS
  rebinding gap`. The type is `feat`, `fix`, `perf`, `refactor`, `docs`, `test`,
  or `chore`; the scope is `search`, `engines`, `cli`, `mcp`, `infra`, or `docs`.
- The title becomes the squash commit on `main`; write it as the one line someone
  reads in `git log`.
- Use the pull request template. Explain the problem, what changed, and why it
  works. List the checks you ran and their results.

## Releasing

See [docs/contributing/releases.md](docs/contributing/releases.md).

## Conduct and security

Be straightforward and kind in issues and reviews. Report a vulnerability the way
`SECURITY.md` describes, not in a public issue.
