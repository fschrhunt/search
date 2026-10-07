# Fetch

`Fetcher` retrieves public pages as `Page` results, preserving metadata and
reporting failures without panicking. It does not store a persistent corpus.

| File or directory | Responsibility |
| --- | --- |
| `mod.rs` | HTTP client, body/concurrency limits, redirect handling, and ordered batches |
| `guard.rs` | SSRF address classification, hostname checks, and connect-time DNS validation |
| `cache.rs` | Byte- and count-bounded, in-memory TTL cache |
| `extract/mod.rs` | Main-content extraction, cleaned fallback, metadata, and redirect detection |
| `extract/visibility.rs` | Hidden-content and boilerplate filtering |
| `extract/serialize.rs` | Whitespace normalization and link/image neutralization |

## Trust boundary

URLs and redirect targets are untrusted. Check schemes and destinations before
dialing; the guarded resolver rechecks DNS answers at connection time. Literal
redirect addresses also require validation. Environment proxies are disabled.
Never weaken private/metadata address classification to make a website work.

Client-side redirect targets are parsed from page content, not executed as script.
Every followed target returns through the guard, with bounded hops. Explicit
`allow_private_networks` is an operator override, not a property of page content.

Only successful pages enter the transient cache. Zero TTL or byte budget disables
storage; oversized pages bypass it. Full caches clear rather than growing without
bound. Passage selection is separate from retrieval in `core::text`.

Extraction runs off the async executor. Preserve literal text such as
`values[index]` while neutralizing Markdown link/image targets. Filtering is
defense in depth, not a guarantee against prompt injection: consumers must
treat extracted text as source data, not instructions.

Run `./x test core::fetch` and `./x guard` from the repository root, followed by
`./x check`. Preserve the guard's counterexample tests. See
[security](../../../docs/security.md) and [configuration](../../../docs/configuration.md)
for deployment and limit settings.
