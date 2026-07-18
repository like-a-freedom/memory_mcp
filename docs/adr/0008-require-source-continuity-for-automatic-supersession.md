# ADR-0008: Require source continuity for automatic supersession

## Status

Accepted

## Context

Claims from official systems, documents, conversations, and informal observations can disagree. A newer observation is not necessarily more trustworthy, and allowing ingestion order to determine truth would let an unrelated or low-authority source rewrite established memory.

## Decision

Automatic supersession requires either continuity within the same source lineage or an explicitly authoritative source for the relevant claim schema and domain scope. Zero-configuration operation assigns no source authority by default. Claims from different non-authoritative lineages may contradict one another or remain temporally ambiguous but cannot automatically invalidate one another.

## Consequences

- Append order and recency do not silently choose a winner between sources.
- Connectors that represent versioned records should provide a stable lineage identifier.
- Source authority is explicit, scoped, and optional rather than globally inferred.
- Cross-source disagreement remains visible for retrieval and review.
