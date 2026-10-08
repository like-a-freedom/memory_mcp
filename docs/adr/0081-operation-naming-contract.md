# ADR-0081: Every log event names its operation

- Status: accepted — 2026-10-08
- Relates to: [ADR-0078](0078-human-readable-logging-on-tracing.md) (the `op`
  filter key), [ADR-0080](0080-one-request-identity.md)

`RUST_LOG=<prefix>=<level>` selects records by the `op` field, and the HTTP
runtime already pins its operation names in an inventory (`HTTP_OPERATIONS`) with
a test that the list matches what the code emits. The rest of the codebase had no
such contract, and an enumeration found the drift it invites. Conforming first
segments in use include `main`, `http`, `control`, `oidc`, `embedding`, `reembed`,
`knowledge`, `ner`, `graph`, `lifecycle`, `db`, `schema`, `config`, `cache`,
`triple_extraction`, `fs_watch`, the six canonical tools, and the two app tools.
Drifting from them: `startup` in three spellings (`startup`,
`startup.version_probe_failed`, `startup.versions`), `model_loader`,
`dimension_override_mismatch` — each names a module or a flag — one ad-hoc
subsystem name (`cosine_similarity`), and one verb phrase
(`extract_from_episode`). A filter key that cannot be enumerated cannot be
documented, tested, or trusted as a dial.

## Decision

**An operation name is `<namespace>` or `<namespace>.<event>`, where
`<namespace>` is a member of a pinned, closed registry and `<event>` is one or
more lowercase dot-separated segments.**

The registry is:

```
assemble_context  app_command  cache  config  control  db  embedding  explain
extract  fs_watch  graph  http  ingest  invalidate  knowledge  lifecycle  main
ner  oidc  open_app  reembed  resolve  schema  triple_extraction
```

`OP_NAMESPACES` is that constant in `logging.rs`. A bare name (`extract`) is the
namespace with an implicit default event and is legal: the tool and capability
vocabulary logs one line per call under its own name, and an event tail
(`extract.done`) is the same namespace. `RUST_LOG=extract=debug` selects both,
which is the intent.

Eight names are renamed to conform, across five old namespaces:

| Old | New |
|---|---|
| `startup` | `main.config` |
| `startup.version_probe_failed` | `main.version_probe_failed` |
| `startup.versions` | `main.versions` |
| `model_loader` | `embedding.model_loader` |
| `dimension_override_mismatch` | `embedding.dimension_override_mismatch` |
| `cosine_similarity.dimension_mismatch` | `embedding.cosine_similarity_mismatch` |
| `extract_from_episode.start` | `extract.from_episode.start` |
| `extract_from_episode.done` | `extract.from_episode.done` |

`config` and `schema` are new registry members, not renames: the
config-resolution events (`config.default_applied`,
`config.legacy_data_dir_detected` in `service/core/builder.rs`) and the migration
events (`schema.init`, `schema.init.compatibility_conflicts` in
`storage/migrations.rs`) already name a real subsystem.

Dynamic names built with `format!` — `db.{op}.retry`, `db.{op}.timeout` in
`platform/persistence/transactions.rs` — are covered by matching the static
prefix (`db`).

## Enforcement

A source-scanning **lint** walks `crates/memory-mcp/src` and asserts every
emitted operation's first segment is registered. It recognises the idioms this
codebase uses — a literal assigned to an `op` key in any wrapper it appears in
(`serde_json::json!`, `Value::String`, `.into()`, a map or JSON key, or the
`event!` macro), a `format!` template whose static prefix it reads, an
`op: "literal"` struct field, a native `op = "literal"`, and the first literal
argument of `log_event(`, `log_claim_event(`, `log_op(` — and is deliberately not
a general parser. It is labelled a lint, not scenario coverage (AGENTS.md: a
policy check is a lint). The existing HTTP inventory check is extended to read
the files that emit its names, so it remains the exhaustive check for `http`.

## Consequences

- A new event must pick an existing namespace; adding one is a deliberate edit to
  `OP_NAMESPACES`, reviewable as a change to the filter-key vocabulary and to the
  runbook that lists it.
- `RUST_LOG=<namespace>=debug` becomes a complete, documented dial for every
  subsystem instead of a partial one.
- The renames change eight filter keys across five old namespaces. A dashboard
  or saved query naming any of them must be updated. That is the only
  compatibility break; it is bounded, and each old spelling named a module, a
  flag, or a verb phrase rather than an operation.
- The lint recognises the current idioms. A future producer that builds an `op` a
  new way is not seen by it, which is why the registry and the runbook stay the
  contract and the lint is the backstop.
- The eight tool names stay flat (`ingest.done`, `open_app.start`) rather than
  nesting under a `tool.` prefix: they are the user-facing operation vocabulary
  and the documented `RUST_LOG` examples already use them.
