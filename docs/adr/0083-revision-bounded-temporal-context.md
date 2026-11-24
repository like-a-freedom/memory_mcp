# ADR-0083: Server-owned revisions and partial temporal context

Status: Accepted for the authorized temporal implementation; measured acceptance is recorded separately.

## Context

A source may describe an earlier valid time while arriving after a decision. Caller-controlled ingestion time and current mutable search projections cannot reconstruct what the server knew at that decision. Returning a recent episode instead of a visible extracted fact also loses the claim relation needed to distinguish support and conflict.

## Decision

Use server-owned committed knowledge revisions alongside valid time. Keep immutable revision-visible canonical fact selection in the memory context and relation projection in knowledge. Expose effective snapshot, partial coverage and omitted selectors. Supplement exact full-question FTS with a bounded revision-visible fact pool, preserving direct hits. Apply declared-subject grounding before budget selection, preserving numeric identity. Resolve explicit subjects by canonical name, bypassing fuzzy aliases. Exclude current global FTS statistics from historical ranking so future writes cannot alter a fixed revision. Never manufacture current/conflicting roles for episode-only evidence or missing relation coverage.

Preserve current retrieval for requests without explicit temporal axes. MCP `as_of` maps to both axes; independent valid and knowledge cutoffs remove that ambiguity for new callers. Reject caller-supplied `t_ingested`. Migration 056 preserves unknown provenance for legacy rows instead of backfilling invented history. No new MCP tool, dependency or migration modification is required by this follow-up.

## Consequences

Historical context is deliberately partial, including empty results. Bounded scans may miss older facts and must disclose truncation. Proper-name grounding covers a conservative lexical subset, not arbitrary alias/entity resolution. Revision ordinals are store-local. Restores must preserve ledger/clock/data as a unit; mixed-version writes and downgrade need a separately proven protocol.

The public contract is [TEMPORAL_CONTEXT](../compatibility/TEMPORAL_CONTEXT.md). Acceptance uses deterministic public-boundary observations and a frozen positive margin; [the evidence report](../evals/DETERMINISTIC_MEMORY_QUALITY.md) distinguishes diagnostic source coverage, complete decisions and optional agent outcomes.
