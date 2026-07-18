# ADR-0004: Register claim schemas, not world properties

## Status

Accepted

## Context

A closed registry of properties such as ARR, email, or project status would cover only a small fraction of real-world knowledge. Allowing unrestricted predicate strings would provide breadth but make comparison and contradiction detection unpredictable.

## Decision

The system registers a small set of compositional claim schemas that define structural slots and comparison semantics. Concrete metric dimensions, attributes, and relations remain open-ended and receive deterministic comparison keys. Claims are automatically compared only when their normalized structural keys match or an alias has been confirmed. Fuzzy or lexical similarity may create a possible-alias suggestion but cannot trigger contradiction detection or supersession.

## Consequences

- Generic comparators can cover many domain-specific properties without a global ontology.
- New real-world dimensions do not require adding code merely to exist as claims.
- Unknown synonyms remain separate until an alias is confirmed, favoring missed matches over false contradictions.
- Fuzzy matching remains useful for review and alias discovery without affecting automatic reconciliation.
- Claim schemas, key normalization, and alias packs can evolve independently.
