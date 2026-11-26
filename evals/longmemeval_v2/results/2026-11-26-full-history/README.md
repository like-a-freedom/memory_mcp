# Full saved-history diagnosis — 26 November 2026

This is a **resource diagnosis**, not a new LongMemEval answer score. No model
or LM Studio was used. Production memory code is unchanged from `e58e03fd`.
All captures ran natively on Apple Silicon. The archived seven-answer score
remains **2/7**; no new answer was generated or graded here.

## Inputs and retained attempts

`registration.json` pins the 100-trajectory input, all six files in the saved
full-store snapshot, exact source IDs, queries and reviewed spans before capture.
Each read used a separate disposable copy of the **entire** saved store, not the
two reduced packing fixtures. The store contains 151 episode segments. Original
snapshot hashes match before and after each attempt (`snapshot-integrity.json`).
Temporary copies were removed after their child processes were terminated and
waited for. The original store was not opened for writing by the diagnostic.

| Attempt | Dropdown `0c6b0c60` | Toolbar `f61a096f` | Interpretation |
| --- | --- | --- | --- |
| `attempt-01-config-missing/` | Missing database username | Same | Invalid startup, no retrieval measurement |
| Root capture, 120 s cooperative / 180 s external limit | External timeout at 180.143 s | External timeout at 180.141 s | Neither returned a stage receipt |
| `attempt-03-query-progress/`, 30 s cooperative / 45 s external limit | External timeout at 45.175 s | External timeout at 45.105 s | Existing completed-query observer and CPU sample localize work; no completed context |

The third attempt is an explicitly registered diagnostic amendment, not a
successful retry selected for reporting. All six scheduled case attempts are
retained. Each attempt's `capture-identity.json` records its binary and test
source hashes; process records retain exact commands and elapsed wall time.
The final formatted diagnostic source is checked separately and must not be
treated as a fresh performance capture of those earlier binaries.

## Measured facts and limits

Both valid attempts verified the exact reviewed source and complete serialized
span **before** retrieval: source availability is **2/2** in the full store.
This proves availability in the episode carrier, not correct fact extraction,
candidate admission, ranking or final answer accuracy.

The last attempt recorded 7 / 15 completed SQL calls for dropdown / toolbar,
totalling 4.559669 / 27.611514 s. Of these, 5 / 13 occurred after
`FULL_HISTORY_START`, totalling 4.547808 / 27.607112 s. These are sums of the
successful observed queries only. The observer does not report pending or
failed queries; these totals are **not** complete stage costs or end-to-end
latencies. Raw SQL and timings remain in the case logs; `summary.json` exposes
the denominators and nulls for missing measurements.

`attempt-03-query-progress/toolbar-cpu-sample.txt` is a two-second native sample
starting about 18.1 s after toolbar-process launch. It reports a 7.5G physical
footprint at that sample and shows active work under:

```text
recall_context → retrieve_fact_channels → select_fact_records_for_query
  → select_facts_for_lexical_terms → SurrealDB pipeline → sort/spill merge
```

This establishes costly lexical fallback execution before final selection and
packing in the sampled interval. It does not establish all-run peak memory,
the fraction of all runtime spent there, or that wide projection alone is the
cause. Narrowing the sorted payload is a hypothesis to test, not a measured fix.
The test profile uses debug application code; numbers cannot be compared with
earlier release-profile latency measurements to claim a regression or speedup.

The inner Tokio deadlines did not return before the external process bounds.
That observation demonstrates why a cooperative future timeout is insufficient
for this embedded workload. The process boundary ended the attempts. Candidate,
selection and packing coverage, completed read latency, and new answer accuracy
are **unknown**, not zero or successful.

## Reproduction

The ordinary deterministic suite does not require this saved local store.
These two opt-in integration tests require a separately built, hash-matched
snapshot; corpus preparation/build instructions are in the main
[LongMemEval runner guide](../../README.md). Do not silently substitute a new
formation run or a reduced history for the pinned snapshot.

```sh
cargo test --locked -p memory_mcp --features eval-support \
  --test full_history_evidence --no-run
```

Use the executable Cargo prints. Run each named test with `--ignored --nocapture`,
`MEMORY_FULL_HISTORY_INPUT` pointing to the registration and
`MEMORY_FULL_HISTORY_STORE` pointing to a **disposable full snapshot copy**.
An external supervisor is mandatory: launch the test executable directly via
Python `subprocess.Popen`, use `wait(timeout=child_seconds)`, and on
`TimeoutExpired` call `kill()` then `wait()` before deleting its temporary store.
Capture stdout/stderr into a fresh case log and retain timeout/exit status,
command, source/binary hashes, registration hash and before/after snapshot hashes.
Do not use an unbounded `cargo test -- --ignored` invocation against this store.
The optional macOS `sample` command diagnoses resource use; it is not needed
for scoring or the normal provider-neutral test suite.

## Next decisive experiment

1. Establish a public storage integration oracle for ordered, per-term lexical
   candidates, scores and full source fields using explicit time cutoffs. Include
   ties, content/index-key matches, duplicates, empty results and invalidation.
2. Test narrow candidate ranking followed by full-record hydration within one
   consistent read view. Keep the same terms, limits, tie-breakers and temporal
   rules. Reject any changed candidate or missing quote/provenance; do not trim
   generic terms or lower limits merely to obtain a faster score.
3. Only after equivalence passes, make one new bounded matched full-snapshot
   capture with stage receipts. Require both reviewed source blocks and the
   existing native memory gate without regressions; report timeout as timeout.
   Check the seven frozen answers only after usable full contexts exist.

The implementation ledger and acceptance details are in the
[follow-up plan](../../../../docs/superpowers/plans/2026-11-26-memory-quality-repairs.md).
