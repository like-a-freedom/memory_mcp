# ADR-0013: Use deterministic bi-temporal claim artifacts

## Status

Accepted

## Context

Projection, retry, backfill, and re-evaluation can process the same fact or claim pair many times. Random identifiers create duplicates; overwriting records erases extraction and reconciliation history. Claim truth in the world must remain distinct from the period during which a particular derived representation was current in the system.

## Decision

Claim and claim-relation semantic payloads are immutable and deterministically identified from canonical inputs. An open real-world or transaction-valid interval may be closed once, monotonically; it is never reopened or rewritten.

- A claim ID is derived from the namespace-local source fact ID, claim schema and version, canonical claim payload, and extractor fingerprint.
- A claim-relation ID is derived from the canonical ordered claim pair and a reconciliation-context fingerprint covering evaluator, schema, alias, cardinality, temporal, and source-policy versions.
- Canonical serialization is explicitly versioned; maps and qualifiers are sorted, text is Unicode-normalized, and typed values are serialized without locale-dependent formatting.
- Claim real-world validity uses `valid_from` and `valid_to`. Claim and relation transaction validity uses `t_ingested` and `t_invalid_ingested`.

Retrying identical inputs produces the same IDs. Re-extraction or re-evaluation under a changed fingerprint appends new artifacts and closes only the prior transaction-valid projection. Supersession closes real-world validity; correction closes transaction validity for the erroneous claim projection.

## Consequences

- Retries and backfill are naturally idempotent.
- Historical extraction and reconciliation decisions remain reconstructable.
- Canonicalization changes require a new version or fingerprint rather than an in-place rewrite.
- Tests must lock canonical serialization and ID fixtures across releases.
