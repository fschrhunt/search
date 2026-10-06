# Remote hosting

Local CLI commands and stdio MCP work without authentication. To share a corpus
and provider configuration, host Search on one machine and pair each client.
There is one device role: execution access to search, fetch, index and refresh.
Device administration runs locally on the hosting machine.

On the host:

```sh
search serve -address 0.0.0.0:8642 -hostname search.example.net
```

Use a hostname or IP clients can reach. `-hostname` sets the generated
certificate's name and is required for wildcard binds. The first start creates
a persistent identity in `dir/trust/identity.json`; later starts retain it.
The host prints the public certificate path `dir/trust/host.pem`, its SHA-256
fingerprint, and a random one-use code. Copy only `host.pem` to the client and
compare the fingerprint through a trusted channel. Never copy `identity.json`:
it includes the private key.

On the client:

```sh
search remote pair home https://search.example.net:8642 ./host.pem HOST_SHA256
# Enter the host's one-use code at the stdin prompt.
search remote use home
search "rust async"
search fetch https://www.rust-lang.org
search index "async"
search refresh
```

The fingerprint argument is the exact lowercase SHA-256 hex value printed by
the host. Search verifies it against the supplied certificate before reading
or sending the code. The TLS handshake then checks the exact leaf pin, hostname,
validity and signatures using Rustls WebPKI. There is no unverified discovery
probe. Copying a public certificate once is the explicit trust bootstrap.
Codes expire after 15 minutes, allow at most 20 attempts, and can enroll one
device. On the host, run `search pair-code` to issue another code while the
server keeps running; use the same `-config PATH` or `DIR` as the serving process.
The command prints only the new one-use code and invalidates the previous code.
The identity and existing device credentials persist.

Only the code hash, creation/expiry timestamps, attempt count and consumed state
are stored in owner-only `dir/trust/pairing.json`. Renewal and admission use the
same cross-process file lock. Failed attempts persist, and consumption commits
before device issuance, so simultaneous requests and process crashes cannot
reuse a code. If device issuance fails after consumption, run `search pair-code`
again. A clock earlier than the code's creation time fails closed.

`search remote list` shows saved names, URLs and device IDs with `*` marking
the selected target. Pairing saves a profile but does not select it.
`search remote off` selects local execution; `search remote remove NAME` deletes
a client profile and selects local execution if that profile was active. Removing
a profile does not revoke it on the host. Replacing a profile also leaves the old
host device enrolled; revoke its ID separately.

`search devices` lists host device IDs, names and enrollment timestamps.
`search revoke DEVICE_ID` removes a device's hash. Every new API or MCP request
reloads that registry, so the next request is refused, including requests on
existing MCP sessions. Already admitted work may finish. There are at most 64
enrolled devices. Management commands accept `-config PATH` for local settings.

Agents still spawn `search` over stdio; the two tools and their output shapes
stay the same. A process reads the selected target when it starts, so restart
an existing stdio process after switching profiles. Remote network, trust,
revocation and response errors are explicit; they never open the local corpus
as a fallback. `search serve` always hosts local data even if a remote is selected.
Configuration, credentials, fetch policy, index writes and refresh hosts resolve
on the host. CLI fetch passage formatting remains on the client.

Private files under `dir/trust` use directory mode 0700 and file mode 0600,
and must belong to the user running Search.
Client `remotes.json` contains per-device secrets and certificate pins;
host `devices.json` contains only device hashes and `pairing.json` contains the
current one-use code hash and admission bounds. Settings can be shared, these files
cannot. Do not point multiple machines at the same trust storage. Storage
currently requires Unix permission semantics; other platforms fail closed.

HTTPS execution and MCP endpoints require a device bearer secret; `/pair` is the
only public route. Credential-bearing clients reject redirects and proxies,
allow only HTTPS origins, and use connection and overall deadlines (10 and 120
seconds). Answers are capped at 8 MiB. Remote operation bounds match the HTTP/MCP
contract: queries at most 512 bytes, limits at most 50, at most 10 URLs per fetch.
The service is for mutually trusted devices on a private network, with no
mDNS discovery, admin roles, OAuth or tenant isolation. Identity name changes
require deliberate identity replacement and re-pairing.
