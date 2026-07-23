# Procedural Memory

> Procedural memory is a separately gated bounded context. It is projected
> through the existing `FactType::Experience` seam, not exposed through new
> tools.

## Procedure gate

Procedural memory (Tasks 10–11) does not start until:

- the core release gate passes;
- at least three independent task families have successful and failed outcomes;
- one repeated lesson candidate has at least three independent trusted outcomes;
- the operator-review workflow has an owner and retention policy;
- the projected 365-day storage remains within the configured project budget.

**Current status: gated (shadow-only).** The migration and model exist, but
promotion is disabled. Absence of procedural memory is the correct result until
the gate is met.

## Candidate lifecycle

1. Candidates derive only from accepted lesson evidence linked to trusted
   outcomes.
2. They group deterministically by namespace, scope, project, and task
   fingerprint.
3. They append evidence (success/failure counts) and derive a Beta posterior.
4. They **never** auto-promote. An operator must explicitly promote.
5. Only current, promoted, scope-authorized versions become
   `FactType::Experience` records.
6. Existing `assemble_context` retrieves them under the existing shared budget.
7. Content changes create a new candidate/version; they do not edit promoted
   history.

## No public CRUD

There is no procedure tool, no public procedure parameter, no second unbounded
response collection, and no automatic edits to a promoted version. Operator
review uses the existing `open_app` and `app_command` tools.
