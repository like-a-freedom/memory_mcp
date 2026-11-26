# Eval harness simplification: faster evidence without weaker oracles

**Status:** Approved by the user on 2026-11-26; implementation in progress.
**Date:** 2026-11-26. **Required runtime target:** local native Apple Silicon.
Portable deterministic tasks must not require a model host, vendor account, or platform-specific filesystem primitive.

## Decision and scope

Make routine memory development answer one question cheaply: did this implementation fix the named behavior without introducing regressions? Keep agent usefulness, external benchmark performance, operational latency, and deployment evidence as separate claims with the evidence each needs.

Change the existing harness incrementally. Retain corpora, independently reviewed truth, public product boundaries, historical reports, and compatible replay loaders. Do not build a replacement evaluation framework, add an orchestration service, or rewrite all Rust/Python code into one language. Line count and file count alone do not justify deletion.

This proposal changes future execution policy after approval. It does not revise frozen historical plans, turn old NO-GO results into passes, or close outstanding production-data prerequisites.

## Observed causes and limits

1. `MemoryDecisionsSuite` runs the primary variant and, on primary execution success, two additional variants: verbose/shadow and evidence diagnostic. Each variant starts a new server/store and repeats public ingest/extract. See [variant calls](../../../crates/eval-harness/src/suites/memory_decisions.rs#L509) and [setup](../../../crates/eval-harness/src/suites/memory_decisions.rs#L781). The current native gate calls four complete profiles: default twice and shadow twice ([gate](../../../crates/eval-harness/tests/deterministic_quality_gate.rs#L352)). Eight cases therefore nominally execute **4 × 8 × 3 = 96 scenario variants**, before counting their individual tool calls. This is work count, not measured time attribution; execution failures may prevent later diagnostics.
2. The generic runner awaits a suite without a global deadline and only evaluates `time_budget_seconds` afterward ([execution](../../../crates/eval-harness/src/runner.rs#L136), [budget verdict](../../../crates/eval-harness/src/runner.rs#L192)). Existing stdio startup/call/shutdown timeouts are useful but do not impose a whole-run deadline. A simple timeout around `spawn_blocking` would leave its running work alive.
3. LongMemEval copies the complete store for each question and invokes a cold CLI; its subprocess has no timeout ([adapter](../../../evals/longmemeval_v2/memory_mcp_backend.py#L237)). The copy protects against access counters changing later question rankings: [isolation contract](../../../evals/longmemeval_v2/README.md#L90). Removing isolation would invalidate the comparison.
4. The latest seven-case capture took 1,875.603 seconds overall. Its start-to-prompt-completion window was 1,615.753 seconds, about 86% of wall time. A separate saved-context reader replay took 69.848 seconds ([manifest](../../../evals/longmemeval_v2/results/2026-11-26-current-master-repeat/execution-manifest.json)). These are distinct stages/protocols, not interchangeable timings. Existing measurements do not separate copy, process/database startup, query, and shutdown or prove which dominates. Three workers overlap query durations; summed per-query time is not wall time. A faster reader alone cannot remove context-collection cost.
5. C1 retained hashes/match flags instead of actual synthetic query/selector values. The 4/4 to 0/4 outcome swing could not be replayed from exact requests ([review](../../../evals/paired/c1-agent-managed-correction-recovery-v7-selector-diagnostics-2026-11-25/CROSS_CAPTURE_REVIEW.md#L35)). Repeated model runs were being used to diagnose a gap in observation.
6. A judge exception aborts upstream scoring ([observer](../../../evals/longmemeval_v2/scored_coverage.py#L91)). Reader answers now survive, but collecting contexts, producing answers, and grading still lack one routine resumable path.
7. The default gate is a historical same-binary default-versus-shadow promotion experiment. Its acceptance requires a minimum relative gain ([acceptance](../../../crates/eval-harness/src/suites/memory_decisions/acceptance.rs#L396)). This checks a rollout contrast; it is not equivalent to checking the current revision against a reviewed previous revision. Existing 5/8 decision outcomes must remain visible failures, even when the relative rollout comparison passes.

The source tree contains 79 Rust files and 51,933 lines under `crates/eval-harness/src`, including inline tests. Additional Python experiment evaluators and archived profiles increase the navigation cost. This supports reducing active entry points and repeated responsibilities, not an automatic claim that these files are unnecessary.

## Approaches considered

| Approach | Benefit | Limitation | Decision |
| --- | --- | --- | --- |
| Tune models, concurrency, and individual commands | Small local changes | Leaves repeated scenario formation, ambiguous timings, and poor failure replay | Use only for measured bottlenecks |
| Simplify the execution policy and separate capture from grading in the existing harness | Reuses working oracles and public-boundary adapters; reduces repeated work | Requires explicit compatibility and cache invalidation rules | Recommended |
| Replace the harness/framework or unify all code into one language | Could yield a cleaner greenfield design | Repeats validation and risks losing historical comparability before proving a benefit | Reject for this work |

## Proposed execution policy

| Route | Question | Execution | When required |
| --- | --- | --- | --- |
| Focused development | Does this defect reproduce, and does the fix work? | Selected public behavior case plus controlled oracle tests | Each relevant edit |
| Current deterministic regression | Did any required memory behavior regress? | One primary capture of the complete current deterministic corpus | Before accepting a memory change |
| Specific contract checks | Are ordering, repeat reads, isolation, verbose output, or rollout behavior correct? | Targeted independent scenarios; explicit default/shadow or repeated captures when needed | Changes to those contracts; retain scheduled broader coverage where useful |
| Agent experiment | Can the named agent configuration use the memory correctly? | Fixed-evidence control, then actual autonomous tool use under a bounded declared budget | Claims about agent utility or changes to agent-facing workflows |
| External benchmark | How does this frozen benchmark protocol score? | Pinned upstream capture/reader/scorer, preserving its prescribed conditions | Benchmark claims or explicit milestone |

The default deterministic gate does not run every historical rollout arm or all verbose/evidence diagnostics. Moving a check requires naming its replacement and the behavior it still protects. A mandatory full current deterministic gate protects against incomplete change-based selection; only the inner development loop may run a subset.

For the current eight-case corpus, one primary capture would execute eight scenario variants rather than the nominal 96. This is a structural reduction, not a promised 12× wall-time improvement. Repeated reads remain inside their own regression scenarios. A single capture provides no evidence of stochastic reliability.

Do not enforce a new quality gate that requires a fresh minimum gain over an old feature flag on every unrelated change. Compare current results to a reviewed immutable baseline by **case and dimension**: required/forbidden evidence, assertion, status, time boundary, isolation, and resource constraints. Gains cannot offset a new wrong-source result elsewhere. New regression tests must fail the defective implementation and pass the fix at the public observable boundary.

Report separately:

- required cases passed;
- known failures, with stable IDs and reasons;
- new regressions and new safety violations;
- execution/measurement failures and unobserved cases;
- no-new-regressions verdict;
- all-cases-pass verdict.

The candidate never refreshes its own baseline automatically. Existing failures stay FAIL and prevent an all-cases-pass claim. Approval of scoped no-regression does not imply all memory behavior is correct.

**Baseline preparation is a prerequisite to replacing the gate.** Freeze the product executable/source identity present before this harness refactor, together with the corpus, oracle, scorer, configuration, and schema. Reuse an archived default capture only after verifying those identities and its raw evidence; a historical shadow arm is not the incumbent for current-product regression. If no compatible default capture exists, make one bounded primary capture through the existing production boundary. Independently review its per-case/per-dimension outcomes and record every known failure with its source receipt and reason before evaluating the simplified path. The historical 5/8 result is a reference to investigate, not an assumed baseline score or a license to grandfather new failures. Keep the existing gate active until this baseline and the new path's verdict-equivalence check are complete.

## One trace; independently rerunnable stages

Keep the existing adapters; expose one maintained path to capture and to regrade their supported artifacts.

1. **Capture memory behavior:** execute the actual request against an isolated scenario through the public MCP/CLI boundary; persist exact request, response, identities, and stage timings incrementally.
2. **Optional reader/agent execution:** consume a saved public-compatible response, or let the agent choose real calls, depending on the declared question. Keep the truth and grader hidden. Do not rewrite a bad query into the gold query on the agent's behalf.
3. **Score and compare:** deterministic, side-effect-free evaluation over saved evidence wherever truth permits. Model judgment is optional for semantics that cannot be checked reliably otherwise.

Use complete traces for explicitly public synthetic fixtures. Persist only the task's relevant inputs/outputs, not ambient environment variables, credentials, or hidden reasoning. Private-data mode may retain reduced diagnostics, but must declare what cannot be replayed; hash-only observation must not be presented as sufficient for reproducing an exact failed request.

Persist attempts and per-case statuses before proceeding. Resume appends attempt history and retries only the failed/unstarted allowed stage; it never overwrites failed observations or presents the best attempt as first-attempt success. A grading failure does not authorize another memory query or agent task execution. Preserve every scheduled case in the denominator.

Check stage compatibility from actual inputs, including all relevant dependencies:

- memory: executable/source identity, configuration, corpus/source bytes, schema and DB engine, immutable snapshot, exact request, extraction/embedding identity, clocks/order/seeds where used;
- reader/agent: saved context, instructions/tool contract, model/runtime identity, sampling and resource policy;
- grading: saved observation/answer, oracle, rubric/scorer identity and normalization version.

A changed dependency invalidates its stage and downstream results. Never substitute an incumbent's saved context for a candidate's capture. Comparing independently captured incumbent/candidate contexts is valid for a declared reader experiment; it is not execution of the candidate memory. Replay supplies **no fresh latency observation**. Keep capture/replay provenance and protocol modification flags explicit; using upstream predicates in a modified prompt/retry protocol does not make that replay a complete official benchmark run.

## Runtime bounds and performance

Enforce a real whole-run deadline propagated to stages and owned child processes. Bound startup, calls, shutdown, and retries; terminate and reap children and stop blocking workers on cancellation. Keep partial records and classify a timeout as execution failure, never as a product-quality pass. A deadline on an awaited blocking task alone is insufficient.

Measure preparation/copy, startup/database readiness, formation, query, shutdown, reader, and grading separately, plus total wall time and concurrency. Reuse the existing measurement capabilities where they suffice. Verify the first small sample before scheduling a large capture. Choose numerical budgets from those measurements and the maintainer's acceptable feedback time; do not imply this audit measured a future speedup.

Retain per-scenario isolation. Investigate a persistent public MCP server per isolated scenario where it removes repeated process setup. Reusing a live store across independent LongMemEval questions is conditional on proving access-state reset and question-order independence. Filesystem reflinks can be optional optimizations; portable copying remains a fallback. Do not shortcut formation by writing gold facts directly into storage in an end-to-end quality run.

## Honest and simpler oracles

Preserve independently reviewed expected facts, allowed alternative evidence sets, forbidden sources, temporal cutoffs, and explicit known/unknown/conflict outcomes. Separate insufficient evidence from proven absence. Existing structured retrieval evidence groups already provide a useful reusable pattern ([scorer](../../../crates/eval-harness/src/suites/retrieval_quality/scoring.rs#L31)). Prefer them over a new generic assertion language.

Validate grader controls cheaply before model runs: known-good, valid alternative, known-bad, unsupported extra assertion, missing evidence, wrong time/source, no-op, and an execution error. Apply controls relevant to each task; do not require a fixed tool sequence or a retry unless task semantics requires it. Return/citation equality is justified only where identity or exact representation is itself required.

The seven exposed LongMemEval cases are development/regression data. Further repeats can test those cases and execution variation; they cannot restore an unseen-holdout claim. Full-reviewed-span presence remains a literal diagnostic, not semantic evidence recall. Upstream list scoring allows unsupported extras; preserve it for upstream comparability while reporting stricter internal checks under distinct names. Typed deterministic synthetic checks do not establish free-text extraction or embedding quality; keep real-model component profiles for claims about those components.

Debug instrumentation and workflows on a stable development cohort. Freeze fresh, independently reviewed families only after the design is stable and a confirmatory question justifies collecting them. Continue to honor old stop rules. A development rerun after an evaluator change is neither a hidden extra confirmation attempt nor a reason to erase earlier failures.

## Minimal implementation order after approval

1. Enforce bounded execution and durable partial attempts; add missing synthetic request/response traces and stage timings. Reproduce cancellation/timeout behavior with controlled subprocesses, not long sleeps or expensive model calls.
2. Make optional diagnostics explicit, add selected-case execution, and separate the current regression gate from historical rollout acceptance. Preserve repeated-read/order/isolation and relevant rollout coverage in named focused tests.
3. Provide routine offline regrade/resume using existing trace loaders. Compare old and new deterministic graders over identical compatible saved artifacts; any verdict change requires an explained grader defect or an explicitly approved new metric, not a convenient threshold change.
4. Optimize the measured context-collection bottleneck, preserving independent snapshots and public formation. Characterize cold and warm runs separately and keep pre-change timings.
5. Maintain one current contributor entry point and corpus/profile index. Retain archived evidence and old loaders for declared compatibility; consolidate future C1 criteria into the existing evaluator only where duplication is established. Do not rewrite pinned historical evaluator scripts.
6. Run one bounded native validation of the resulting route, then an agent/LongMemEval experiment only if its own remaining question requires a new capture. Replays cannot establish improved agent utility.

No new dependencies or MCP tools are proposed. This is a targeted simplification of the current implementation, not a new release program. A numerical latency target remains to be established from isolated stage measurements.

## Acceptance of this simplification

- The selected original defect fails before the change and passes after it; valid alternatives still pass and adversarial controls still fail.
- Compatible saved traces receive equivalent deterministic verdicts under the old and simplified paths, except separately explained corrections; missing/error records cannot become success.
- The complete current deterministic suite runs at the native public boundary with no vendor/model-server dependency; no new per-case or per-dimension regressions are hidden by aggregate gains.
- Isolation, repeat reads/order, rollback/default contracts, and current model-dependent components retain explicit coverage appropriate to the changed behavior.
- Timeouts leave no owned worker/child running and keep all scheduled outcomes visible. Stage-only reruns do not re-execute earlier successful stages.
- Report actual wall time, stage time and work counts before/after on matched conditions. No factor-of-speed claim comes from reducing variant count alone.
- Agent utility, external benchmark score, and production semantic quality remain unavailable until their respective evidence is collected.

## Primary reference checked

Read through `/keenable-cli` on 2026-11-26: [Anthropic, Demystifying evals for AI agents](https://www.anthropic.com/engineering/demystifying-evals-for-ai-agents), published 2026-01-09. Its recommendations support deterministic graders where possible, checking transcripts and actual outcomes, and avoiding unnecessarily rigid action sequences. They do not establish that this proposed refactor will produce a particular speedup; the repository findings and later matched measurements must establish that.
