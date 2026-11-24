# Temporal candidate paired-agent feasibility

> **Rollout status update (2026-11-21):** This agent study's no-go applies to
> claims of downstream agent benefit and to confirmatory agent-study
> promotion. It does not contradict the later deterministic memory-stage
> comparison that set `evidence` as the product default. That comparison and
> its limited scope are recorded in
> [the rollout archive](../../evals/paired/claim-rollout-current-master-2026-11-21/README.md).

> Configuration audit, 2026-11-21: the legacy HTTP `product-config.json` declares regex extraction and `memory_decisions/main`, but the current adapter clears the environment, leaves extraction at the product default (currently anno), and sets tenant storage to `tenant/tenant` plus control storage. The declared configuration is not implemented by this launch contract. Existing HTTP artifacts and numbers are preserved as historical diagnostic observations; they are not a configuration-attested controlled pilot or acceptance evidence. A file hash alone does not attest process settings. The new independent HTTP capture selects regex/disabled embeddings explicitly; this does not repair old runs retroactively. No cross-transport causal inference is supported.

**Decision: no-go for promotion and no confirmatory sample size.** The
candidate has two observed prohibited-evidence violations in the first valid stdio and
HTTP runs, and one in the second HTTP repeat. The three 95% family-cluster
intervals for downstream task success include zero or negative effects. The
resource reductions are descriptive on eight previously observed development
families; they do not establish improved agent usability.

This report supplements the deterministic product results in
[`MEMORY_DECISION_BASELINE.md`](MEMORY_DECISION_BASELINE.md). The deterministic
memory-decision evaluator remains the primary, vendor-neutral measure. These
optional paired runs ask whether a fixed agent can complete a task with the
public memory interface; Codex is only the capture adapter used here.

## Audit correction (2026-11-21)

Historical `direct_memory_success` in these paired receipts means evidence collected by the agent, not an independent fixed-history probe. Do not use it as the independent deterministic memory endpoint. The original scorer labelled every forbidden source `temporal_mismatch`, including unrelated entities. Those immutable receipts prove prohibited-evidence failures, not necessarily future-knowledge leakage. The corrective scorer emits `prohibited_evidence` for untyped prohibitions; legacy receipt values remain readable. No historical numbers or artifact hashes are rewritten. This correction does not remove the hard-failure no-go.

## Run identity and validity

All runs compare a clean baseline built from starting revision
`1ae1da4a4e3120f2de94753fc1a949caee3aced7` with the authorized dirty-worktree
candidate. They use the same eight v3-derived development history families,
same task manifest, pinned prompts, `gpt-6.1-sol` model alias, Codex CLI
`0.169.0`, budgets, and local arm order. The exact provider model revision is
not reported by the CLI trace. The plan therefore identifies a same-alias
invocation, not an immutable model build.

The eight families were already observed while developing the deterministic
candidate. They are not a held-out cohort. Stdio and HTTP also have different
transport-specific product binaries and plans; treat their results as
transport-specific descriptive comparisons, not as one pooled sample.

The first HTTP attempt is invalid and excluded. All 16 trials ended as
`model_failure` before a model turn. A one-task local smoke reproduced the
reason: HTTP returned `403 host not allowed`. The isolated runner bound a
dynamic loopback port but sent `Host: 127.0.0.1:<port>` while configuring only
bare hostnames. The production contract intentionally compares the complete
raw Host header, including the port. The runner now selects a loopback port,
adds that exact authority to its temporary allowlist, and sends the same
authority. No production host-validation rule was relaxed. The deterministic
seed simulator and the opt-in live HTTP seed/checkpoint test pass after this
change. The excluded receipts are retained under
[`temporal-candidate-http/results/invalid-host-authority-first/`](../../evals/paired/temporal-candidate-http/results/invalid-host-authority-first/).

## Paired results

Positive memory and task deltas mean candidate minus incumbent. Positive
resource deltas mean lower candidate cost. Intervals are 95% family-cluster
bootstrap intervals over eight families, with 10,000 draws and the plan-pinned
deterministic resampler.

| Transport / run | Task success, incumbent → candidate | Direct-memory gain, 95% CI | Task-success gain, 95% CI | Tool-call gain, 95% CI | Input-token gain, 95% CI | Wall-time gain, 95% CI | Hard violations, incumbent / candidate |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| stdio / 1 | 4/8 → 4/8 | +0.250 [0, +0.625] | 0 [-0.500, +0.500] | +1.625 [+0.500, +2.750] | +10,874.5 [+2,952.1, +18,766.3] | +9,281.8 ms [+2,904.9, +15,835.9] | 3 / 2 |
| HTTP / 1 | 4/8 → 5/8 | +0.125 [-0.250, +0.500] | +0.125 [-0.250, +0.500] | +0.750 [0, +1.500] | +9,345.5 [+1,364.9, +16,316.5] | +5,217.4 ms [+484.6, +9,181.6] | 3 / 2 |
| HTTP / 2 | 6/8 → 6/8 | +0.125 [0, +0.375] | 0 [-0.375, +0.375] | +1.125 [+0.375, +2.000] | +14,353.1 [+10,212.8, +19,275.8] | +7,090.1 ms [+3,402.8, +11,177.8] | 2 / 1 |

No run exceeded its registered budget. Every valid paired comparison still
contains a candidate prohibited-evidence violation, so none passes the hard-zero gate.
Direct-memory point estimates are positive but weak and reuse development
families. Task-success intervals do not establish a positive effect. Lower
tool and token costs are an observed property of these traces, not proof that
agents found memory more useful: the task-success and hard-invariant gates
remain unmet.

The frozen stdio baseline/candidate binary SHA-256 values are
`b8183356a88586763ecaf476504856520e86232417333dd03c424bea4436ac49` and
`1bc7b22e7cfdae6f357b3365573accc59119a833959abfa634e9c43bbfbbeabc`. The
Streamable HTTP baseline/candidate values are
`f0e4ea056a731b63cdf8964ab8380fd8fad7e3e7962639e3ab706fa67d35bffb` and
`f2ee9714f15f89d54d5d50d4dd7ea7d310ed5a27221d66d9244fef6d9bf0dc50`. The
tool-schema hashes were `bbdbae2c07912a2c09a1d27a5998ec56e0e08ffe564164a559aac9c5ddf41877`
for the baseline and `7b8c60a7d6bdc5d50a71b04e5d2d3dc514516dd69b46627f5b0481cd41c20e61`
for the candidate. Both transport plans use task-manifest SHA-256
`cd9889a2ffa2c0443e0d632a880c937e6143e9ef165c48a405859942400ab478`.

| Comparison | Run ID | Canonical plan SHA-256 | Manifest SHA-256 | Artifacts |
| --- | --- | --- | --- | --- |
| stdio / 1 | `25802-1795270000586208000-0` | `d2ed3da204ae9bbcf42d01715959c2d388343bfbbe191adbd86c4efd62800c77` | `8df539869ba2dc48dd8c65f7f3dc80169b4df7cf74c9fe10a335a6b6dda02052` | [per-arm outputs and checksums](../../evals/paired/temporal-candidate-stdio/results/run-1/) |
| HTTP / 1 | `32874-1795272113350562000-0` | `79cb74383f2043b9aad97720e99738b152d8ea6cd276b7da37a6dba6a52fdde4` | `8df539869ba2dc48dd8c65f7f3dc80169b4df7cf74c9fe10a335a6b6dda02052` | [per-arm outputs and checksums](../../evals/paired/temporal-candidate-http/results/run-1/) |
| HTTP / 2 | `33278-1795272667317817000-0` | `79cb74383f2043b9aad97720e99738b152d8ea6cd276b7da37a6dba6a52fdde4` | `8df539869ba2dc48dd8c65f7f3dc80169b4df7cf74c9fe10a335a6b6dda02052` | [per-arm outputs and checksums](../../evals/paired/temporal-candidate-http/results/run-2/) |

## Deterministic memory evidence and next experiment

The deterministic v3 replay in the baseline report remains frozen: the
evidence-stage temporal metrics are 0.667 evidence, 0.111 decision, and 0.111
complete-probe success; the complete-probe rate is 1/9. The default `shadow`
branch remains 0.556 / 0 / 0. The post-fix v2 replay improved one decision and
one probe relative to pre-fix while leaving the default shadow metrics
unchanged. These are narrow deterministic outcomes, not a general improvement
claim.

Do not preregister a confirmatory `N` from these runs. They contain only eight
already-observed temporal families, no ordinary-workflow stratum, no justified
smallest worthwhile effect or target alternative, and no candidate that passes
the hard source-support invariant. Powering a promotion rule before those conditions
are met would create a precise estimate of an unsuitable or unstable
candidate, not useful evidence. Keep the rollout at `shadow`.

For the next memory candidate, preserve v3 unchanged and write new development
cases around the observed failures: abstention with unrelated retrieved facts,
discrimination between similarly named entities, valid-time conflict roles,
and unverified correction wording. Add ordinary-workflow non-inferiority
cases. Score these through the deterministic public-MCP evaluator before
changing memory behavior; only consider a powered agent comparison after the
candidate clears deterministic hard invariants and a product impact rationale
sets the margins.

The per-arm runs, payload-digest receipts, and paired interval inputs are
preserved beside the plans:

- [`stdio plan and run`](../../evals/paired/temporal-candidate-stdio/)
- [`HTTP plan and two valid repeats`](../../evals/paired/temporal-candidate-http/)

The private Codex login was read locally via `--codex-home`; no separate
`CODEX_API_KEY` was required or copied into these artifacts. The scorer,
deterministic evaluator, plan format, and external-observation CLI do not
depend on Codex or any other agent runner.
