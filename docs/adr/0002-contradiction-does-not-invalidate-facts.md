# ADR-0002: Contradiction does not invalidate facts

## Status

Accepted

## Context

Memory receives incompatible statements from different sources, plus legitimate updates describing how the world changed. Treating both as replacement hides source disagreement and corrupts historical answers.

## Decision

Detecting a contradiction records a relationship between claims but does not invalidate either source fact. Only a confirmed supersession may close the earlier claim's validity interval. It does not set the source fact's invalidation time; whole-fact invalidation is a separate retraction operation.

## Consequences

- Unresolved contradictory claims and their source facts remain available for retrieval and audit.
- Retrieval must identify contradictory claims instead of silently choosing the newest one.
- Automatic invalidation requires evidence of temporal supersession, not merely different content.
- Fact retraction remains available for erroneous, withdrawn, corrupted, or incorrectly ingested source evidence.
