# ADR-0006: Separate observation time from claim validity

## Status

Accepted

## Context

An email, meeting, or imported document carries a timestamp showing when a claim was observed — not when the described state began or ended. Treating every source timestamp as the start of validity turns ordinary observations into false temporal transitions.

## Decision

Claims distinguish observation time from their validity interval. Validity is populated only from explicit temporal evidence or a trusted source-schema rule; otherwise it remains unknown. A claim with unknown validity cannot automatically supersede another claim.

## Consequences

- Late ingestion and retroactive evidence do not rewrite history merely because they arrived later.
- Snapshot sources may define deterministic rules that interpret their reference date as an as-of time.
- Differing claims with insufficient temporal evidence produce temporal ambiguity rather than automatic invalidation.
- Claim reconciliation must preserve temporal evidence and its derivation method.
