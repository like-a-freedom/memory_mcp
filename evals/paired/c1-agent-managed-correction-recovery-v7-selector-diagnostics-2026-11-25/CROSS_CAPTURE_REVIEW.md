# C1 correction recovery v7 — cross-capture review

## Decision

**NO-GO for workflow qualification.** Capture 01 passed every gate and allowed the single preregistered repeat. Capture 02 failed the task-success floor in both arms (0/4 each), with one candidate `prohibited_evidence` violation. The stop rule is now exhausted; do not run a third capture on this cohort.

These are repeated model-backed workflow observations on the same four synthetic families and identical product binaries. They do not estimate a memory treatment effect. The sharp 8/8 to 0/8 change in exact-cited task outcomes across captures shows that one passing capture is not a reliable qualification signal. Do not pool repeated families as eight independent cases.

## Arm-level comparison

| Arm | Capture 01 task success | Capture 02 task success | Probes (01; 02) | Extracts (01; 02) | Invalidations (01; 02) | Reads (01; 02) | Mean recall ms (01; 02) | Hard violations (01; 02) |
|---|---:|---|---|---|---|---|---:|---|
| incumbent | 4/4 | 0/4 | 4/4; 4/4 | 4/4; 4/4 | 4/4; 4/4 | 4 (4 nonempty); 7 (1 nonempty, 6 empty) | 14186; 18614 | 0; 0 |
| candidate | 4/4 | 0/4 | 4/4; 4/4 | 4/4; 4/4 | 4/4; 4/4 | 4 (4 nonempty); 7 (1 nonempty, 6 empty) | 15341; 20226 | 0; 1 |

Across both captures, all independent probes, exact-once correction extractions, exact-once invalidations, exact stale-fact target checks, and pre-invalidation evidence checks passed 4/4 per arm per capture. No resource exits occurred. Requested and returned valid-time checks matched on every read (4/4 in capture 01; 7/7 in capture 02). Exact first-read query equality was 0/4 per arm in both captures.

## What failed in capture 02

Each arm made seven reads: six empty reads over three tasks, each repeated once after authorized guidance, plus one nonempty read. The incumbent's nonempty read covered the registered sufficient evidence, but its final answer still failed exact answer and citation checks. The candidate's sole nonempty read did not cover the sufficient evidence; that task also recorded the single `prohibited_evidence` violation. All four independent probes still passed in each arm. This separates an agent-facing retrieval failure in three tasks from a final-use/citation failure in the incumbent's one evidence-bearing task and an unsupported-evidence failure in the candidate's one nonempty task.

The empty reads were marked partial. Their second empty result did not establish absence; the retry validator confirmed all observed retries followed the registered query-only contract.

## Selector field matches

Entries are `matches / reads`; false matches are diagnostics, not gates.

| Capture | Arm | fact_types | as_of | valid_at | known_at | known_revision | budget | view_mode | window_start | window_end | compact |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 01 | incumbent | 4/4 | 4/4 | 4/4 | 0/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 1/4 |
| 01 | candidate | 4/4 | 3/4 | 3/4 | 1/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 1/4 |
| 02 | incumbent | 7/7 | 4/7 | 4/7 | 3/7 | 7/7 | 7/7 | 7/7 | 7/7 | 7/7 | 1/7 |
| 02 | candidate | 7/7 | 6/7 | 6/7 | 1/7 | 7/7 | 7/7 | 7/7 | 7/7 | 7/7 | 0/7 |

`known_at` and `compact` mismatches are common in both captures. Capture 01 still returned sufficient evidence on every read and passed every task; therefore these fields alone do not explain the capture 02 failure. Requested and returned valid-time matches are complete even when the fieldwise `as_of`/`valid_at` booleans differ, consistent with the scorer's temporal alias normalization. The artifacts deliberately retain no raw selector values or query text, so the mismatch direction and exact supplied values cannot be reconstructed from these receipts.

## Conclusions and limits

- The current product can support this synthetic correction workflow: both captures passed the deterministic independent read probes and exact write/invalidation checks.
- Agent-facing recall and answer use are unstable across otherwise identical captures. A single pass is insufficient; this cohort ends NO-GO.
- Capture 02 does not isolate a product-memory defect. Query formulation, selector choices, provider variability, and final answer handling remain entangled. Selector mismatch booleans identify fields to investigate but not the values or causal mechanism.
- Do not change memory retrieval based on these results. A next diagnostic should be frozen on a new disjoint cohort and should safely distinguish whether the returned read contained the registered evidence and whether the agent used/cited it; any new selector detail should avoid persisting raw arguments.
- The separate positive ordinary agent-control v3 passed its registered screen, but that does not override this C1 NO-GO because the workflows and tasks differ.

Detailed artifacts: [capture 01](capture-01/REPORT.md), [capture 02](capture-02/REPORT.md). Frozen registration: `plan.json`, `tasks.json`, `criteria.json`, `fingerprints.json`, and `freeze-manifest.sha256`.
