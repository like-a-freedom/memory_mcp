# Memory Contract

Load this reference before a write, when choosing time boundaries, or
when recovering from a non-success result. Live CLI help remains authoritative
for accepted flags and values.

## Source

- `source_id` is stable and round-trippable. Same ID and content is idempotent;
  same ID with different content is a conflict.
- `content` is bounded, source-linked, verified, and safe to pass through the
  current shell environment.
- `source_type` uses a value accepted by live command help.
- `--t-ref` is the source's valid time in ISO 8601. Omit transaction-time
  overrides unless the source supplies a justified value.
- Changed evidence gets a new source identity. Superseded facts are invalidated,
  never deleted.

## Storage boundary

One Active Namespace is bound at server startup (environment configuration, not
per-command input). No ordinary command accepts a `--scope`, `--project`, or
`--namespace` flag, and none should be invented or probed. Use repeatable
`--policy-tag` on ingest for content governance so recall-time policy filtering
has evidence to work with. Empty recall is not authority to widen a boundary.

Credentials never enter arguments or persisted content. Sensitive business data
is eligible only when the server's configured policy explicitly permits it.

## Time and results

`--as-of` asks what was both knowable and valid at the specified point.
Invalidated facts remain in historical and diff views.

Classify capture as `verified` (facts inspected), `episode-only` (empty fact
set), or `pending` (nonzero exit or unresolved failure). Classify recall as
`assembled`, and as `cited` only after every used claim has provenance.

Never treat exit zero, an episode ID, or an empty result as stronger evidence
than the structured response provides.

## Recovery

- Invalid time: correct it to the source's actual ISO 8601 time.
- Source conflict: retain the original and use a new identity for changed
  evidence.
- Empty extraction: report `episode-only`.
- Nonzero exit or invalid structured output: preserve identifiers and report
  `pending`.
- Empty recall: reframe terms within the same authorized boundary.

After a mutation, require structured output or a subsequent read that
demonstrates persisted state before reporting completion.
