# ADR-0009: Separate claim supersession from fact retraction

## Status

Accepted

## Context

A fact is provenance-bearing source evidence and may yield several independent claims. Invalidating the whole fact when one claim changes would erase unrelated claims and obscure what the source originally said. Conversely, an erroneous or withdrawn source must be retractable without deleting its audit trail.

## Decision

The claim is the lifecycle unit for supersession and targeted correction. Confirmed supersession closes only the earlier claim's real-world validity interval. Explicit correction closes the earlier claim's transaction-valid projection for the same validity context. Both leave the source fact unchanged. The fact is the lifecycle unit for retraction: whole-fact invalidation is reserved for erroneous, withdrawn, corrupted, or incorrectly ingested source evidence. A retracted fact and its derived claims are excluded from active truth selection while remaining available for provenance and audit.

The public memory contract must distinguish claim supersession from fact retraction. Existing fact invalidation must not be reused to represent routine claim updates.

## Consequences

- Claim storage needs separate real-world and transaction-validity lifecycles.
- A source fact remains stable evidence even after one of its claims is superseded.
- Retracting a fact deactivates all claims derived from it without deleting either the fact or the claims.
- The current fact-level `invalidate` behavior cannot serve as the implementation of automatic claim supersession.
