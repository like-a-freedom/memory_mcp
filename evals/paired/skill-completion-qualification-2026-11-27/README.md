# Bounded progressive workflow qualification — 27 November 2026

**FAIL.** All44 registered development trials executed once:37 final answers,25 operation passes,20 strict full passes. Current10/22 and previous10/22; all22 current trials were required to pass. No independent memory/skill benefit or default change follows.

| Cohort | Scheduled | Answers | Operation | Strict full | Current / previous full |
| --- | ---: | ---: | ---: | ---: | --- |
| Six fresh native cases |12|10|6|6|3/6,3/6|
| Sixteen exposed regression cases |32|27|19|14|7/16,7/16|

These two cohorts do not estimate population accuracy. Compared with the historical32-trial capture, more exposed tasks finish (27 vs14), but binary, runtime and bundled prompt protocol differ; this is descriptive, not a causal improvement estimate. Primary source-based non-blinded review is saved per cohort, not mislabeled independent adjudication. All frozen expected_behavior clauses remain required, including source/status/time and explicit limitations.

Failures separate cleanly:

- Two fresh MCP trials terminate because the agent omits returned top-level source_episode when reconstructing explain items. Native assembly did supply it. This exposed a recoverable-tool-error handling defect in the host, not a missing product field.
- Two model response envelopes have finish_reason=tool_calls but both content and tool_calls null; one separate response has invalid text JSON. There are no inference request timeouts among185 valid-registration requests. All185 starts have exactly one terminal and no retries.
- Two long-source controlled MCP trials exhaust9 turns after redundant source-window/provenance reads and lack a final answer, even though the last tool response succeeds.
- Some answered cases misuse successful nonempty note as episode-only, omit required source/time/import-limit clauses, or incorrectly say compact adds inline evidence. Actual interrupted reembed is correctly reported unfinished; intentional controlled exit130 is not an unplanned inference fault.
- Restrictive frozen operation predicates also reject legitimate explicitly limited episode_fallback reads, an additional source-bound explain readback and a source continuation with honestly incomplete provenance. Their FAIL scores remain unchanged; these are prospective task/grading design issues, not fabricated facts or failed source binding. Advice/action ambiguity is labeled separately.

Runtime: native Apple Silicon, offline local MLX with the complete13-file model bundle rehashed, no LM Studio service or remote inference. The frozen full local model path is its advertised model ID.9 HTTP404 requests from an earlier invalid short-alias infrastructure attempt remain under invalid-model-alias-attempt, separate from the valid44-trial registration. Only that model identity was corrected; tasks, prompt, budgets and grades stayed fixed. Startup warmup has no task/gold and is not a scored trial.

Budgets unchanged:9 turns,384 completion tokens,20 seconds/request,180 seconds/trial,zero retries. Total valid capture1305.316s including native setup/cleanup; trial medians43.406s fresh and22.862s regression, maxima56.283s and63.051s. Costs are descriptive/nonisolated and do not qualify a20-second end-to-end pilot assumption. Owned local model runtime was stopped after all44 completed.

After capture, a narrow deterministic runtime fix returns a valid correlated tools/call JSON-RPC error as an observed denied receipt, allowing truthful recovery without erasing failure. Malformed transport, initialization failure and process death remain terminal. Native subprocess stdin/stdout now close. The nine controlled subprocess tests and all103 paired tests pass. No new inference or regrading follows that fix: captured runner and MCP binding were archived and hash-verified first; any future capture needs a new source-pinned freeze. C2 remains NO-GO.

Reproduce saved grades, initial prompts and request accounting without any model, binary or database:

```bash
python3 evals/paired/skill-completion-qualification-2026-11-27/replay_capture.py
python3 -m unittest discover -s evals/paired -p 'test_*.py'
```

The replay expects44 exact operation grades/prompts,20 linked reviewed full passes and185 unique started/completed requests. It verifies pinned human reviews; it cannot independently re-adjudicate their semantics. Raw trials, public MCP/process receipts, both frozen plans, acceptance, execution identity, startup/preflight and all negative results are retained. The prior historical32-trial8/32 capture is unchanged.
