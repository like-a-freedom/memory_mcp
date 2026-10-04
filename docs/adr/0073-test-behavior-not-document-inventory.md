# ADR-0073: Test behavior, not document inventory

- Status: accepted — 2026-10-03
- Supersedes: the documentation-guard portion of [ADR-0065](0065-reinstate-the-source-tree-and-doc-claim-guards.md)

Production regression tests must exercise a use case or externally observable
contract and assert its outcome: returned values/errors, durable state,
isolation, resource release, recovery, or a shipped artifact. Repository
directory presence and manually mirrored document status strings are not
evidence that production behavior works.

## Decision and evidence

Remove `tests/doc_claims.rs` and all three of its assertions. The reported CI
failure was reproduced in a scratch checkout by moving only the authoring
`docs/superpowers/plans/` directory: two assertions failed with the exact ADR and
AGENTS references from the CI log, while production code was unchanged.
Historical prose was also treated as a required filesystem dependency.

The third assertion, named `every_spec_status_line_matches_the_code`, did not
inspect code. It compared a status line with a hand-maintained expected table.
Updating both could make it pass with no implementation at all. Keeping it would
retain false assurance rather than protect a scenario.

Do not repair these assertions with placeholder directories, broad exemptions,
or by moving the same checks to `xtask`. There is no production contract to
preserve. Documentation status and implementation claims remain review
responsibilities backed by observed scenario results, not by matching labels.
ADR-0064's Cargo-only CI policy determines how useful checks run; it does not
make an arbitrary assertion useful.

## Testing standard

- A unit test crosses the owning module's public interface with fixed inputs
  and controlled adapters for time, identity and other external effects. Real
  databases (even embedded in-memory engines), filesystem, network,
  subprocesses and process-global environment are integration dependencies,
  regardless of Cargo's `unittests` label or source-file location.
- Each unit test has one scenario and one focal action. Split independent cases
  into separately named or parameterized tests instead of runtime
  `if`/`for`/`while` assertion logic. Do not call private implementation helpers
  directly or expose them solely for tests; retire trivial accessor/default
  checks that protect no rule.
- Arrange a concrete scenario, invoke the owning use case or transport
  interface, and assert the observable outcome. Cover failure, recovery,
  concurrency and limits where the contract requires them.
- Use narrow controlled adapters for ordering and faults; exercise real
  persistence adapters when the defect is in storage statements or transactions.
- For a regression, demonstrate that the scenario detects the original defect
  before accepting the fix.
- Establish ordering with barriers, permits or explicit future polling rather
  than sleeps. Bound subprocesses and asynchronous waits.
- File assertions are appropriate for actual product inputs/outputs, such as
  filesystem ingestion, backups and release bundles—not authoring directories.
- Keep architecture-policy lints explicitly separate from functional evidence.
  Source reachability and lexical checks do not prove that a use case is wired
  or correct; their own fixtures test the checker, not the product.

The existing runtime, protocol, tenancy and vector-persistence scenario suites
remain the regression evidence. No replacement directory or status-table test
is added.
