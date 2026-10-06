# Install

Search ships only Mwmbl and SearXNG, with Mwmbl ready to use by default.
Installing Search does not require Python or a SearXNG instance. Optional
executable packages declare their own runtime requirements. Use `search engines
available` after installation. Packages/settings/private trust live under
`~/.search` or the explicit `SEARCH_HOME`; see [engine packages](engines.md).

Search ships binaries for macOS and Linux, arm64 and x86_64. No language
interpreter is required for the binary or its shipped HTTP engines.

## The installer

Download and review the script before running it:

```sh
curl -fsSL https://raw.githubusercontent.com/fschrhunt/search/main/install.sh -o install-search.sh
less install-search.sh
sh install-search.sh
export PATH="$HOME/.local/bin:$PATH"
search version
search help
```

It downloads the archive for this computer from the latest GitHub release,
checks it against the release's `checksums.txt`, and puts `search` in
`~/.local/bin` (or `$SEARCH_INSTALL_DIR`). Run it again to update. Pin a version
with `sh install-search.sh --version vX.Y.Z`, or select a folder with
`sh install-search.sh --dir DIR`. `SEARCH_VERSION` also selects a version;
these are installer options, separate from Search's CLI commands. Use
`sh install-search.sh help` for its own reference. The installer needs curl or
wget, tar, and sha256sum or shasum. It refuses to replace a symlink
or an installation marked as package-manager-owned; update that installation
through its package manager.

The checksum verifies the archive against the release's checksum file; review
the script and trust the release source before running downloaded code.

## Homebrew

```sh
brew tap fschrhunt/search https://github.com/fschrhunt/search
brew install search
```

## From source

Install Git, rustup, and your platform's C linker/build tools first. Rustup
installs the toolchain pinned in `rust-toolchain.toml` on the first build.

```sh
git clone https://github.com/fschrhunt/search
cd search
cargo build --release
./target/release/search version
./target/release/search help
```

The binary is `target/release/search`. Add it to your `PATH`, or use its full
path in the usage examples. The root Cargo package builds both the `search`
library and binary; there is no workspace or separate CLI package. Default
features enable `cli`, including MCP and optional hosting/authentication
dependencies.

For library builds without CLI or protocol dependencies, or with MCP only:

```sh
cargo build --no-default-features
cargo build --no-default-features --features mcp
```

These build the library without the CLI binary. See the
[Rust example](../README.md#one-engine-more-surfaces) and
[module/feature map](contributing/architecture.md).

Local CLI and stdio MCP work immediately without credentials. To host one
machine for paired devices, run `search serve` and follow [remote hosting](remote.md).
