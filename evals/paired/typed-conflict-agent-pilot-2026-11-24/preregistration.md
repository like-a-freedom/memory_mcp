# Typed Conflict Agent Pilot: Preregistration

**Status:** registration v2 frozen before any agent-task capture. Exploratory pilot only.

## Question and scope

Does the `evidence` claim-rollout stage improve a Codex agent's first-pass handling of synthetic one-item retrieval tasks with explicitly conflicting records, relative to `shadow`, while preserving a small finite set of ordinary recall controls?

This is a narrowly scoped development pilot. The incumbent and candidate use the same memory server binary, schema, tool surface, synthetic history families, prompts, task oracle, runner, and budgets. The registered product treatment is the claim-rollout stage (`shadow` versus `evidence`). It cannot establish a general memory improvement across products, models, tasks, or users.

## Frozen inputs

- `plan.json`: registration v2; 12 fixed cases, eight conflict families and four ordinary-control families; each arm order is balanced.
- `tasks.json`: frozen synthetic task statements and task oracles.
- `independent-probes.json`: frozen independent public-MCP memory probes.
- `impact-rationale.json`: outcome definitions, margins, analysis unit, and pilot/confirmation rules.
- `probe-preflight/preflight-summary.json`: deterministic scoring replay for all 24 family-by-arm captures.
- `freeze-manifest.sha256`: hashes of the registration inputs and this preregistration.

Registration v1 failed runner preflight because its RecallOnly task-prompt fingerprint was stale. No Codex process was invoked and no task was sent. The failure is recorded in `preflight-failures/registration-v1.json`; v2 changes only the fingerprint to the exact hash of the task-rules file embedded by the runner. The task set, expected outcomes, product arms, budgets, and analysis were not changed. The corrected registration and all frozen inputs were re-hashed before the first model capture.

All scenario text, histories, identifiers, expected answers, and supporting records are invented for this evaluation. No user, repository, company, or confidential data is sent to the remote model. The Codex CLI receives only those synthetic tasks and the registered public memory-tool interface.

## Arms and execution

- Incumbent: current product binary and `shadow` rollout configuration.
- Candidate: the same product binary and `evidence` rollout configuration.
- Agent: Codex CLI 0.171.0, alias `gpt-6.2-sol`, using the same authenticated local Codex home for both arms.
- The fixed agent task makes exactly one `assemble_context` read with a one-item budget. It must return the exact safe conflict response and clarification state, cite a returned supporting source, and finish without a hard violation or budget overrun. Ordinary controls require the canonical value, no clarification, and a source citation.
- Each arm receives a fresh memory store per trial. The run is public read-only stdio MCP; extraction, embeddings, and memory writes are disabled.
- Capture two complete paired runs in new output and temporary directories. The repeated family runs assess feasibility and repeatability; they are retests, not independent observations.

## Deterministic memory gates

Before agent execution, the independent fixed-query scorer must replay byte-for-byte-equivalent JSON results from each accepted capture. The expected typed-memory pattern is:

- Conflict stratum: `evidence` passes all 8/8 typed conflict probes; `shadow` passes 0/8.
- Ordinary stratum: both arms pass all 4/4 source-coverage controls.
- Every observation must be tied to the returned supporting source, accepted product binary, registered oracle, and the correct committed memory revision.

These gates confirm the treatment is visible in the memory output and that the probe/scorer wiring works. They do not substitute for agent-task success or imply a human-population effect.

## Pilot outcomes and interpretation

Primary pilot outcome: per-family paired agent conflict-task success. The minimum worthwhile point estimate for a future claim is +10 percentage points; sample size planning targets a +25 point alternative. The pilot estimates workflow feasibility, attainable headroom, direction, paired discordance, and within-family repeatability. It cannot confirm benefit, change the frozen margins, or be reused as confirmatory data.

Ordinary recall is a finite regression screen only. In each capture, candidate must pass at least 3/4 ordinary tasks and must not have fewer successes than incumbent. Four hand-authored controls are not representative; no population non-inferiority claim or powered ordinary-task inference will be made.

Any hard violation, budget overrun, invalid/incomplete artifact, deterministic memory-gate failure, missing conflict headroom, or task outcome that prevents a meaningful comparison is reported as pilot failure/no-go. No exposed family may be tuned or reused for confirmation.

## Confirmation design, if the pilot supports it

Confirmation must use a newly generated and disjoint cohort of synthetic conflict families, frozen prompts and graders, the same arm treatment, and one paired task outcome per independent family. The statistical unit is the history family, never a repeated model capture.

The confirmatory primary succeeds only when both conditions hold: (1) the candidate-minus-incumbent conflict-task risk difference is at least +0.10, and (2) the one-sided exact McNemar test rejects no difference at alpha 0.025. Plan for 90% power at true risk difference +0.25. Estimate the family-level discordance probability conservatively using the one-sided 95% Clopper-Pearson upper bound from the eight pilot conflict families, marking a family discordant if either registered repeat is discordant. Compute exact power by summing the binomial probabilities for the number of discordant families and the conditional binomial probabilities for candidate wins; do not use a normal approximation or treat repeats as extra families. Search integer sample sizes through the preregistered cap of 200.

If the conservative required sample size exceeds 200, pilot headroom is absent, or an operational/deterministic gate fails, record a no-go and do not silently lower power, widen the cap, change margins, or reuse the pilot cohort. The ordinary-control gate remains a finite regression screen, not a second powered primary. Report exact sample-size assumptions and limitations before any confirmatory capture.

## Frozen model execution command

Run each capture with the same frozen plan and inputs, a distinct fresh temp root, and a previously nonexistent output directory:

```sh
memory-eval run-paired-codex \
  --plan evals/paired/typed-conflict-agent-pilot-2026-11-24/plan.json \
  --tasks evals/paired/typed-conflict-agent-pilot-2026-11-24/tasks.json \
  --independent-probes evals/paired/typed-conflict-agent-pilot-2026-11-24/independent-probes.json \
  --codex-binary /opt/homebrew/bin/codex \
  --codex-home /Users/solovey/.codex \
  --memory-tool-profile recall_only \
  --incumbent-memory-server target/debug/memory_mcp \
  --candidate-memory-server target/debug/memory_mcp \
  --temp-root /private/tmp/memory-mcp-typed-conflict-agent-pilot-2026-11-24/run-N \
  --output-directory evals/paired/typed-conflict-agent-pilot-2026-11-24/runs/run-N
```

The command is intentionally provider-adapter-specific only for this authorized pilot; evaluator plans, deterministic probes, and offline scoring remain provider- and harness-independent.
