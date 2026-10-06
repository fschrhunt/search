# Install

search runs on macOS and Linux, arm64 and x86_64. It is one static binary with
no runtime dependency.

## The installer

```sh
curl -fsSL https://raw.githubusercontent.com/fschrhunt/search/main/install.sh | sh
```

It downloads the archive for this computer from the latest GitHub release,
checks it against the release's `checksums.txt`, and puts `search` in
`~/.local/bin` (or `$SEARCH_INSTALL_DIR`). Run it again to update. Pin a version
with `--version vX.Y.Z`, or a folder with `--dir DIR`.

## Homebrew

```sh
brew tap fschrhunt/search https://github.com/fschrhunt/search
brew install search
```

## From source

```sh
cargo build --release
```

The binary is `target/release/search`. The pinned toolchain in
`rust-toolchain.toml` installs itself on the first build.

Local CLI and stdio MCP work immediately without credentials. To host one
machine for paired devices, run `search serve` and follow [remote hosting](remote.md).
