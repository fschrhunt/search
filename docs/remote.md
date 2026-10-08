# Remote hosting

Local CLI commands and stdio MCP work without authentication. To share engine
configuration and fetch policy, optionally host Search on one machine and pair
each client. There is one device role: execution access to search and fetch.
Device administration runs locally on the hosting machine.

Install Search on both machines. The example assumes `search.example.net`
resolves to the host on a private network and clients can reach port 8642.
On the host:

```sh
search serve -address 0.0.0.0:8642 -hostname search.example.net
```

Use a hostname or IP clients can reach. `-hostname` sets the generated
certificate's name and is required for wildcard binds. The first start creates
a persistent identity in `SEARCH_HOME/trust/identity.json`; later starts retain it.
The host prints the public certificate path `SEARCH_HOME/trust/host.pem`, its SHA-256
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
```

The fingerprint argument is the exact lowercase SHA-256 hex value printed by
the host. Search verifies it against the supplied certificate before reading
or sending the code. The TLS handshake then checks the exact leaf pin, hostname,
validity and signatures using Rustls WebPKI. There is no unverified discovery
probe. Copying a public certificate once is the explicit trust bootstrap.
Codes expire after 15 minutes and can enroll one device. A code holds 256
random bits, so wrong guesses are not counted and cannot lock out pairing.
On the host, run `search pair-code` to issue another code while the server
keeps running; use the same `SEARCH_HOME` as the serving process.
The command prints only the new one-use code and invalidates the previous code.
The identity and existing device credentials persist.

Only the code hash, creation/expiry timestamps and consumed state are stored
in owner-only `SEARCH_HOME/trust/pairing.json`. Renewal and admission use the
same cross-process file lock, and consumption commits before device issuance,
so simultaneous requests and process crashes cannot reuse a code. If device
issuance fails after consumption, run `search pair-code` again. A clock
earlier than the code's creation time fails closed.

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
stay the same. Paired clients call the host's [JSON API](usage.md#paired-https)
(`/v1/search`, `/v1/fetch` and `/v1/status`). A process reads the selected
target when it starts, so restart an existing stdio process after switching
profiles. Remote network, trust, revocation and response errors are explicit;
they never execute locally as a fallback. `search serve` always runs its local engine even if a remote is selected.
Engine configuration, credentials, and fetch policy resolve on the host. CLI fetch passage formatting remains on the client.

Private files under `SEARCH_HOME/trust` use directory mode 0700 and file mode 0600,
and must belong to the user running Search.
Client `remotes.json` contains per-device secrets and certificate pins;
host `devices.json` contains only device hashes and `pairing.json` contains the
current one-use code hash, expiry and consumed state. Settings can be shared,
these files cannot. Do not point multiple machines at the same trust storage. Storage
currently requires Unix permission semantics; other platforms fail closed.

All API and MCP endpoints, including `/healthz`, require a device bearer secret;
`/pair` is the only public route. Credential-bearing clients reject redirects and proxies,
allow only HTTPS origins, and use connection and overall deadlines (10 and 120
seconds by default). Remote answers are capped at 64 MiB by default; configure
`remote.timeout` and `remote.max_response_bytes` on the client to change these
bounds. See [configuration](configuration.md). Clients check the shared input
bounds before sending, and the host checks them again: queries at most 512
bytes, limits at most 50, at most 10 URLs per fetch.
The service is for mutually trusted devices on a private network, with no
mDNS discovery, admin roles, OAuth or tenant isolation. Identity name changes
require deliberate identity replacement and re-pairing.
