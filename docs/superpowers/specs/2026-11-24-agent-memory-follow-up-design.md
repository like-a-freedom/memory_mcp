# Agent memory follow-up implementation plan

> **Status:** Proposed. The tasks below are future work.
>
> **For implementers:** Use Superpowers executing-plans or subagent-driven-development and TDD. Complete each task at its public boundary before changing the next layer.

**Goal.** Diagnose where agent tasks lose useful memory evidence, then improve the measured failing behavior without regressing ordinary recall, source integrity, or resource budgets.

**Architecture.** The evaluator observes the actual public memory read and scores it against the private task oracle. Product changes follow only when a fixed-input reproduction identifies the owning context. Deterministic replay remains the required gate for product changes. A separate paired live study measures downstream agent behavior.

**Scope.** Rust, Cargo, public MCP, versioned JSON observations and checked-in synthetic fixtures. Binding designs are [agent outcomes](2026-11-09-agent-memory-outcome-evaluation-design.md), [decision quality](2026-11-09-decision-grounded-memory-quality-design.md), [source authority](2026-11-09-ingestion-source-authority-design.md), and [ADR-0083](../../adr/0083-revision-bounded-temporal-context.md).

## Starting evidence

| Question | Archived result | Outstanding decision |
| --- | --- | --- |
| Does revision-aware retrieval improve fixed-history decisions? | Development and confirmation each improved from 1/8 to 7/8 complete decisions, with sufficient evidence rising from 6/8 to 8/8. | Expand coverage for older facts, ambiguous entities and realistic history sizes. |
| Does default `evidence` expose useful claim status? | v10 decisions 8/8 vs 2/8 under `shadow`, with evidence 8/8 in both arms. | Identify which fields an agent actually consumes. |
| Does an agent reliably complete a task from evidence? | Conflict probes 8/8 under `evidence`; agent task success 2/8 and 3/8. Ordinary fixed probes 4/4; ordinary agent success 0/4 in both captures. | The frozen pilot stopped before confirmation. Separate query construction, returned evidence and answer formulation. |
| Does inline `extract` evidence help maintenance? | v6 cut formation calls from 32 to 24 per capture and fit the candidate budget in 16/16 phases; the second capture scored 7/8 against an 8/8 task gate. | Diagnose the end-to-end miss and measure complete workflow cost. |
| Does explicit source import support the intended workflow? | Deterministic v4 source recall 4/4 against 3/4; one exact Codex workflow screened at 4/4 per arm. | Compare legitimate workflows with equal goals, including operator setup cost. |

Sources: [deterministic memory report](../../evals/DETERMINISTIC_MEMORY_QUALITY.md), [v10](../../../evals/paired/memory-decisions-v10-baseline-2026-11-23/README.md), [typed-conflict pilot](../../../evals/paired/typed-conflict-agent-pilot-2026-11-24/README.md), [maintenance v6](../../../evals/paired/agent-managed-maintenance-v6/README.md), [source-authority workflow qualification](../../../evals/paired/source-authority-codex-gpt62-sol-qualification-2026-11-24/runs/run-02/report.md).

## Constraints and review focus

- Keep the eight-tool surface. Use the public CLI and MCP seams, the owning memory/knowledge contexts, and narrow adapter dependencies. Preserve wire compatibility and historical artifact semantics. New dependencies and migrations follow the repository approval rules.
- PR regressions run with Cargo on macOS, Linux and Windows without provider credentials or a model server. Preserve the pinned local encoders only for the existing retrieval quality profile.
- Preserve the difference between a failed tool call, an empty successful result, an insufficient result and a wrong answer. Legacy artifacts carry unknown observations.
- Limit exported telemetry to closed enums, booleans and bounded counts. Reject raw prompts, arguments, source text, model responses and provider bodies at the persisted observation boundary. Keep evaluator oracles private.
- Preserve `partial` or `unknown` for incomplete historical visibility. A zero result in a partial snapshot cannot establish global absence.
- Freeze case identities, inputs, expected outcomes, product configuration, budgets and decision rules before reading results. Retests are correlated. Use task families as the analysis unit.

## Task 1. Attribute failures on the actual agent read

**Owns:** `crates/eval-harness/src/paired_agent.rs`, `paired_agent/tool_results.rs`, `paired_agent/observations.rs`, `paired_agent/chat_completions.rs`, `paired.rs`; bring `codex_agent.rs` to the same contract where the adapter sees the tool reply. Tests live in `chat_completions_runner.rs`, `codex_agent_runner.rs` and `paired_comparison_cli.rs`.

- [ ] RED: add scripted public-runner cases with a failed read, a successful empty read, a sufficient response without typed status, and a sufficient typed response with a wrong answer or missing citation. Keep the same successful independent probe in all four cases. Assert the independently named failure stage and unchanged independent score.
- [ ] Implement one shared normalized per-read outcome. Bind it to the actual tool call and its matching reply. Keep the read outcome, cutoff input disposition, source coverage, expected annotation match, completeness and citation membership distinct. Evaluate oracle-dependent checks in the scorer using returned observations rather than trusting an adapter's task verdict. If an adapter cannot observe a field, emit the explicit unknown state.
- [ ] Version changed external observations and receipts. Reject inconsistent read/reply associations and missing required observations. Preserve existing schema readers and byte replay of old archived reports. Test that persisted artifacts omit sentinel raw text.
- [ ] GREEN: run `cargo test -p eval-harness --locked --test chat_completions_runner --test codex_agent_runner --test paired_comparison_cli`. Verify model-free fixtures for both adapters and identical offline reports from two equivalent normalized inputs. Commit evaluator changes separately.

## Task 2. Isolate query choice, then evaluate response presentation

**Owns:** the paired runner and fake `MemoryToolSession` seam, `paired_agent/tool_results.rs`, `tests/paired_prompt_contract.rs` and `tests/chat_completions_runner.rs`. Product wire experiments, if earned, belong to `crates/memory-mcp/src/tools/assemble_context.rs` and the owning memory use case with `assemble_context_wire_contract.rs`.

- [ ] RED: freeze fresh ordinary, conflict and partial-history development families. For each family register one expected response from the real public tool. Drive the controlled runner with this fixed reply and with an agent-selected public read. Assert exact tool reply delivery, valid source citations and that both variants use the same history and budgets. No oracle content may enter the prompt.
- [ ] GREEN: add an evaluator-only fixed-reply mode that runs on registered or scripted responses and records the mode in its fingerprint. Check the following costs separately: successful reads, clarification turns, input/output tokens and budget failures. Keep the unconstrained public-tool path as a separate capture.
- [ ] Run a bounded all-synthetic development screen on the same agent configuration. If fixed replies work and agent-selected reads fail, target query arguments, defaults or documentation. If both fail on adequate evidence, compare the existing response against a compact presentation carrying a typed answer/clarify/insufficient disposition and source references. Hold every other product and runner input fixed.
- [ ] For a product response change require RED→GREEN public wire cases for ordinary success, conflict, partial history, reply size and explicit compatibility. Check normalized value correctness separately from strict schema/literal checks, and keep citation and unsafe-assertion checks independent. Require zero safety failures and no new ordinary misses on the frozen deterministic controls. Ship only the minimal proved change.
- [ ] Verify through the focused runner targets and, for a product change, `cargo test -p memory_mcp --locked --test assemble_context_wire_contract`. Record diagnostic conclusions and any unknowns in the paired evaluation guide.

## Task 3. Establish historical retrieval and identity boundaries

**Owns:** `crates/memory-mcp/src/memory/retrieval/pipeline.rs` and `admission.rs`, `src/knowledge/entity_resolution.rs` and revision-visible selection in the knowledge context. Tests: `historical_query_grounding.rs`, `historical_selector_exclusions.rs`, `assemble_context_reconciliation.rs`, and `crates/eval-harness/tests/memory_decisions_stdio.rs`.

- [ ] Freeze deterministic histories with 249, 250 and 251 eligible facts, plus an older target outside the supplement without a direct full-text hit. Capture explicit valid time and knowledge revision. Assert correct old/corrected fact selection, truthful truncation/completeness and no post-cutoff admission. Verify stable output after a later write and under fixed-revision replay.
- [ ] Freeze separate entity controls for case variation, a transliterated alias, similar names with different numeric IDs, and a query naming two subjects. Assert false merges, false conflicts and false admissions independently while preserving a positive explicit identity case.
- [ ] Run the focused tests against the incumbent. For a measured miss, implement the smallest owning-context change. Prefer bounded revision-safe candidate discovery for a missed old fact over raising the cap blindly. Preserve canonical identity before fuzzy matching. Version any historical selector before admitting it to complete historical reads.
- [ ] GREEN: run `cargo test -p memory_mcp --locked --test historical_query_grounding --test historical_selector_exclusions --test assemble_context_reconciliation`. Repeat both deterministic arms through public MCP. Require a preregistered improvement in the intended failure class, zero prohibited evidence, unchanged ordinary controls, truthful completeness and registered item/byte budgets. Record runtime separately on a fixed environment. Commit independent retrieval and identity fixes separately if both are warranted.
- [ ] Keep historical repair as a separate decision. Reproduce one pre-existing wrong assignment using supported storage first, inventory affected durable state and design a reversible repair with dry-run, replay and revision audit. An automatic migration requires its own review and recovery proof.

## Task 4. Qualify workflows and confirm only eligible effects

**Owns:** checked-in registrations under `evals/paired/`, the [paired runner contract](../../evals/PAIRED_AGENT_EVAL.md), and the existing pilot, comparison, reliability and power commands.

- [ ] Freeze separate populations for ordinary recall, typed conflict, agent-managed correction and operator import. For source import compare the same legitimate user goal via supported literal ingestion and explicit operator import, with identical input bytes. Account for setup actions and time, tool calls, tokens, exact writes, citation and end-to-end success.
- [ ] Qualify an exact runner/model/configuration on a small fresh synthetic development set before scaling. Require the preregistered ordinary-task floor, independent probes, zero safety failures and resource ceilings. Treat transport/schema smoke as a separate cheap check. Register an optional local or authorized synthetic remote adapter as interchangeable capture infrastructure.
- [ ] Only for an eligible candidate, register a disjoint confirmation with a justified minimum effect, ordinary non-inferiority margin, power and a fixed resource cap before capture. Use the existing exact paired-power command when its binary endpoint and design assumptions apply. Validate the frozen analysis unit, replay the offline comparison and preserve unsuccessful runs. The historical conflict pilot remains a no-go and its 123-family calculation supplies planning context.
- [ ] Report deterministic, workflow reliability, resource and confirmation results in distinct columns. Use the [promotion rules](2026-11-09-agent-memory-outcome-evaluation-design.md#promotion-decision) for claims about agent utility. Gate subsequent claims on observed outcomes rather than checkbox completion.

## Task 5. Broaden coverage after the native gates

**Owns:** `evals/corpora`, `evals/profiles`, the `evaluation` CI job and [official LongMemEval-V2 design](2026-11-09-longmemeval-v2-official-run-design.md).

- [ ] Add distinct, synthetic history shapes with multiple updates, conflicting sources, lexical distractors and realistic history lengths. Preserve the exposed v4–v10 histories as replay/regression cases. Measure fixed public-call budgets, per-case success and query/runtime distributions separately.
- [ ] Verify the deterministic Cargo commands on Linux and Windows in addition to macOS, with no vendor login. Archive the exact invoked profile and per-platform receipts. Treat platform-specific model smoke as optional.
- [ ] Run the official benchmark as a separate project slice. Execute the pinned checkout and manifest preflight, lifecycle and order-independence probes, scorer reconciliation and failure accounting before a full scored run. Publish the dataset/runner identity and denominators if the official protocol completes.
- [ ] Keep retention, feedback, consolidation and index experiments behind measured triggers. Specify the observed error or resource limit and its deterministic comparator before implementing a mechanism. Reuse the follow-up evidence rules in the [research ledger](2026-11-09-agent-memory-review-evidence-ledger.md).

## Independent backlog

These items have their own evidence and acceptance criteria. They can proceed independently from Tasks 1–5.

- [ ] **Bounded explain citations.** Ratify the [existing spec](2026-11-13-explain-bounded-citation-output-design.md) in a newly allocated ADR. First capture public byte and Unicode reconstruction failures through `explain` and the response-size suite. Then implement the budget and source/provenance continuations at the memory use case. Require reconstruction, bounded over-budget behavior, wire compatibility and cross-platform response-size replay.
- [ ] **Extract quality after deployment.** Compare equivalent operation windows against the fixed [extract baseline](2026-11-02-extract-quality-and-cost.md). Track false relations, eligible entity recall and per-stage latency/query cost. Open repair only for a reproduced stored misattribution and follow Task 3's recovery criteria.
- [ ] **Assemble-context latency.** Re-run the [existing performance design](2026-11-14-assemble-context-latency-design.md) on a comparable SDK 4.0.1 host and profile. Change only a measured bottleneck, with correctness and resource controls in the same run. Keep the unavailable 512 MiB mode out of acceptance reporting until it has a valid baseline.
