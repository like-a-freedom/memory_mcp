# ADR-0003: Claim extraction is deterministic and partial by default

## Status

Accepted

## Context

Extracting arbitrary structured propositions from unrestricted text requires probabilistic inference and introduces false contradictions. The default server must run with zero configuration and minimal dependence on external services or downloaded models.

## Decision

The default claim extractor runs in process and emits claims only for supported deterministic schemas. A fact whose structure cannot be determined reliably remains stored and retrievable without a claim and does not participate in automatic contradiction detection. Model-backed extraction may exist only as an optional adapter that is disabled by default.

## Consequences

- Default operation requires no external inference service or model configuration.
- Claim coverage is intentionally incomplete in exchange for predictable precision.
- Unsupported facts remain available through ordinary retrieval and provenance flows.
- New claim families must be added with deterministic rules and labeled evaluation cases.
