# Independent review of ece2b831..69735632

**Decision.** The frozen baseline is suitable for the eight registered synthetic cases. Keep the existing gate active until the raw-evidence and deadline findings below are fixed and the replacement passes through the native gate entry point.

## Verified evidence

I independently matched the manifest's product SHA-256 to the local binary, all three scorer/normalizer/case source hashes to both revisions, the corpus digest to its registered source bytes, and both index identities to the manifest. I checked all sixteen canonical raw receipt hashes, primary formation counts and success envelopes, isolated launch settings and prescribed repeat counts. The current CLI regraded both captures offline and returned zero with three visible failures. Every case has identical primary and temporal evidence/decision/probe verdicts and reason codes across captures.

| Cases | Primary evidence | Primary decision and probe | Temporal evidence, decision and probe |
| --- | --- | --- | --- |
| absent-subject, peer-conflict, prior-knowledge, unverified-correction, valid-boundary | Pass | Pass | Pass |
| office-entry, similar-name, single-current | Pass | Fail, `decision_unobservable` | Pass |

Source evidence for each case is the corresponding `receipts/receipt-<case>.json` under each capture. The current pass counts are 8/8 evidence, 5/8 primary decisions and 8/8 temporal decisions. The execution had zero invalid cases. No product source changes are in the reviewed range. Fresh output reservation protects deterministic grading attempts against overwrite. The opt-in synthetic recorder retains exact arguments and raw replies without copying ambient environment or transport error text.

## Critical

None identified.

## Important

1. **Missing or changed repeated reads can pass the raw gate.** In `suites/memory_decisions/verification.rs:391` only the first primary response is normalized. Removing the second office-entry response and canonically rehashing its v5 receipt still gave CLI exit 0. The live scorer at `suites/memory_decisions.rs:521` instead evaluates all repeats. Reuse that rule, validate the registered count, and reject missing observations or a saved score detached from a changed repeat. A genuine nonrepeatable capture should regrade to its actual failure. Cover both v4 and v5.
2. **Registration does not fully bind formation or runtime identity.** In `verification.rs:234` v4 checks diagnostic formation in place of primary formation. At lines 260 and 320 the loader checks extract requests and rollout overrides but accepts absent extract replies or a different NER backend. Three independent rehashed mutations each returned exit 0: remove all primary v4 ingest records, remove a v5 extract response, change v5 NER_EXTRACTOR. Validate primary and diagnostic histories against their own ordered checkpoints and bindings, require successful operation envelopes, and match all fixed public launch settings against the registered configuration.
3. **A deadline detaches external workers and discards completed evidence.** `runner.rs:43` drops the whole suite future. `suites/external_retrieval.rs:255` keeps spawned tasks in ordinary join handles, which detach on drop. Sequential suites such as `suites/retrieval.rs:539` also lose accumulated outcomes; every scheduled case is then reported as attempted. Preserve completed, running and unstarted states at case boundaries. An owned JoinSet with explicit abort and drain, controlled via an injected case evaluator, is suitable. Test one completed, one blocked and one unstarted case and check worker destruction before return.
4. **A deadline during an MCP tool call is labeled a product-quality failure.** Using the existing call_hang fixture through the CLI with a one-second budget produced `quality_failed` for absent-subject with a reported 120000 ms timeout. `suites/memory_decisions.rs:1145` treats the stdio deadline as an ordinary product failure. Preserve a typed execution-timeout reason even if the watchdog kills the process first and use the effective wait in diagnostics.
5. **Saved grades cannot identify the exact upstream capture or scorer.** `suites/memory_decisions/comparison.rs:196` records configuration/product identity shared by multiple captures. It omits index digests, scorer/normalization identity, individual passed cases and the reasons for the three known failures. Bind immutable input and grader identities and retain per-case dimensions/reasons. The related paired offline route at `paired_agent/observations.rs:574` validates before reserving output and deletes partial outputs on failure. Preserve an invalid attempt receipt there if documenting it as a resumable grading route.

## Minor and completion conditions

- Both capture READMEs use `decisionunobservable`; the raw stable reason is `decision_unobservable`.
- Public trace `call_failed` at `paired_agent/public_trace.rs:80` currently indicates a transport error only. Raw JSON-RPC/MCP error replies are correctly retained and the agent marks them failed, but the sidecar field is false. Name the field explicitly or represent response presence independently from operation success.
- Public phase sidecars are written after the phase returns (`paired_agent/chat_completions_backend.rs:371`). A runner crash loses that phase's calls. Document this granularity or persist unfinished attempts incrementally before claiming crash-safe traces. Rust profile preparation, including hashing the binary, precedes starting the runner timer (`main.rs:1777`). Keep wall-time guarantees explicit.
- After repairs, run one full `memory_decisions_current` capture at the normal gate entry point, bind the candidate to the invoked binary, compare against the registered immutable baseline, enforce complete coverage and report the scoped verdict. Pin both historical v9 profiles to diagnostics=true when omission becomes false. Update the active runbook/CI description and the plan checklist. Do not present the historical rollout gain as a current requirement.
- Retain the office-entry two-read check and its raw mutations. Source-order behavior has direct coverage in `assemble_context_supersession_ranking::timeline_keeps_persisted_predecessor_before_its_successor`. Preserve `assemble_context_wire_contract::compact_and_verbose_tool_json_preserve_ids_and_verbose_evidence`, revision-boundary/exclusion tests, stdio distinct stores and the Python forward/reverse isolation check. Rollout protection needs actual disclosure behavior: run `assemble_context_reconciliation::withholds_relations_below_the_evidence_stage` alongside evidence-stage controls, default configuration and explicit stdio shadow override tests. Keep historical profiles runnable. These focused checks justify removing four routine captures; they do not measure cross-run reliability.

## Declined to judge

- Agent utility, real-model extraction/embedding performance, unseen benchmark accuracy and production semantic quality require separate experimental evidence.
- General speedup, cold/warm comparability and the exact original harness executables cannot be independently verified from the frozen pair. The product binary was available. The candidate harness digest was not archived and the baseline executable digest is a recorded capture assertion.
- Cross-machine CI behavior, full LongMemEval snapshot identity, arbitrary descendant processes and reconstruction of external/private state were outside the native deterministic evidence. The Python bound is per CLI invocation; it excludes snapshot copying.
- I did not repeat expensive/ignored native or model runs or the complete suite and clippy runs reported by the executor. I ran the offline CLI, controlled timeout and mutation checks. The intended gate switch was pending at the reviewed commit.
