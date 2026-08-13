# Memory Contract

Load this reference before a write, when choosing time boundaries, or
when recovering from a non-success result. Live MCP schemas remain authoritative
for accepted fields and enums.

## Source

- `source_id` is stable and round-trippable. Same ID and content is idempotent;
  same ID with different content is a conflict.
- `content` is bounded, source-linked, verified, and secret-free.
- `source_type` uses a value accepted by the live schema.
- `t_ref` is the source's valid time in ISO 8601. The server normally owns
  transaction time.
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

Facts have valid time and transaction time. `as_of` asks what was both knowable
and valid at the specified point. Invalidated facts remain in historical and
diff views.

## Result states

| State | Meaning |
|---|---|
| `verified` | episode stored and returned facts inspected |
| `episode-only` | episode stored and extraction returned no durable facts |
| `pending` | the call failed or timed out; known handles preserved |
| `assembled` | returned context inspected within the stated boundary |
| `cited` | every used claim has source provenance |

Never collapse `episode-only`, `pending`, and `verified` into “captured.”
No matching durable fact means absence from memory at the requested boundary,
not absence from reality.

## Recovery

- Invalid time: correct it to the source's actual ISO 8601 time.
- Source conflict: retain the original, use a new identity for changed evidence,
  then reconcile superseded facts.
- Empty extraction: report `episode-only`.
- Failure or timeout: preserve identifiers, report `pending`, and retry only
  according to returned guidance and idempotency guarantees.
- Empty recall: reframe terms within the same authorized boundary.

After a mutation, require a response or subsequent read that demonstrates
persisted state before reporting completion.
