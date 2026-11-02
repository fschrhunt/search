# Retrieval evaluations

Run `./x check` for behavioral, security, feature-build, and evaluation-scoring
contracts. The offline retrieval tests exercise real text through controlled
engines and guarded readers; they measure useful evidence rather than only JSON
shapes. They do not measure the quality of the live Mwmbl index.

The evaluation runner accepts a JSON array of cases. Each case has a unique
`id`, `queries`, and `relevant` labels containing `url` and verbatim useful
`spans`. Labels should be independently reviewed, cover different domains and
task types, and not be selected because an engine already returned them.

```sh
# Calibration fixtures: validates scoring, NOT live retrieval quality.
./x eval tests/fixtures/evaluation/labels.json \
  --packets tests/fixtures/evaluation/packets.json

# Live observations against your configured engines; save for reproducibility.
./x build
./x eval tests/fixtures/evaluation/labels.json \
  --binary ./target/debug/search --config /absolute/path/settings.json \
  --save-packets /absolute/path/observed-packets.json

# Re-score exactly the captured evidence.
./x eval tests/fixtures/evaluation/labels.json \
  --packets /absolute/path/observed-packets.json -k 5
```

Reported metrics are useful-source recall@k, labeled context efficiency,
returned/useful characters, host diversity, fetch/engine failures, and packet
latency. Recall requires nonempty, successfully read evidence: a link to a
failed source does not count or consume a top-k evidence slot. Redirect
destinations are matched as well as requested URLs. Context efficiency measures only the union of
verbatim labeled spans, avoiding overlap and repeated-label inflation. It is
a conservative label-dependent proxy, not an assessment of answer correctness.

URL matching removes fragments and recognized tracking parameters, but preserves
opaque query bytes and trailing path slashes: `/article` and `/article/` can serve
different content. One redirect destination earns at most one source label,
preferring its final URL, and determines host diversity. Blank passages are not
successful evidence. Labeled spans are matched within individual passages, not
across synthetic separators between potentially distant source ranges.

Compare saved packets from different configurations or systems against the
same labels and k. Include failure cases, cold/warm caches, engine versions,
settings, collection dates, and costs. Live pages and indexes change; only
packet replay is reproducible. Never publish captured private content, queries,
credentials, or settings without reviewing them.

`tests/fixtures/evaluation/agent-tasks.json` holds 22 questions coding agents
actually ask — library and tool docs, config keys, error messages, standards —
each labeled with the authoritative page that answers it and a verbatim span
from that page. Labels name the page that should answer, not what an engine
returned, and every span was checked against the live page; one author labeled
them, so review them before relying on the set.
Score a configuration against it the same way:

```sh
./x eval tests/fixtures/evaluation/agent-tasks.json \
  --binary ./target/debug/search --config /absolute/path/settings.json \
  --save-packets /absolute/path/agent-packets.json -k 5
```

On 2026-11-02, the keyless Mwmbl default found 2 of 22 (recall@5 0.09); a
self-hosted SearXNG with its general engines found 15 of 22 (0.68). Pages move:
re-check a label's URL and span before treating a miss as a retrieval failure.

The included two-case dataset is a harness starter, not a representative
benchmark. No claim of superiority or parity with Exa, Parallel, or another
managed engine follows from it. Such a claim needs a larger independently
labeled task set and comparable runs on those services.
