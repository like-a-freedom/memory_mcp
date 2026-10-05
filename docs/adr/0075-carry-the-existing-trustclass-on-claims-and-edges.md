# ADR-0075: Carry the existing TrustClass on claims and edges

- Status: accepted — 2026-10-05
- Related: [ADR-0074](0074-connect-the-reconciliation-relation-to-the-read-path.md)
- Narrows, and does not extend, [ADR-0016](0016-agent-memory-lifecycle-integration.md)

The frontier risk in this system is a trusted agent writing untrusted content
that later becomes a hidden instruction. The repository already models trust for
lifecycle events: `TrustClass` in `models/memory_event.rs` with an exhaustive
`TrustPolicy::may_derive` relation, including the rule that external content can
never promote itself. What is missing is trust on what a reader later consumes —
`Claim` and `Edge` carry no trust field, so a derived artifact's provenance
cannot be checked before it is retrieved or consolidated.

Neither the existing enum nor `may_derive` is recorded in any ADR; the trust
model is currently carried only by code and doc comments. This record closes
that gap. It narrows ADR-0016 rather than extending it: AD-3 of that record
concerns trust *authority* — that trust derives from the invocation channel and
never from tool arguments — which is a different question from where a derived
record's trust comes from.

## Decision

Reuse the existing `TrustClass` on `Claim` and `Edge`. Do not introduce a second,
domain-specific trust vocabulary.

A `TrustClass` on a claim or edge means **trust in the source that produced this
record**, not trust in a memory event. This distinction is recorded explicitly
because the type is shared: the same enum now appears on `MemoryEvent` and on
`Claim`/`Edge`, and a future reader will otherwise assume they mean different
things or, worse, that the memory-side value is redundant.

Two trust vocabularies would be the worst outcome of this decision. The
propagation rule that matters — derived trust is the minimum over its bases,
never greater — is then expressible in one `may_derive` call instead of two
policy implementations that can drift apart.

## Consequences

- **This decision requires a migration.** `claim` is declared
  `DEFINE TABLE claim SCHEMAFULL` and `edge` is `DEFINE TABLE edge TYPE RELATION`
  (migration 029); both reject a field the schema has not declared. Phase 6 adds
  the field to the base registry as `053`, which trips
  `latest_registered_migration_is_expected` (`knowledge/claims.rs:792`) — that
  assertion asserts the last base-registry entry is
  `039_filesystem_ingestion.surql`, so it must be updated in the same change. The
  HTTP-scoped migrations 040–052 are registered by a different path and do not
  participate in this tripwire.
- Consolidation must read the minimum over a derived record's bases and must not
  elevate trust through summarization. `may_derive` is the single place that
  encodes this; Phase 6 extends it rather than introducing a parallel check.
- `Edge` has no `namespace` field, so ADR-0038's one-Active-Namespace rule is not
  engaged. Trust is an attribute of a record, not a storage-context selector.
- The poisoning gate (`poisoning_pass_rate`) extends with a scenario proving a
  low-trust source cannot produce a high-trust derived artifact through any
  consolidation path. A gate that only proves the write path rejects untrusted
  content does not prove the consolidation path preserves that property.