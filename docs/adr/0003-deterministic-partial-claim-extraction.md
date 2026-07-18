# ADR-0003: Claim extraction is deterministic and partial by default

## Status

Accepted

## Context

The default server must work with zero configuration and minimal dependence on external services, language models, or downloaded model artifacts. Extracting arbitrary structured propositions from unrestricted text would otherwise require probabilistic inference and would introduce false contradictions.

## Decision

The default claim extractor runs in process and emits claims only for supported deterministic schemas. A fact whose structure cannot be determined reliably remains stored and retrievable without a claim and does not participate in automatic contradiction detection. Model-backed extraction may exist only as an optional adapter that is disabled by default.

## Consequences

- Default operation requires no external inference service or model configuration.
- Claim coverage is intentionally incomplete in exchange for predictable precision.
- Unsupported facts remain available through ordinary retrieval and provenance flows.
- New claim families must be added with deterministic rules and labeled evaluation cases.
