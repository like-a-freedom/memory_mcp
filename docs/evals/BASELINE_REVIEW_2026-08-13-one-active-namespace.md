# Baseline Review — One Active Namespace (2026-08-13)

**Scope:** approve the first PR and release evaluation artifacts for the
scope-free one-Active-Namespace contract (ADR-0038) as frozen regression
baselines.

## Artifacts reviewed

| Profile | Artifact | Approved baseline |
|---|---|---|
| PR | `target/evals/one-active-namespace-pr.json` | `evals/baselines/one-active-namespace-pr.json` |
| Release | `target/evals/one-active-namespace-release.json` | `evals/baselines/one-active-namespace-release.json` |

Both runs executed against the code at commit `6aa00349` on
`scopes-reimagine`, macOS/aarch64, debug build, after the code-review fixes
(ADR-0008 source-lineage gate, procedure v2 identity, projection scope-free
write path, eval corpus rebase).

## Review criteria (from plan Task 19 and the final validation gate)

- **Exact coverage:** PR = 113/113 cases across the declared suites
  (local-retrieval 61, extraction 9, claim-reconciliation 41,
  external-retrieval 2); Release = 117/117 (plus lifecycle 4). Coverage
  matches the updated profile manifests.
- **Zero invalid:** every selected case was `passed`; `invalid = 0`.
- **Gates:** PR 7/7 passed (recall_at_5 1.0, mrr 0.9918, top_1 0.9836,
  entity_f1 0.75, claim_precision 1.0, claim_recall 1.0, external recall 1.0);
  Release 9/9 passed (same plus action_grounding 1.0, poisoning 1.0).
- **Budget status:** passed for both profiles within the declared time budget.
- **Fingerprints:** package `memory-mcp` v1.7.0 / eval-harness 0.1.0,
  rust 1.97.1; `configuration_hash`/`profile_digest` remain `uncomputed` by
  design (compile-time placeholders), and `git_commit` is unset unless the
  build defines `GIT_COMMIT`; neither affects gate evaluation, which reads
  observed metrics only.
- **Baseline consumption:** re-running both profiles with
  `--baseline evals/baselines/…` reproduces `RESULT: PASSED` with baseline
  values equal to the observed metrics, so regression budgets are trivially
  satisfied and the comparison path is exercised.

## Approval

Both artifacts are approved as frozen baselines. Regression budgets in
`evals/profiles/pr.json` and `evals/profiles/release.json` are now enforced
against these files; `make eval-pr` / `make eval-release` pass them
automatically.

Replacement requires before/after artifact review and a dated rationale
(plan Task 19). The pre-approval `target/evals/` artifacts used for this
review remain available for comparison; they are not authoritative baselines
by themselves.

## Still open (unchanged from the plan)

- Remote SurrealDB permission/concurrent-migration verification (no remote
  instance available).
- Linux CI verification (macOS gates verified locally).
- Distributed `memory-mcp`/`memory-cli` skill publication to the global skill
  directory (repository-owned copies are updated and versioned with this
  change).
