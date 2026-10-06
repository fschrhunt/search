# Security

search is built to run on a private machine and be reached over a private
network. It still treats every URL and every page as hostile.

## The fetcher

A URL is model-chosen, so the fetcher assumes it is hostile:

- **Private destinations are refused.** Loopback, RFC1918, link-local (including
  cloud metadata at `169.254.169.254`), CGNAT, multicast, and IPv6 unique-local
  are refused by literal, by name, and — through a resolver installed on the
  client — by every address a name resolves to. A DNS answer that changes between
  the check and the dial (rebinding) is caught, because the socket opens to the
  address that passed the guard.
- **Every IPv6 form that embeds such an IPv4 is refused**, including NAT64
  (`64:ff9b::/96`), 6to4 (`2002::/16`), and IPv4-compatible (`::/96`), and the
  bracketed forms `url` produces.
- **Redirects are re-checked per hop**, bounded by `max_redirects`. A
  client-side redirect (a meta refresh or a scripted location change) is
  followed only through the same guard, so page content cannot steer the fetcher
  inside the network.
- **Bodies are size-capped** and **requests carry a deadline**.

The one escape hatch, `allow_private_networks`, disables the guard for tests and
air-gapped mirrors. Never set it on a reachable service.

## Untrusted content

A fetched page is untrusted content, and the cheapest place to hide an
instruction aimed at a model is text a person never sees. Extraction removes
visually hidden text — inline `display:none`/`visibility:hidden`/`opacity:0`,
the `hidden` attribute, `aria-hidden`, screen-reader class names, and off-screen
positioning — and neutralizes links and images, so page content cannot form a
markdown image that exfiltrates. The remaining text is passed to the model as
data, in a clearly delimited field.

Search holds a private corpus and can fetch public pages. Paired devices are
trusted with that corpus and with initiating those fetches; extracted content
still must be treated as data by the consuming model.

## Authentication

Every execution request, including MCP, requires a revocable per-device secret
on HTTPS. The host stores only SHA-256 hashes and compares them with `subtle`.
The one-use pairing code expires after 15 minutes and locks after 20 attempts.
`search pair-code` renews it locally without restarting the host. Only the code
hash is stored, with expiry, consumed state and failed attempts, under the same
cross-process lock used for admission. Consumption commits before a device is
issued; renewal invalidates earlier codes. Clock rollback before code creation
fails closed.
Clients verify the public certificate fingerprint before transmitting the code.
TLS checks the exact leaf certificate pin and delegates chain, hostname, expiry,
and handshake signatures to Rustls WebPKI; no insecure certificate acceptance
is used. Credential-bearing clients refuse redirects, proxies and plaintext.
Browser Origin headers are refused. Trust files are owner-only and separate
from settings; a corrupt registry fails closed. See [remote](remote.md).

The MCP transport also validates the inbound `Host` header, to prevent DNS
rebinding against a locally running server, so a deployment names the authority
it is reached by.

## The index

Query text reaches SQLite only as a MATCH expression built from quoted terms, so
FTS operators in user input cannot inject syntax.

## What is not a vulnerability

See `SECURITY.md`: a provider's accuracy, execution requests without a global rate limit, and `allow_private_networks` reaching private
addresses are all by design.
