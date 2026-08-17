-- Migration 038: persist claim source spans.
--
-- `ClaimDraftCandidate.source_span` (byte offset range in the original source
-- content) is now carried through the projection pipeline onto the persisted
-- claim. Provenance metadata only: it does not participate in claim identity
-- (ADR-0013) and existing rows simply carry NULL.
DEFINE FIELD source_span ON claim TYPE option<array>;
