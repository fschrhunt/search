# Releases

A release is a `v*` tag on main. Everything after the tag is automatic.

## Cutting one

From an up-to-date main checkout:

```sh
scripts/release.sh v0.2.0    # opens a PR naming CHANGELOG.md's Unreleased section v0.2.0
scripts/release.sh v0.2.0    # after merging it: checks CI passed on main, then tags
```

## What the tag runs

`.github/workflows/release.yml`, in order:

1. **check**: the tag is on main and `CHANGELOG.md` has its section; `./x check`.
2. **release**: archives for macOS and Linux (arm64 and amd64) with checksums and
   build provenance; the GitHub release, with the changelog section as its notes.
3. **formula**: regenerates `search.rb` (`scripts/formula.sh`) for the release and
   pushes it to main.
4. **install**: a real `install.sh` install of the release on both systems.

## Who can write to main

The `formula` job has the `RELEASE_DEPLOY_KEY` write credential and runs in the
`release` environment, which only `v*` tags can use. To restrict direct updates to
main, configure branch protection and explicitly allow this deploy key. Without
that policy, repository write access governs who can push.

Setting it up, or replacing the key, takes an admin of the repository:

1. Create the `release` environment, limited to tags matching `v*`.
2. Make a key with `ssh-keygen -t ed25519 -N "" -f key` in a temporary folder.
3. Add `key.pub` as a deploy key with write access, and `key` as the environment's
   `RELEASE_DEPLOY_KEY` secret; then delete both files.
4. Configure main's protection policy to allow this deploy key to update the
   formula.

## Where people get it

- `curl -fsSL .../install.sh | sh`: `install.sh` from main, which downloads the
  latest release and checks its checksum.
- `brew install`: from `search.rb` at the top of this repository, which Homebrew
  reads as a tap.
