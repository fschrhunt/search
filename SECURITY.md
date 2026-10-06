# Security

## Reporting

Report a vulnerability through GitHub's private vulnerability reporting on
<https://github.com/fschrhunt/search/security/advisories/new>. Do not open a
public issue for a security problem.

Include what you did, what happened, and what you expected. A proof of concept
helps; a working exploit is not required.

## What is a vulnerability here

search runs on a private machine and is reached over a private network, but it
treats every URL and every page as hostile regardless. These are the properties
worth attacking:

- **The fetcher reaches only public addresses.** Loopback, RFC1918, link-local
  (including `169.254.169.254`), CGNAT, multicast, unique-local, and every IPv6
  form that embeds such an IPv4 must be refused, on the URL and on every
  redirect hop. A way to reach one is a vulnerability.
- **Every execution request is authenticated over HTTPS.** An API or MCP path
  that executes without a current paired-device credential is a vulnerability.
  The public pairing endpoint requires a bounded, expiring one-use code.
- **Untrusted HTML cannot crash the service or exhaust it.** A panic, an
  unbounded allocation, or an infinite loop driven by a fetched page is a
  vulnerability.
- **The full-text index cannot be driven by query text.** A query that reaches
  SQLite as anything other than a quoted-term MATCH expression is a
  vulnerability.

## What is not a vulnerability

- Hosting on a private network. search is meant to sit behind a
  tailnet; the listener itself uses paired HTTPS.
- A provider returning wrong or stale results. Discovery borrows other indexes;
  their accuracy is their own.
- The absence of a rate limit. search is a personal service behind a private
  network and paired-device trust.
- `fetch.allow_private_networks: true` letting the fetcher reach private addresses. That setting
  is documented as disabling the guard, for tests and air-gapped mirrors only.
