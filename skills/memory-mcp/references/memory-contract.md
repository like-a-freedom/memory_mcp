# Memory Contract

Load this reference before a write, when choosing time boundaries, or
when recovering from a non-success result. Live MCP schemas remain authoritative
for accepted fields and enums.

## Source

- `source_id` is stable and round-trippable. Same ID and content is idempotent;
  same ID with different content is a conflict.
- `content` is bounded, source-linked, verified, and secret-free. URL/path text
  is literal; ingestion does not fetch URLs or open files.
- `source_type` uses a value accepted by the live schema.
- `t_ref` is the source's valid time in ISO 8601. The server owns
  transaction time; omit `t_ingested`, even if the source has another date.
- Changed evidence gets a new source identity. Superseded facts are invalidated,
  never deleted.

## Storage boundary

One Active Namespace is bound at server startup (environment configuration, not
per-request input). No tool accepts a `scope`, `project`, or `namespace`
argument, and none should be invented or probed. `policy_tags` on ingest are
the content-governance mechanism: tag sources so recall-time policy filtering
has evidence to work with. Empty recall is not authority to widen a boundary.

Credentials never enter memory. Sensitive business data is eligible only when
the server's configured policy explicitly permits it.

## Time

Use `valid_at` for world-valid time and `known_at` for knowledge committed by
a timestamp, or `known_revision` for an exact server checkpoint. `known_at` and
`known_revision` are mutually exclusive; legacy `as_of` sets both axes and
cannot accompany any new axis. Omitted axes capture a new request snapshot.
For a same-snapshot refinement copy `context.snapshot.valid_at` and
`context.snapshot.known_revision`; omit `as_of` and `known_at`. Revision tokens
belong to this namespace/history, not arbitrary stores.

Historical reads support `view_mode=current` only and may be partial: inspect
completeness, omitted channels, candidate-scan truncation and guidance. An empty
partial result is not global absence; one permitted refinement cannot make it
complete. Current assembly is a logical read that can persist access heat.

## Result states

| State | Meaning |
|---|---|
| `verified` | episode stored and returned facts inspected |
| `episode-only` | episode stored and completed successful extraction returned no durable facts |
| `pending` | extraction is partial/pending, or the call failed/timed out; known handles preserved |
| `assembled` | returned context inspected within the stated boundary |
| `cited` | every used claim has source provenance |

Never collapse `episode-only`, `pending`, and `verified` into “captured.”
After completed successful extraction, no facts confirms no canonical fact capture, not loss of the episode:
source fallback can still return stored source text. Empty recall is bounded
retrieval evidence, not absence from reality.

## Recovery

- Invalid time: correct it to the source's actual ISO 8601 time.
- Source conflict: retain the original, use a new identity for changed evidence,
  then reconcile superseded facts.
- Completed successful extraction with no facts: report `episode-only`.
- Partial/pending extraction: preserve `episode_id`, report `pending`, and follow
  retry guidance even when the CLI exits zero.
- Failure or timeout: preserve identifiers, report `pending`, and retry only
  according to returned guidance and idempotency guarantees.
- Empty recall: reframe terms within the same authorized boundary.

After a mutation, require a response or subsequent read that demonstrates
persisted state before reporting completion.
