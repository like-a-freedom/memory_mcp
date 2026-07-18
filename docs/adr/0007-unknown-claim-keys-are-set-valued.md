# ADR-0007: Unknown claim keys are set-valued

## Status

Accepted

## Context

Many real-world attributes and relations can hold several simultaneous values, including employment, contact methods, roles, and organizational relationships. Inferring single cardinality from a relation name would cause valid concurrent claims to invalidate one another.

## Decision

Every comparison key has a cardinality policy. An unknown or newly derived key is set-valued by default. Automatic supersession is allowed only when a key has an explicitly confirmed single-valued policy and the subject, qualifiers, and temporal evidence identify the same logical slot.

## Consequences

- Open-ended comparison keys remain safe without a complete world ontology.
- New values for unknown keys coexist instead of silently replacing prior values.
- Single-valued policies can be supplied by a claim schema, built-in alias pack, or trusted local extension.
- The existing global singleton predicate list must not drive claim reconciliation.
