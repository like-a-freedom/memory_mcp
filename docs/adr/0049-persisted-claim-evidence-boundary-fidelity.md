# ADR-0049: Preserve persisted Claim evidence boundary fidelity

## Status

Accepted — 2026-08-23, architecture audit remediation wave 4.

## Context

Claim reconciliation evaluation is intended to score the relation rows that the Claim worker actually persisted. The persisted `ClaimRelation` already has the active policy tags and source fact IDs needed for that evidence, and `ClaimEvidenceReader` already receives the process-bound Active Namespace. However, the worker currently writes an empty policy-tag array, while the evidence projection drops the reader namespace and uses a lossy ad-hoc policy representation. The evaluation harness also does not validate persisted boundary metadata against the exact fact lineage it loaded.

ADR-0038 makes one process-wide Active Namespace the storage boundary and classifies `scope` and `project` as legacy operational metadata. They must not be revived as request-level or relation-level partitioning fields.

## Decision

The persisted Claim evidence seam will preserve and expose authoritative boundary metadata without introducing a second boundary model:

- The Claim worker copies the owning Claim's active `policy_tags` into each persisted `ClaimRelation`. Candidate Claims are selected by the same slot fingerprint, so a relation is only persisted within the existing same-policy slot semantics.
- `ClaimEvidenceReader` projects the Active Namespace supplied at construction into every `EvaluatedRelation`. It never derives the namespace from legacy `scope` or `project` fields.
- The evidence projection derives the canonical v2 policy fingerprint from the persisted raw `policy_tags`; raw tags remain the durable source of truth.
- The read-only evidence adapter remains responsible for loading and projecting persisted rows. The eval harness remains responsible for comparing exact persisted fact IDs with its case boundary map and classifying isolation, policy, or unresolved-lineage mismatches.
- Rows without both source fact IDs remain excluded as pre-migration evidence. They are not assigned inferred lineage or synthetic boundary metadata.

No schema migration or data rewrite is required. Existing relation identity, append-only versioning, Active Namespace ownership, and named storage seams remain unchanged.

## Consequences

- Persisted-quality evaluation can detect relation metadata loss instead of validating only relation IDs and outcomes.
- Boundary semantics remain local to the existing Claim evidence seam and the evaluation harness; no production module learns eval-fixture policy.
- Historical rows without source fact IDs remain compatible but cannot silently pass persisted-evidence checks.
- Any future change to persisted policy metadata must update this ADR and the evidence contract together.

## Alternatives considered

### Reconstruct boundary metadata from `scope` and `project`

Rejected by ADR-0038: those fields are legacy operational metadata, not the active storage boundary or policy identity.

### Store only a policy fingerprint

Rejected: a fingerprint is not reversible evidence. Persisted raw policy tags are needed for audit and deterministic recomputation.

### Validate boundaries inside `ClaimEvidenceReader`

Rejected: the reader would become coupled to evaluation fixtures instead of remaining a small, reusable read-only adapter.
