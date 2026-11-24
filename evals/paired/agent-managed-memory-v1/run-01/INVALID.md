# Invalid capture: managed supplement ordering

The first live attempt used the preregistered v1 plan but did not produce a valid evaluation. It executed all eight tasks in both arms, then the evaluator rejected its stage supplements because the plan's intentionally alternating task order was compared against a sorted key list. No sidecars or accepted comparison were produced. These receipts and arm artifacts are retained for the implementation regression, not scored or included in any pilot summary.

The regression is covered by `agent_managed_trial::managed_paired_run_writes_compatible_artifacts_and_reports_pairwise_probe_outcomes`: it failed before the fix and passes with unsorted manifest order. Per the frozen stop rule, the repaired evaluator requires a new plan identity and a fresh run set.
