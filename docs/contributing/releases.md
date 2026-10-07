# Releases

A release is a `v*` tag on main. The tag starts the release workflow.

## Cutting one

With Git and the authenticated GitHub CLI (`gh`) installed, and permission to
push a release branch, open its PR, and push tags, use a clean, up-to-date main
checkout. Choose an unused version with nonempty `Unreleased` notes:

```sh
scripts/release.sh v0.2.0    # opens a PR naming CHANGELOG.md's Unreleased section v0.2.0
scripts/release.sh v0.2.0    # after merging it: checks CI passed on main, then tags
```

## What the tag runs

`.github/workflows/release.yml` runs:

1. **check**: the tag is on main and `CHANGELOG.md` has its section.
2. **build**: `./x check` and release compilation on six native runners: Linux,
   macOS, and Windows, each on arm64 and amd64. Unix binaries use `.tar.gz`;
   Windows binaries use `.zip`. No Apple SDK or Windows linker is expected on
   a Linux host.
3. **release**: collect all six archives, generate checksums, attest build
   provenance, and publish the GitHub release with the changelog section as notes.
   Builds target the root package with the default `cli` feature. Root `build.rs`
   embeds the catalog and release identity; binary upgrades do not replace
   installed engines, settings, or private trust files.
4. **formula**: regenerates `search.rb` (`scripts/formula.sh`) for the release and
   pushes it to main.
5. **install**: a real installer run on all six native runners: `install.sh` for
   Linux/macOS, `install.ps1` for Windows, followed by version and package-store
   smoke checks.

Both `formula` and `install` depend on `release` and may run in parallel.

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

- The [official installer](../install.md#the-installer): review `install.sh` from
  main (or `install.ps1` on Windows) before running it; it downloads the release
  and checks its checksum.
- [Homebrew](../install.md#homebrew): from `search.rb` at the top of this
  repository, which Homebrew reads as a tap.
