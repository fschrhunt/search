# Maintained engine packages

`search_engines` implements engine manifests, the offline maintained catalog, and
the local installation store. `catalog` reads embedded metadata. `resolve` uses an installed package
or the embedded `mwmbl` default; resolving that default in a new home writes
nothing. Every other engine must be installed explicitly. Installation never
runs a command, downloads assets, or reads credentials.

The catalog is discovered generically by `build.rs`: direct child directories
with `engine.json`, plus their declared files, are embedded at build time.
Packages have no install hooks. `mwmbl` uses the root JSON array with `s` as the
query parameter, `/title` and `/extract` as field pointers, and `/value`
for each highlighted text part.
Its keyless service has limits; it is not advertised as unlimited. `searxng`
requires configuration of the full search URL and an instance allowing JSON.

Brave, Marginalia, Wikipedia, Hacker News, Stack Exchange, arXiv and DuckDuckGo
are self-contained stdlib Python command packages. Python 3 must already be
installed. Their manifests declare `requires: ["python3"]`; this is executable
requirements metadata, not an installer or dependency solver. They run
`python3 -S -B` to suppress site initialization and bytecode writes while keeping
local package imports available. They accept the version-1 stdin JSON protocol and emit one results
object, with errors redacted and exit status nonzero. Query execution is an
explicit trusted-code action, never part of installation or inspection.
Transport enforces bounded request/body/output sizes, bounded gzip expansion,
HTTPS redirects to the same service host only, and configurable timeouts.
Environment proxies are ignored. Package `config` accepts `timeout` (seconds),
`max_response_bytes` and `user_agent`. Core command deadlines still apply.
Brave carries the former browser user agent as its overridable default.

HTML scrapers can fail when markup changes, bot challenges appear, or providers
throttle requests. DuckDuckGo has no supported search API; its optional package
parses the HTML endpoint without challenge bypasses and reports unrecognized
responses as failures. Stack Exchange quota/backoff and arXiv rate limits remain
service restrictions. Installation and fixture tests prove no live availability.

The canonical shared Python helpers live in `support/`; packages carry copies
so installations are independent of the repository. Keep those copies identical;
the offline parser tests check this invariant. The fixtures describe whether a
sample came from the former tests or is synthetic.

# Private store contract

The caller supplies an absolute home whose parent already exists. Installation
creates a private home and `engines/` beneath it, or requires existing private
current-user-owned directories. Mutations reject symlink/parent traversal and
foreign-owned or externally writable ancestors. Root-owned sticky `/tmp` is
allowed with private current-user-owned descendants. Root ownership is read
from the filesystem root, including when host root is unmapped in a UID namespace.
Unix stores use mode 0700 directories and 0600 files. Only assets explicitly
listed in `executables` receive mode 0700; every such asset must be a declared
file. Executable permissions are validated when reading installed packages.
Opened control-file handles are checked for ownership, permissions, link count
and identity using no-follow directory-descriptor walks.

Disk installation fails closed on non-Unix platforms; embedded metadata/default
operation is portable. Explicit local relative paths are allowed; there is no
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
updates exchange directories atomically; other Unix platforms use recoverable
renames while excluding API readers with the store lock.

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

Run `cargo test -p search_engines` and
`cargo clippy -p search_engines --all-targets -- -D warnings`.
The crate tests run Python parser/protocol tests offline, alongside store and
cross-process collision tests. Repository-wide validation remains `./x check`.
