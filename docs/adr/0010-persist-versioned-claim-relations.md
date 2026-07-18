# ADR-0010: Persist versioned claim relations

## Status

Accepted

## Context

An ingest-time warning is insufficient for audit, historical explanation, and deterministic retrieval. Reconciliation outcomes may also change when claim schemas, confirmed aliases, temporal evidence, source policy, or the reconciliation algorithm change. Overwriting an earlier outcome would erase how the system reached a previous answer.

## Decision

Persist reconciliation as a versioned `ClaimRelation` connecting two claims. Its outcome is one of `duplicate`, `supersession`, `correction`, `contradiction`, or `temporal_ambiguity`. Each relation version records the reason code, supporting evidence, evaluator version, evaluation time, and the earlier relation version it supersedes when applicable.

The claim pair is stored in canonical order for deterministic identity. Directional outcomes additionally record explicit predecessor and successor claim IDs; contradiction, duplicate, and temporal ambiguity remain symmetric.

Existing relation payloads are immutable. Re-evaluation appends a new version and makes it the active decision for that claim pair; the only permitted change to the prior version is monotonic closure of its transaction-valid interval. Any claim validity closure caused by supersession must cite the `ClaimRelation` that authorized it.

## Consequences

- Contradictions and ambiguities survive process restarts and remain explainable.
- Retrieval can use the active relation while audit views reconstruct earlier decisions.
- Changes to schemas, aliases, temporal evidence, or source policy can trigger explicit re-evaluation.
- Storage and metrics must distinguish newly evaluated, unchanged, and superseded relation versions.
