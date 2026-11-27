# Source-graph qualification, 27 November 2026

Frozen before outputs: 24 cases (16 exposed + 8 new author-created/source-reviewed development controls), 48 calls; source-ledger versus source-graph. No label or budget changes: 256 completion tokens, 60 seconds/request, retries0, 1800 seconds total. Acceptance requires all 24 candidate cases completed correct, format-valid and literal-source-quoted, with zero faults/truncations. On FAIL stop protocol iteration; full-history reader stays unactivated. No held-out or autonomous-agent benefit claim.


## Actual result: FAIL

One capture completed in252.861 seconds:48 started/finished requests,48 response bodies,unique request IDs,attempt1 only,no transport errors. Source-ledger:21/24 completed values,24/24 format,22/24 literal quotes,19/24 strict joint. Source-graph:22/24 completed values,23/24 format,20/24 literal quotes,19/24 strict joint; one length truncation. Three false-premise values pass3/3 in both arms, but that does not qualify all UNKNOWN and quotation controls.

| Cohort | Ledger value / strict joint | Graph value / strict joint |
| --- | --- | --- |
| Exposed16 |15/16;13/16|16/16;13/16|
| New development8 |6/8;6/8|6/8;6/8|

Graph failures: conflicting-repeat, opaque-intervening and clipped-repeat have correct UNKNOWN but quote graph JSON instead of original source; opaque-repeat starts reasoning and truncates before a boxed answer; virtualized-last wrongly returns None despite unresolved successor. No output is repaired. Graph did repair the previously exposed consistent-repeat value, but equal strict19/24 and the new failures reject promotion. Quotation presence is weaker than entailment and source binding; this gate cannot establish either.

The independent final review found no Important/Critical source-graph defect: all165 old prompts identical,243 raw ranges correct,all8 new labels grounded in their own sources. That does not make model responses correct. See final-semantic-review-v1.json, candidate-decision-v1.json and raw outputs. No product default change. full-history-activation.json is NOT_ACTIVATED under the frozen rule; the earlier source-ledger32 and conditional14 registrations remain unchanged. Protocol tuning stops here rather than rerunning exposed cases until success.

Preflight attempt1 found the old owned server absent, with zero dispatched model calls and no capture claim. The restarted runtime and rehashed13-file bundle were pinned before this single inference capture. Do not interpret the restart as a failed reader trial, successful retry or proof of cold model state. Portable deterministic suites and the optional SDK-controlled suite passed; exact receipts are in evals/results/memory-quality-verification-2026-11-27/reader-source-graph-final-python.json. Actual inference used local native Apple Silicon; no LM Studio server or remote model was used.


Cost: ledger37,075 prompt/480 completion tokens and102.174 summed request seconds; graph44,380/715 and149.948 seconds. Cache/order are not balanced; these descriptive costs do not establish a general speed effect. There is no joint-quality gain to justify the extra context as a default. Owned MLX runtime stopped after capture; cleanup receipt confirms PID exit and unavailable health port. Stores and raw evidence retained until acceptance.
