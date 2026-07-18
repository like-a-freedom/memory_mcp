# ADR-0014: Reconcile only within an exact claim slot

## Status

Accepted

## Context

The current warning detector scans a fixed set of active facts and uses fact type plus overlapping entity IDs. This can compare unrelated statements, miss relevant older records, and cross project boundaries. Fuzzy comparison also makes automatic behavior difficult to explain and unsafe for private or multi-tenant memory.

## Decision

Automatic reconciliation considers only claims in the same exact claim slot: namespace, scope, project identity including an absent project, access-policy fingerprint, canonical subject, compatible claim schema, comparison key, and normalized qualifiers must match. A confirmed comparison-key alias may establish key equality; fuzzy similarity and possible aliases may not.

Candidate lookup uses indexed, stable pagination over the slot. It must never use a global table scan, a latest-N shortcut, or silently truncate the candidate history. Processing may stop at a time or page budget only if durable pending work records the remaining cursor.

Claims missing a required slot component remain retrievable through their facts but are skipped from automatic reconciliation with an explicit reason code.

## Consequences

- Scope, project, and access-policy isolation are correctness and security invariants, not ranking hints.
- Candidate generation becomes deterministic, indexable, and explainable.
- Coverage is intentionally conservative when identity or qualifiers are uncertain.
- Candidate-query tests must prove that older records are not lost behind a fixed limit.
