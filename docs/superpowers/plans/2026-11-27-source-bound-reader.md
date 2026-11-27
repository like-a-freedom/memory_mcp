# Source-bound reader and bounded workflow implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans task-by-task, with one final independent review. Existing authorization permits implementation on master and continuous execution.

**Goal:** Bind reader evidence to delivered source ranges and qualify bounded autonomous completion without weakening frozen criteria.

**Architecture:** Keep legacy protocol messages/captures immutable. A focused source-reference module validates and resolves line references; the existing reader runner registers the additive arm. Agent workflow uses explicitly registered progressive disclosure while preserving complete observed receipts.

**Tech stack:** Python standard library, existing local inference client, native Memory MCP binary; no new dependencies.

**Spec:** `docs/superpowers/specs/2026-11-27-source-bound-reader-design.md`.

## Global constraints

- Native local Apple Silicon evals only; portable deterministic controls; no LM Studio service or remote inference.
- Reader256 tokens/60 seconds/zero retries/1800 total; workflow9 turns/384 tokens/20 seconds/180 total/zero retries.
- Old outputs, failed gates, budget denominators and source labels remain unchanged.
- Oracle source identity and exact support are frozen before outputs and never supplied to the model.
- Release candidate SHA stays immutable; post-release work is separate.

## Review focus

- Correct value with an irrelevant or foreign reference must fail support (Task1).
- Escaped newline/supplementary Unicode must map back exactly (Task1).
- Malformed gold is invalid evidence, never a model failure or empty pass (Task2).
- A response timeout after successful operations keeps both the operation receipt and failed completion (Task3).
- Progressive discovery must expose every live tool and exact schema; cannot hide hard operations (Task3).

### Task1: Exact source-reference contract

Files: create `evals/longmemeval_v2/source_references.py`, `test_source_references.py`; modify `reader_protocol.py` and its public tests.

Public seams: `source_catalog(projected_sources)` -> source records/line ranges; `resolve_references(projected_sources,evidence)` -> exact decoded quotes/raw spans or ValueError; `classify`/`protocol_messages` additive arm.

- [x] Add one public source-bound classification control; observe RED for unregistered arm, then implement minimal catalog/prompt/JSON parsing and exact range resolution.
- [x] Add one control per defect through RED→GREEN: foreign source, wrong-source value, irrelevant support, duplicate keys, invalid line types/ranges, escaped Unicode and joined source boundaries.
- [x] Implement source-bound frozen support alternatives and coverage validation through the same public classifier; maintain separate source-binding/proof/value metrics.
- [x] Verify all prior frozen prompts match; run portable reader/attempt/coverage tests.

### Task2: Finite qualification and conditional full history

Files: new `fixtures/source-range-independent-v1/` and new result directory; update main quality report/plan.

Public seams: existing `freeze`/`capture` and `summarize`, with prevalidated source oracle.

- [x] Test freeze rejects malformed/unbound support and pins new module identity. Observe RED→GREEN.
- [x] Author/review new development controls and required spans before inference; register strict all-case acceptance/model/runtime hashes.
- [x] Run one finite local capture; retain all faults and raw outputs.80/80 calls; candidate35/40 value,39/40 binding,17/40 proof,16/40 strict; FAIL.
- [x] Apply conditional activation: strict all40 FAIL, full7 NOT_ACTIVATED with zero calls. No repeat-until-green or changed gold.

### Task3: Workflow completion and fresh qualification

Files: `evals/paired/skill_workflow.py`, public tests, new workflow diagnosis/capture directory.

Public seams: `SkillHost.dispatch`, `create_messages`, `play`, `freeze`/plan verification.

- [x] Save deterministic historical32-trial fault/prompt/usage diagnostic, preserving18 faults.
- [x] Test explicit progressive tool discovery returns exact registered schemas with no omitted tools; unknown tool rejected. Observe RED→GREEN.
- [x] Test unregistered mode rejection and old message identity; implement additive protocol only.
- [x] Test successful tool receipt + final timeout remains failed completion; no synthetic finish. Verify pending/partial/time-selector/source controls unchanged.
- [x] Freeze/run/review once:44/44 trials,37 answers,20 full, current10/22; gateFAIL. Raw actions/source/status/time reviewed; primary non-blinded adjudication explicitly labeled.
- [x] C2 decision-v4 remainsNO_GO: no independent qualified autonomous captures, no pilot/confirmation/effect/power. New44-trial development capture fails admission.

### Task4: Verification and reconciliation

- [x] Run affected portable Python suites and exact frozen-report replay checks; run Rust checks only if Rust changes.
- [x] Obtain one independent final review, fix substantive issues with RED→GREEN, verify source hashes and all registered attempts.
- [x] Update metrics/action plan and separate completed finite evaluations from failed quality acceptance.
- [ ] Restore preserved unrelated deletions after release dispatch.
- [ ] Reconcile hosted publication evidence without holding up quality work or claiming unverified assets.

## Execution ledger

### Task9: Raw-only whole-item simplification (post-capture)

Existing public seams: `protocol_messages`, `classify`, `freeze`, `capture`. No new production API, dependency or migration; no inference.

- [x] RED→GREEN: the new raw-only protocol delivers exact source text without derived navigation; the old whole-item name is not a live inference arm.
- [x] RED→GREEN: item-only freeze records raw-only treatments/projection; mixed range/item freeze retains range navigation.
- [x] Verify all whole-item identity/proof/value/limit/fault controls, legacy prompt identity and three archived replays. Measure prompt bytes only; do not infer accuracy/speed from byte reduction.
- [x] One final new-scope independent review and one substantive fix pass; record deterministic completion separately from unqualified model performance.

### Task10: Finite source-action v4 qualification on fresh inputs

Use the already registered `source-actions-native/v2` cohort and existing freeze/play/grade seams, not another transport or variant. Its first capture has not run. Preserve v1 examples byte-for-byte; v2 uses three newly authored sources on both MCP/CLI and both arms. State the final snapshot requirement in the timed question explicitly. Previous is the captured44 current skill, current is the pinned live candidate; this is a bundled development qualification, not isolated skill causality or C2 confirmation.

- [x] RED→GREEN at public freeze: v2 emits fresh source inputs and explicit actual-snapshot final requirement; v1 remains unchanged.
- [x] Freeze source/skills/binary/model/rubric before outputs; source review the new scenarios and strict all6-current/no-lost-previous gate. Keep9 turns/384 tokens/20s requests/180s trials/zero retries,2460s global including startup/cleanup and fixed12 denominator.
- [x] One native offline Apple Silicon capture, no LM Studio service; every start/terminal/error retained. Failure closes this capture, no further workflow inference in this task.
- [x] Review actual source/status/time/quote claims, archive exact implementations and replay. Update metrics and C2 decision without regrading old trials; one final new-scope review and one substantive fix pass.

Initial ruling: use a separate versioned contract rather than regrade incorrect historical QUOTE outputs. Source-bound references reduce copying burden but do not answer the question or establish support by themselves. Release preflight/hosted checks run independently; inference does not overlap heavy native compilation.

Ruling: implement deterministic Task3 seams while Task2 inference waits for release compilation to finish. These tasks share no model outputs; all qualification gates and caps remain unchanged. Task1 observed nine RED→GREEN defects/absent contracts; 15 new controls plus40 legacy reader controls pass, and213 legacy reader prompts/32 legacy workflow prompts match. A malformed first test import was corrected before its actual missing-registration RED; it is not counted as a product defect.


Historical pre-capture checkpoint (27 November): final reviewed deterministic suites reader150 (139 passed/11 skipped), paired94/94. Legacy messages213 reader and32 workflow match exactly. All four Important findings and one regraded fairness finding fixed through one RED→GREEN pass. Forty-case reader registration/strict acceptance is frozen before outputs; its sole capture is active. Six fresh native workflow scenarios are implemented; their capture waits for the immutable rebuilt v2.1.0 binary. No new model qualification outcome or memory-benefit claim yet.

Ruling: timed fresh native tasks use known_at2099 so freshly seeded facts are included reproducibly; they measure selector preservation and actual snapshot receipt matching, not historical transaction isolation. That distinct property remains covered by the existing deterministic temporal suite. Wrong source/fact/episode, failed process/envelope, lost time selectors and wrong returned snapshots fail the fresh operation grade.


Final reader outcome: the sole80-call capture completed with no transport faults. Strict16/40 FAIL; no production default change, no conditional full-history calls. Nineteen correct values omit preregistered proof coverage. Three unresolved-scope reasoning errors, one top-level array, and one extra role label account for five value-contract failures. Source identity39/40 is not answer entailment. See the current main report and frozen decision/audit. Native upstream restore5/5 subsequently passed; SDK reader145/150 plus those5 covers all scheduled native/SDK cases (portable subsets overlap). The new44-trial capture subsequently completed under frozen budgets:37 answers,20 full passes, current10/22 FAIL. The short-alias HTTP404 infrastructure attempt is retained separately, not hidden as successful inference.

### Task5: Preserve a recoverable correlated MCP tool error

Observed after the sole44-trial capture; a focused runtime correction, not a new inference round. Files: `evals/paired/memory_quality_diagnostic.py`, `evals/paired/skill_workflow.py`, new `evals/paired/test_skill_mcp_recovery.py`.

- [x] Archive and hash-verify the captured runner and MCP binding before edits.
- [x] RED through public MCP.tool→SkillHost→play: a correlated JSON-RPC invalid-arguments response must reach the agent; a later truthful finish is observed but original operation grade remains FAIL.
- [x] Add a narrow typed tool-error receipt; preserve actual JSON-RPC ID/code/message. Keep process death, initialization and malformed transport terminal. No auto-repair or retry.
- [x] Verify9 controlled subprocess cases, including process-death, foreign-response identity, malformed/init errors, released stdin/stdout and successful-recovery negative grading; all103 paired tests passed. Preserve old scores and require a new source-pinned freeze for future capture.

Task5 outcome:6 runtime RED→GREEN cycles (correlated tool-error recovery, pipe release, malformed error object, invalid transport JSON, incomplete correlated envelope, invalid tool-content JSON). A separate tampered-review RED→GREEN binds both manual review files to the stored capture-integrity hashes. Nine subprocess integration controls and103 paired tests pass. No model calls after fix. Saved44 operation grades/prompts and185 request lifecycles reproduce without model/native binary. Dynamic acceptance remainsFAIL, not upgraded by deterministic recovery.

### Task6: Whole-item evidence selection, deterministic stage

Files: existing `source_references.py`, `reader_protocol.py` and public reader tests. Public seams remain `protocol_messages`, `classify`, `freeze` and `capture` registration validation.

- [x] RED: new `source-item-final-first` accepts a full existing item selected by a short scoped handle; implement only the new contract.
- [x] Add individual controls for foreign/duplicate handles, wrong-source proof, value/unit mismatch, aggregate8000-scalar cap, escaped Unicode raw mapping, unresolved/conflicting/clipped scopes and faults. Existing strict proof alternatives remain unchanged.
- [x] RED: freeze/capture must pin the new reference/matcher identities and validate the reviewed oracle. Register explicitly; preserve every legacy prompt and saved grade.
- [x] Run the affected deterministic reader suites; keep dynamic qualification unactivated until new reviewed controls and finite registration exist. No inference during this task.

Task6 outcome:44 focused controls pass;171 reader tests scheduled,160 portable passed/11 skipped,166 installed-SDK passed/5 native-only skips. The native restore5/5 run predates these Python-only changes and is not counted twice.80 archived range/graph prompts and80 case grades reproduce unchanged. One final review found3 Important issues; one RED→GREEN fix pass rejects impossible oracles, non-scalar Unicode and stale cross-question handles. No second review or new inference. See `evals/longmemeval_v2/results/2026-11-27-whole-item-contract/verification.json`. This closes the deterministic implementation stage, not the failed dynamic qualification.

### Task7: Fair source-sufficient authorized actions

- [x] RED→GREEN through `grade_observed`: valid bound episode fallback may satisfy recall, while canonical-fact coverage remains false; exact value/claims reviewed separately.
- [x] Reject foreign source/episode/fact, changed snapshot, failed response/process, extra mutation, unsupported source-text verification. Preserve legacy v2 grades.
- [x] Accept authorized bound post-capture readback/explain without weakening the original ingest/extract/status requirements.
- [x] Register a distinct operation/cohort version and immutable prior skills; unknown/tampered mode rejected before inference. Live candidate skills never reinterpret old captures.
- [x] Use observed baseline skill failures to add only the missing forwarding/status/time/compact reference instructions; application-test one bounded local cohort, retain every failure. No repeat-until-green.
- [x] One final new-scope review, one fix pass, affected suites and old44 replay. Keep C2 NO_GO unless its separate independent qualification gate passes.

### Task8: Sole whole-item development qualification

- [x] Author16 fresh source/gold controls covering closure, hierarchy, duplicates, opaque/clipped evidence, roles, Unicode, numeric units, source choice and instruction/data boundary; source-review before outputs. Preserve40 shared controls as exposed regressions.
- [x] Freeze shared40 whole-item + fresh16 range/whole-item,72 calls total; exact all56 joint gate and no value regression,1800s global/256tokens/60s/zero retries. Keep historical range outputs unchanged and label their nonconcurrent comparison.
- [x] Run once native locally after workflow capture; retain all outputs/faults/deadlines. On FAIL stop reader inference and retain full7 NOT_ACTIVATED; on PASS explicitly preregister/activate full7 with preserved source audit.
- [x] Replay hashes/reports and reconcile metrics and C2 gate. No repeat-until-green or silent gold repair.

Task7 execution closed, acceptance FAIL:12/12 answered,8/12 frozen-v3 operation passes,8/12 primary-reviewed semantic passes,6/12 full (current3/6, previous3/6),54/54 successful request terminals,0 retries,433 seconds. Four MCP recall operations are falsely rejected by whole-object equality despite API-valid reduced selections. All four timed finals also independently fail snapshot/status clauses. Historical grades remain unchanged. A new v4/cohort-v2 selects actual source identity with exact returned proof and preserves foreign identity/content/offset/snapshot rejections; public RED→GREEN and140 affected paired controls pass after the final fix pass. No v4 model qualification or benefit claim. Post-capture references distinguish fact identity from quote verification and require actual snapshot in historical finals; their changed bytes are not qualified by the preceding capture.

Task8 closed by the registered stopping rule:72/72 calls,319.323s,0transport faults/retries. Whole-item format/binding/proof56/56; values/strict49/56. Shared40 strict16→35, proof17→40, values35→35 with one gained/one lost case; nonconcurrent comparison. Fresh16 range strict10/value14/proof10 versus item14/14/16, with one gained/one lost value. Strict56/56 and no-value-regression gates FAIL. Full7 NOT_ACTIVATED0calls; no further reader inference.72 prompts/grades and72 request lifecycles replay from archived scorers. Owned native server terminated. C2decision-v5 remainsNO_GO; this closes the finite evaluation task, not reader/workflow quality acceptance.

Final new-scope review fix pass:3 Important,0 Critical. Source-only objects/trimmed episode IDs now follow the public parser; missing episodes, invalid unselected entries and legacy aliases fail. Frozen reader replay loads the archived matcher before source references. Both workflow replays pin request logs and verify captured totals/outcomes; blank logs cannot yield false completion. Individual RED→GREEN controls,140 paired passes,171 reader scheduled/160 portable passes/11 explicit skips. Original44/12/72 captures and failed gates replay unchanged;0 new inference. See `postcapture-verification-v3.json` under the source-action result directory. No second review.

Task9 deterministic completion: two observed RED→GREEN slices (raw prompt registration and graph-free item freeze);175 reader tests scheduled,164 portable passed/11 explicit SDK/native skips. Mixed range/item and retired-name controls pass.96 legacy graph/range prompts,56 delivered source messages and56 historical classifications are identical; three archived captures replay unchanged. Content UTF-8 bytes shared40 159882→99151 (−38.0%), fresh16 28373→18805 (−33.7%), excluding framing/tokenizer/runtime. One new-scope independent review found0 Critical/Important. No new inference, no measured speed/accuracy improvement; old FAIL and C2NO_GO preserved. Receipt: `evals/longmemeval_v2/results/2026-11-28-raw-item-contract/verification.json`.

Task10 complete: sole capture12/12 answered,12/12 operations,10/12 semantic/full; current6/6 versus previous4/6, no lost passing previous case. All6-current/no-loss development gate PASS.54/54 successful request terminals,0 retries/faults;431.674s capture,444.589s startup-through-cleanup. Independent pre-inference source review0 Critical/Important; primary non-blinded semantic review of all12 finals hash-linked, replay12/54 succeeds offline. Both prior timed finals falsely call canonical facts fallback; current finals preserve actual source/status/quote/snapshot. Oldv3/full6/12 and reader49/56FAIL unchanged. C2decision-v6NO_GO: small bundled development qualification does not supply independent families or qualify reader. No further inference in this task. Evidence: `evals/paired/source-action-v4-qualification-2026-11-28/README.md`.
