# Persisted Claim Evidence Fidelity Plan

## Goal

Make Claim reconciliation evaluation faithful to the persisted relation rows while preserving the one-Active-Namespace model and the existing Claim relation identity/seam.

## Delivery status

**Delivered — 2026-08-24.** Worker-path persistence, read-only evidence
projection, exact boundary validation/classification, eval coverage, ADR-0049,
and stale-document cleanup are complete.

## Scope

- Preserve active `policy_tags` when the Claim worker creates `ClaimRelation` records.
- Project the `ClaimEvidenceReader` Active Namespace into `EvaluatedRelation`.
- Derive the canonical v2 policy fingerprint from persisted raw tags.
- Validate and classify persisted relation boundary mismatches in `eval-harness`
  using exact fact lineage.
- Add focused projection, worker-path, and eval-harness regression coverage, while retaining the existing end-to-end Claim pipeline coverage.
- Remove the stale integration assertion that treated ignored legacy `scope`/`project` labels as active reconciliation boundaries.
- Correct stale evidence comments and reconciliation documentation.
- Add ADR-0049 and update the domain/architecture documentation only where the current contract is inaccurate.

## Out of scope

- No new MCP tool or public surface change.
- No replacement partition concept, request-level namespace, or reintroduction of `scope`/`project` semantics.
- No migration or bulk rewrite; the required relation fields already exist.
- No new generic Claim storage interface; the existing named `ClaimStore` seam is sufficient.
- No redesign of Claim relation identity, versioning, invalidation, or reconciliation policy.

## Implementation sequence

1. Add a worker-path regression test using a recording ClaimStore and the `Relations` rollout, proving two real same-slot facts persist a relation with the owning Claim's policy tags. Remove the stale integration assertion that only proves the default `TestMemory` rollout currently writes no relations.
2. Add evidence projection tests: the reader's Active Namespace is retained and the canonical v2 policy fingerprint is derived from raw persisted tags; incomplete source-fact rows remain excluded.
3. Implement the minimal worker and evidence projection changes.
4. Extend the eval harness persisted-evidence matcher to validate and classify
   the actual relation's namespace, policy identity, and source-fact lineage
   against exact boundaries, while keeping the reader independent of eval
   fixtures.
5. Update `docs/evals/CLAIM_RECONCILIATION.md` and stale code comments to describe the real read-only storage-backed seam and boundary checks.
6. Run focused Claim/eval tests, then the required package tests, clippy, formatting, diagnostics, and diff checks.

## Acceptance criteria

- The Claim worker's relation-construction path retains the active policy tags of its same-slot Claim; a worker-path regression test proves this through real claim extraction and reconciliation.
- Every evaluated persisted relation carries the reader's Active Namespace.
- Persisted policy identity uses the same v2 canonicalization as active Claim identity.
- A relation with mismatched namespace or policy metadata cannot satisfy persisted-quality matching.
- Rows missing either source fact ID are never treated as valid persisted evidence.
- No MCP tool count, request schema, storage routing model, or migration file changes.
- Documentation no longer claims that `ClaimEvidenceReader` avoids database reads.
- No integration test claims that ignored legacy `scope`/`project` values enforce active isolation.

## Rollback boundary

The implementation is isolated to Claim relation construction, the feature-gated persisted evidence projection, and eval-harness validation. If evaluation behavior reveals an incompatible historical fixture, revert the implementation changes without changing relation IDs or existing persisted records; retain ADR-0049 as the explanation for the intended contract.

## Decision record

See [ADR-0049](../../adr/0049-persisted-claim-evidence-boundary-fidelity.md).
