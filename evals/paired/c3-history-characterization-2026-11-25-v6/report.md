# C3 final query-boundary characterization

**Run date:** 2026-11-25  
**Host:** native Apple Silicon, `aarch64-apple-darwin`  
**Registration:** [frozen v6 registration](registration.json)  
**Product:** debug arm64 binary `23d49b598fb6b92585931dfeb5524b2ab3a53df9b754663d8094f2b9e23a448b`

## Result

The frozen four-case cohort passed in both captures (4/4 each), and the public
decision receipts matched exactly in the archived
[replay comparison](replay-comparison.json). The original v3 baseline passed
3/4 in both captures: `overlapping-measurements-without-correction-evidence` failed with
an unobservable decision, wrong disposition and missing evidence. The v6
candidate returned all three expected sources for that case and a `conflict`
disposition. The other cases cover exact numeric identity, an independent
source conflict and a transaction-time cutoff before a third event.

The defect was at query construction. A non-question follow-up sentence was
included in the lexical candidate search and could contaminate explicit
subject inference. Retrieval now uses the interrogative/request clause as its
lexical surface while the original complete query remains available to
admission, intent and temporal rules. Clause splitting retains tested
initialisms and decimals; auxiliary starters count as questions only when
terminated by `?`, so a negative directive is not mistaken for a question.

The cohort's first/repeat query latencies were:

| Capture | Min | Median | Max | Whole run | Max response | Max items |
|---|---:|---:|---:|---:|---:|---:|
| First | 7,311 µs | 12,911 µs | 22,448 µs | 91.550 s | 7,172 bytes | 3 |
| Repeat | 6,331 µs | 12,228.5 µs | 21,169 µs | 91.177 s | 7,172 bytes | 3 |

These timings characterize one local setup; the repeated captures establish
deterministic decisions, not independent samples or a performance treatment
effect. Requested budget was five per case and at most three were used.

## Historical scale control

The [historical-scale registration](../c3-historical-scale-2026-11-25/registration.json)
froze the 252-event, three-case profile before capture. It includes one target
record among 250 lexical distractors, an ordinary positive and an absent
subject. Both captures passed 3/3 and
[`compare-decision-replays`](../c3-historical-scale-2026-11-25/first/receipts/index.json)
reported identical decisions.

| Query | Expected result | Returned evidence | First / repeat latency |
|---|---|---|---:|
| Old subject among decoys | `supported` | `old-record` only; no `depot-*` source | 84,366 / 85,863 µs |
| Ordinary subject | `supported` | `ordinary-record` | 13,524 / 13,938 µs |
| Absent subject | `unknown` | zero items; no ordinary source | 3,053 / 3,321 µs |

The first and repeated whole runs took 131.878 s and 132.787 s. The archived
[replay comparison](../c3-historical-scale-2026-11-25/replay-comparison.json)
records a successful semantic comparison with hashes for both receipts. This
confirms that the expected source survives this frozen scale case under the
public read budget of five. It does not establish broad scale or production
latency.

## Verification

- `cargo test -p memory_mcp --lib memory::retrieval:: --locked`: 398 passed.
- Focused eval-harness fixture regression:
  `cargo test -p eval-harness --locked --lib evidence_diagnostic_keeps_its_ingest_and_extract_receipts`: passed.
- Strict workspace Clippy with the AGENTS.md feature set: passed after fixing
  the eval fixture's missing measured-read fields.
- `cargo fmt --all --check` and `git diff --check`: passed.
- The current native v9 quality gate passed; its [acceptance report](../native-quality-candidate-2026-11-25/claim-rollout-default-acceptance.json)
  records default 5/8 decisions and 8/8 evidence, shadow 1/8 decisions and
  8/8 evidence, no losses or evidence regressions, and matching repeats.

## Limits

The v6 oracle does not test an explicit claim-level correction workflow:
the overlapping records intentionally contain no correction marker. The
sentence-boundary parser is heuristic; initialisms, decimals, auxiliary
questions, negative directives and the frozen English cohort are covered, but
this is not a multilingual or arbitrary punctuation guarantee. The 252-event
control is one synthetic history, and all timing measurements are local.
LongMemEval's separate post-fix run is reported in
[`RECHECK.md`](../../../evals/longmemeval_v2/results/2026-11-25-c3-final-query-fix/RECHECK.md).
