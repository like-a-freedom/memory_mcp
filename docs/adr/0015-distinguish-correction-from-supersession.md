# ADR-0015: Distinguish correction from supersession

## Status

Accepted

## Context

"The value changed in June" and "the previous report contained a typo" describe different histories. Modeling both as supersession invents a real-world transition that did not occur. Retracting the whole source fact is also too broad when only one of several derived claims was corrected.

## Decision

`correction` is a first-class claim-relation outcome distinct from `supersession`.

- Supersession requires evidence that the world changed and closes the earlier claim's real-world validity interval.
- Correction requires explicit correction or withdrawal evidence plus source continuity or scoped authority for the same claim slot and validity context. It closes the earlier claim's transaction-valid projection.
- A value difference, recency, or higher confidence alone can never authorize correction.
- Whole-fact retraction remains reserved for withdrawal or invalidity of the complete source evidence.

## Consequences

- As-of audit can distinguish what was believed before correction from what was true before a real-world change.
- Targeted corrections do not erase unrelated claims derived from the same fact.
- The reconciliation corpus must contain correction, supersession, contradiction, and ambiguous near-miss examples.
