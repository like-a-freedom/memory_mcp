# Memory quality repairs implementation plan

> **For agentic workers:** Use superpowers:executing-plans and test-driven-development. Record observed RED → GREEN, exact source identities and scope in this ledger.

**Goal:** Implement the four approved quality repairs and publish matched deterministic evidence plus a bounded local qualification of actual agent reads.
**Architecture:** Keep automatic entity admission in its owning knowledge/memory modules; observe actual retrieval boundaries through eval-support without changing the MCP surface. Reuse existing provider-neutral public diagnostic and paired agent runners. Hidden answers remain evaluator-only, and missing observations remain invalid or unknown.
**Tech Stack:** Rust, Python standard library, existing MCP and loopback Chat Completions adapters; native Apple Silicon only.
**Spec:** User-approved action plan in `docs/evals/MEMORY_QUALITY_CLOSURE_2026-11-26.md:122` and the four annotated tasks in this session.

## Global constraints

- Work on master; preserve unrelated untracked skills and archived evidence. No new dependencies, tools or migrations.
- Evals only local Apple Silicon. No LM Studio runtime or remote model calls; optional already-cached standalone model remains replaceable via loopback adapter.
- Test first at observable public boundaries; run smallest relevant target during iteration, then the production crate/harness suites and required workspace clippy/fmt before completion.
- Preserve canonical names with digits, named projects and explicit manual resolve; no output-specific blacklist.
- Formation/candidate/selection/packing observations must come from actual boundaries with source identity and elapsed cost; absence cannot imply a causal stage.
- Old C1 remains stopped. New exposed development tasks qualify only their declared workflow; no automatic C2 or treatment-effect claim.
- Model experiments: freeze tasks, prompts, criteria and identities before capture, one capture plus one planned repeat if qualified; retain all scheduled failures. Do not retry to select success.

## Review focus

- Currency/scalar mentions versus genuine entity identity, including ambiguity of Atlas and metric acronyms.
- List order, complete sets, duplicate/extra elements, count and false premise; exact citations must bind the claimed source.
- Actual agent query and selectors must reach memory unchanged; fixed replies are controls, never actual agent reads.
- Temporal query axes differ from latest reads; match actual call and actual returned snapshot.
- Disposable stores/processes close; existing attempts are never overwritten; deadlines and socket timeouts retain their actual meanings.

## Task 1 — Entity admission and label policy

Owner files: `knowledge/entity_extraction/anno.rs`, `memory/episode/entity_extraction.rs`, owning public integration tests; versioned extraction fixture and suite registration.

- [x] Review source labels independently of predictions; freeze v2 policy, preserve v1 archive.
- [x] Observe RED on automatic scalar/entity noise; establish proper-name/digit/manual-resolve opposite controls.
- [x] Minimal class-aware admission fix, GREEN scoped tests and matched v2 before/after extraction metrics.

Task 1 evidence: `evals/results/extraction-entity-repair-2026-11-26/README.md`; matched v2: 5/9 → 9/9, TP13/FP7/FN0 → TP13/FP0/FN0. Scoped public E2E26, Anno22 and harness7 pass; combined verification below.

## Task 2 — Source-linked loss diagnosis and targeted repair

Owner files: `memory/retrieval.rs` and child modules, `eval_support/retrieval.rs`, public integration tests and existing LongMem diagnosis.

- [x] Capture two frozen list/count sources in bounded isolated stores; retain actual boundary inputs, selections, outputs and stage cost.
- [x] RED for causal attribution losing intermediate/source linkage; feature-gated observation GREEN.
- [x] If loss reproduces, public regression RED → owning-stage fix GREEN; new independent source control. If not reproduced, retain explicit unknown and do not invent a retrieval fix.

Task 2 evidence: `evals/longmemeval_v2/results/2026-11-26-stage-diagnosis/README.md`; actual source → candidates → selected full on both sides, packing 0/2 → 2/2 full blocks; six bounded Rust scenarios pass. Archived seven answers remain 2/7, no new full-history answer claim.

## Task 3 — Closed evidence use

Owner files: `evals/paired/memory_quality_diagnostic.py`, public CLI tests, new frozen reader contract fixtures/artifacts.

- [x] RED: correct reordered closed set must pass; extra/duplicate elements and a false quoted citation must fail.
- [x] Minimal exact-set and source-quote grading, preserving existing scalar/abstention contracts.
- [x] Freeze list/adjacency/count/false-premise tasks and run bounded local reader qualification twice under declared stop rule. Record failures and finite scope, rather than universal model quality.

Task 3 TDD: set-order and fabricated-quote controls failed as expected, then passed; public freeze rejects a quote absent from its cited source, duplicate set labels and unknown comparison modes. First exact-record screen retained as 6/12 failed contract (values/sources/abstentions12/12). Source-span v2 retained as11/12 strict (one shortened raw ID), no repeat after failure. Source-span and all-returned-source citation handles have25 public controls, including two-source capture and wrong/duplicate/unknown handle rejection. The explicit transport adaptation keeps exposed v2 texts, not independent source confirmation; v3 planned captures12/12+12/12=24/24 with0invalid. Raw failures and all registrations remain at `evals/paired/memory-quality-repairs-2026-11-26/`. Immutable native gate8/8 passed.

## Task 4 — Actual autonomous read qualification

Owner files: new task/probe/plan/criteria artifacts using `run-paired-chat-completions` and existing public trace capture; add runner behavior only if a reproduced defect requires TDD.

- [x] Freeze fresh ordinary/temporal tasks, hidden source-backed answers, actual-query/selector diagnostics and independent probes. Agent chooses its requests; adapter never repairs them.
- [x] Run two planned captures if first meets the registered gate; retain exact public requests and responses, budgets and source citations.
- [x] Offline deterministic qualification and explicit C2 decision. A same-product workflow screen is not a memory-treatment comparison.

Task 4 evidence: `evals/paired/memory-quality-repairs-2026-11-26/selector-qualification-session-reuse/`. Registered-read gate had public runner→qualifier RED→GREEN. Both-arm audit rejects absent/late/unlinked reads and works under Python optimization. First registration retained8/8 then incomplete repeat due300s infrastructure budget. Public TDD session reuse reduces33→17 processes without cross-trial store sharing. Before new model calls, execution erratum froze same exposed tasks and600s wholeworkflow limit; two captures8/8+8/8, both-arm v3audits and Rust two-capture qualifier eligible,432.967s total. This qualifies only lexical recall in this exact configuration, not formation/C2/treatment benefit.25s is observed acceptance, not MCP hard interruption; transport cancellation remains explicit future work.

## Task 5 — Combined verification and delivery

- [x] Rebuild current product, native eight-case gate and matched targeted metrics; archive code/binary/data identities.
- [x] Production crate and full harness tests, Python targets, required strict workspace clippy, fmt and diff checks.
- [x] Independent combined review, fix significant findings through TDD, final report and prepare verified changes for commit. Preserve this implementation plan until user acceptance.

Final verification: native product2807pass/45ignored; harness925pass/22ignored;0failed. Python target48pass, fullLongMem49pass/5skip, optimizedaudit10pass. Exact workspace clippy/fmt/diff exit0. Clippy required only moving an existing cfg(test) block after eval-support items; exact pre-capture source reconstruction is hash-verified, then6 public stage scenarios passed again. No function body changed after model captures. Independent code review and report-vs-raw review completed; GTV report wording corrected. Final report: `docs/evals/MEMORY_QUALITY_REPAIRS_2026-11-26.md`; logs/identities: `evals/paired/memory-quality-repairs-2026-11-26/verification.json`.
