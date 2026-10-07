# Engines

`adapter.rs` implements transports; `pool.rs` runs and ranks engines;
`manifest.rs`, `catalog_build.rs`, and `store.rs` own package validation,
embedding, installation, and leases. `mwmbl/` and `searxng/` contain only shipped
package manifests, using the same adapters as custom engines.

Executable adapters set `TMPDIR`, `TMP`, and `TEMP` to the private OS-temp
`search/` directory, or an absolute `temp_dir` configured for that adapter.
Directory creation uses the store's ownership and symlink checks. Scratch files
are not automatically removed; commands remain trusted, unsandboxed host code.

`search::engines` unifies runtime engines, adapters and the pool with engine
manifests, the offline maintained catalog, and the local installation store.
This module contains Rust implementation and shipped declarative assets. For
installation, configuration, adapter fields, credentials, and troubleshooting,
see the [engine guide](../../docs/engines.md).
The [JSON POST starter](../../docs/examples/json-post) exercises the same public
local-package contract; it is not embedded in the catalog.

`catalog` reads embedded metadata. `resolve` uses an installed package or the
embedded `mwmbl` default; resolving that default in a new home writes nothing.
Every other engine requires explicit installation. Inspection and installation
never run package code, download assets, or read credentials.

The root `build.rs` embeds release identity and discovers the catalog generically:
direct child directories of `src/engines/` with `engine.json`, plus their declared
files, are embedded at build time.
Only Mwmbl and SearXNG are maintained here. Packages have no install hooks or
per-engine Rust registrations. `mwmbl` maps a root JSON array using query parameter
`s`, row pointers `/title` and `/extract`, and fragment pointer `/value`.
`searxng` requires the full search URL and an instance allowing JSON output.

Catalog and local packages share the same `home/engines/ID` store and execution
contract, with no special handling for custom packages. Manifest schema 1 and
command protocol 1 remain stable across Search releases; incompatible changes
require a new contract version with continued version-1 support. Binary upgrades
must preserve installed packages, settings, and credentials. See the
[compatibility promise](../../docs/engines.md#compatibility-across-search-updates)
for field and execution guarantees and required security migrations.

## Private store contract

The caller supplies an absolute home whose parent already exists. Installation
creates a private home and `engines/` beneath it, or requires existing private
current-user-owned directories. Mutations reject symlink/parent traversal and
foreign-owned or externally writable ancestors. Root-owned sticky `/tmp`
(also its canonical `/private/tmp` location on macOS) is
allowed with private current-user-owned descendants. Root ownership is read
from the filesystem root, including when host root is unmapped in a UID namespace.
Unix stores use mode 0700 directories and 0600 files. Only assets explicitly
listed in `executables` receive mode 0700; every such asset must be a declared
file. Executable permissions are validated when reading installed packages.
Opened control-file handles are checked for ownership, permissions, link count
and identity using no-follow directory-descriptor walks.

Windows creates directories, control files and assets with an explicit protected
DACL granting the current token user alone full access. Handle checks reject
foreign owners, public grants, unprotected private DACLs and hard links. Ancestors
may be administered by SYSTEM, Administrators or TrustedInstaller, but other
principals may not replace their children or rewrite their security. Ancestor
handles are pinned against rename and write during no-follow opens; junctions
and every other reparse tag are rejected. Windows paths must be local disk paths;
UNC/device namespaces, alternate data streams and reserved filename aliases
fail closed. Explicit local relative paths are allowed; there is no
implicit project/cwd discovery. Declare every package file (`engine.json` is
included automatically); undeclared files/dirs and symlinks are rejected.
Build-time embedding and runtime installation share one manifest schema and
validator. Build-time reads reject package/intermediate/file symlinks and enforce
regular-file and byte limits. Staged copies are validated before publication.

Bounds are 64 package files, 2 MiB per file/manifest, 8 MiB per package, 256
installed engines and bounded directory enumeration. Copies are staged under
private random names, synced and published under the store's process lock.
Receipts are plain bounded metadata with content digests that detect accidental
package edits. They do not authenticate the store owner or protect against
malicious code running as that owner. There are no authentication keys or MACs.
Updates validate the previous receipt and digest, then use the catalog shipped
with the running binary. Local installs cannot be updated from the catalog.

`resolve` acquires a per-package shared lease under the store lock and returns
it in `Installed.lease: Option<Arc<File>>`. Keep this lease for the entire running
host lifetime; clones share the same handle, and the last drop releases it.
The embedded HTTP default has no lease. Installation results and `list` are
metadata without enduring leases: call `resolve` before running an engine.
Update/remove try an exclusive lease immediately and refuse busy packages with
an instruction to stop running Search hosts and release handles. An installed
package's files therefore remain in place while compliant hosts retain leases.

List/resolve are strictly read-only: they neither initialize missing control
files nor recover interrupted operations. An absent disk Mwmbl falls back to
embedded metadata even if unrelated store metadata/control files are damaged.
A present but damaged disk Mwmbl fails closed. Only mutation commands recover
unpublished stages, interrupted replacements and deletion tombstones. Linux
updates exchange directories atomically; macOS and Windows use recoverable
renames while excluding API readers with the store lock. Windows closes an
exclusive lease after publication/tombstoning and before deleting the old tree,
while the store lock still excludes readers. File contents are synced on both
platforms; Win32 cannot flush directory handles, so Windows directory renames
do not promise Unix-style power-loss durability.

Remove validates and exclusively leases the managed live package before renaming
it to a private deletion tombstone, syncing the parent and deleting its contents.
If interrupted, the next mutation finishes deleting the tombstone, even when
its receipt or lease file has already disappeared. Inspection leaves it intact.
Explicit removal may discard edited file contents while validating manifest
identity and declared paths. Malformed live receipts, symlinks and unsafe
permissions fail closed and require operator repair before removal.
Removing a disk-installed default reveals its embedded fallback; the caller must
disable selection separately. The API does not edit settings or create a database.
The embedded default ID is reserved against local replacements: custom/forked
engines must use another ID, preventing installation from shadowing an implicitly
selected default with unapproved executable code. No development links are supported.

## Validation

Run `cargo test --locked --all-features` and
`cargo clippy --locked --all-targets --all-features -- -D warnings`.
Package integration fixtures live under `tests/engines/`; CLI integration fixtures
live under `tests/cli/`. Tests cover the custom example's transport/protocol
offline, private store behavior, and cross-process collisions. Repository-wide validation remains
`./x check`.
