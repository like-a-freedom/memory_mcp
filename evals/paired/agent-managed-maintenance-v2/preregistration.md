# Agent-managed memory maintenance feasibility v2

Registered: 2026-11-22, before any live v2 agent trial.

## Question and estimand

Does a versioned maintenance protocol with a 2,048-token cap on each
`assemble_context` response complete one source-supported correction and
superseded-fact invalidation within the frozen per-phase limits? This is a
bounded feasibility/cost pilot. It does not estimate memory-product treatment
effect, population benefit, non-inferiority, or broad agent usability. The live
Codex calls are stochastic; the scorer and independent probe oracle are
registered deterministic components.

## Single protocol change from v1

The v2 Codex profile changes only `assemble_context.output_token_limit` from
8,192 to 2,048. It retains the same maintenance and recall instructions, four
memory tools, three-read/three-write workflow, host restrictions, model alias,
product binary, evidence rollout stage, per-phase budgets, scoring rules, and
probe contract as v1. Profile, protocol, host policy, simulator, task corpus,
plan, evaluator and executable identities are separately versioned and pinned.
The eight v2 history families are new and disjoint from v1; the v1 outcomes
were used only to motivate the lower response cap and are not part of the v2
score. Differences in case content mean v1-v2 token deltas are descriptive,
not a randomized or matched causal estimate.

## Frozen inputs and execution

- Plan: `plan.json`; tasks and private oracle: `tasks.json`; profile fingerprints:
  `fingerprints.json`.
- Protocol, host policy, simulator and retry policy are adjacent JSON files.
  Exact prompt bytes are archived in the adjacent `prompts-*.txt` files.
- The plan pins product, evaluator, Codex CLI, model alias, prompt, protocol,
  retry, task corpus, oracle and simulator hashes.
- Two independent repeats execute all eight family cases for each arm. Every
  family gets a fresh isolated store. Both arms use the identical product
  binary/configuration and have balanced order. Repeats are `run-01` and
  `run-02`; all in-scope trials remain in the denominator after the first
  agent invocation.
- Existing local Codex authentication is passed with `--codex-home`; no API
  key is required, copied to the runner environment, or retained in artifacts.
  The child Codex process remains read-only with shell, filesystem, web,
  plugins, apps, memories and permission-request tools disabled. The parent
  harness launches it directly on the user's laptop as explicitly authorized.
- Each maintenance phase must read the current fact, ingest one correction,
  extract once, verify the correction before invalidating the exact old fact,
  then verify final state. Recall uses a fresh read-only session. The fixed
  independent probe runs directly through public MCP against the same trial
  store and is not agent-authored.

## Frozen budgets and endpoints

Each maintenance and recall phase has at most 65,536 input/context tokens, 8
turns, 8 MCP calls, 2,048 generated output tokens and 120 seconds. The v2
reader response cap is 2,048 tokens per `assemble_context` result. Feasibility
requires complete coverage in both repeats, all 32 task answers correct, all
32 independent probes supporting the correction, zero unsupported or
prohibited sources and hard violations, exactly one successful ingest,
extract and invalidate per trial, exact correction episode binding, and every
maintenance phase within all budgets. Any uncompleted, over-budget, unresolved,
duplicate or failed action remains a failure; no exclusions after observation.

Report task and fresh-recall correctness, evidence and source violations,
operation accounting, episode-ID binding, per-phase budget status, input/output
tokens, time and tool counts by family, arm and repeat. Report agent outcome
variation as such: the product is identical in both arms, so arm differences
are not a product effect. Replay offline comparisons from frozen artifacts and
require deterministic equality of scored reports. The live agent itself is not
claimed to be deterministic.

## Decision rule and limitations

A passing gate means only that this narrow, single-correction workflow was
feasible on this one Codex CLI/model/host under the registered synthetic
histories. It does not guarantee improvement for other memories, models, hosts
or users, and it does not show an agent benefit. Failure is retained as a
no-go/diagnostic result; do not change the cap, prompts, tasks, product or
scoring after the first v2 invocation and call the same cohort confirmation.
No product memory implementation may be changed based on these data without a
separate proposal and new deterministic memory-quality evaluation.
