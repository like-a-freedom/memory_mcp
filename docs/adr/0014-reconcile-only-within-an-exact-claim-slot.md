# ADR-0014: Reconcile only within an exact claim slot

## Status

Accepted

Amended by ADR-0038: scope and project leave slot identity. Within the Active
Namespace, access-policy fingerprint, canonical subject, compatible claim schema,
and comparison key define the slot; qualifier hashes remain excluded.

## Context

The current warning detector scans a fixed set of active facts and uses fact type plus overlapping entity IDs. This can compare unrelated statements, miss relevant older records, and cross project boundaries. Fuzzy comparison makes automatic behavior difficult to explain and unsafe for private or multi-tenant memory.

## Decision

Automatic reconciliation considers only claims in the same exact claim slot. As amended by ADR-0038, the Active Namespace is implicit in the bound storage context, while access-policy fingerprint, canonical subject, compatible claim schema, and comparison key must match; scope and project are no longer slot dimensions. A confirmed comparison-key alias may establish key equality; fuzzy similarity and possible aliases may not.

**Qualifier exclusion:** Qualifier hashes are intentionally excluded from slot identity. Semantic qualifiers (e.g., `correction`, `transition`, `supersedes`, `replaces`) change the meaning of a claim but do not move it to a different slot — the identical comparison key with a different qualifier is precisely the case that requires reconciliation. Qualifier differences are evaluated inside the reconciliation decision engine (Gates 4, 7) and encoded in the relation outcome and `QualifierHash`, not in the slot fingerprint.

Candidate lookup uses indexed, stable pagination over the slot. It must never use a global table scan, a latest-N shortcut, or silently truncate the candidate history. Processing may stop at a time or page budget only if durable pending work records the remaining cursor.

Claims missing a required slot component remain retrievable through their facts but are skipped from automatic reconciliation with an explicit reason code.

## Consequences

- Access-policy isolation remains a correctness and security invariant, not a ranking hint. ADR-0038 removes scope/project isolation and defines one process as one authorization domain.
- Candidate generation becomes deterministic, indexable, and explainable.
- Qualifier differences are evaluated inside the decision engine, not as a hard isolation barrier.
- Coverage is intentionally conservative when identity is uncertain.
- Candidate-query tests must prove that older records are not lost behind a fixed limit.
