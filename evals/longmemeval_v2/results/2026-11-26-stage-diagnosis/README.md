# Two source-linked packing regressions

Development repair evidence, 2026-11-26. No model calls or full-history ingestion.
The two inputs are the actual independently reviewed frozen menu (`0c6b0c60`)
and nine-action toolbar (`f61a096f`) states, each formed through public
`ingest` + `extract` into a fresh isolated in-memory service. The fixture is
`../../fixtures/reviewed-state-cases-2026-11-26.json`; its content and reviewed
span hashes bind these inputs to the frozen sources. These are isolated state
reproductions, **not** current reads of the original 100-event histories.

The feature-gated EvalSupport seam drives the same recall policy, retrieval,
selection, and byte-excerpt packaging used by public context assembly. It
returns receipts explicitly; no raw memory content is added to production logs.

| Observed boundary | Before, both cases | After, both cases |
| --- | --- | --- |
| Persisted source episode carrier (`formation`) | Complete reviewed span | Complete reviewed span |
| Ranked fact / admitted episode candidate union | Complete reviewed span | Complete reviewed span |
| Selected items immediately before byte packaging | Complete reviewed span | Complete reviewed span |
| Packed context | Complete span absent | Complete span present |
| First observed loss | Packaging | None observed |

`pre-repair-test.log` records both public regressions failing with the original
lexical excerpt call, while the stage observer and independent control pass
(2 pass / 2 fail). `post-repair-test.log` records both repaired public reads and
three controls passing (5/5). The trace snapshots and source episode identities
are retained in `pre-stage-receipts.json` and `post-stage-receipts.json`.
`pre-report.json` / `post-report.json` are deterministic CLI analyses of these
receipts, with **no reader outputs**: reader correctness remains unknown.

The minimal repair preserves a short maximal group of adjacent accessibility
buttons/lists when the selected lexical window intersects it. Plain and
JSON-escaped newlines/indentation are handled. The existing **2048-byte limit**,
byte-exact quote, and original-source byte offsets remain. An oversized group
falls back to the lexical window; its short suffix is not treated as a complete
group. Ordinary queries cannot lose lexical score; explicit UI enumeration
terms may favor a complete control group over extra matching page context.
This is a structural packing rule, not a general semantic completeness claim.

## What is actually observed

Formation here means a read of the persisted **episode carrier**, not that all
atomic facts were extracted. Fact candidates are captured after channel ranking,
and episode candidates after fallback admission, before final reranking and
selection. These are the declared candidate boundaries, not every internal
storage/filter stage. A cache hit may lack candidate observations; the diagnostic
then leaves the missing earlier boundary unknown. Absence of the full reviewed
span is conservative: it does not prove every answer is impossible, and candidate
sets might collectively carry an answer without containing the contiguous span.

The unchanged seven archived source/context/reader receipts are independently
replayed in `archive-recheck.json`: strict answers **2/7**, complete spans **2/7**,
query median **423.736693 s**, max **991.328374 s**. Only the previously observed
complete-evidence reader failure can be localized there. This isolated repair
does not retrospectively identify their unobserved formation/selection/packing
stages or establish that their full histories are repaired.

## Latency and limits

One observed sample per source/version, disabled embeddings, local native
Apple Silicon, no remote inference. Times below include the diagnostic source
read and snapshot cloning. Stage times are **cumulative offsets from read start**,
not isolated stage costs or a benchmark improvement.

| Case | Version | Public ingest + extract | Ranked facts | Episode candidates | Selected | Packed |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Menu | Before | 162.746 ms | 36.098 ms | 55.813 ms | 57.685 ms | 63.521 ms |
| Menu | After | 165.170 ms | 37.106 ms | 57.650 ms | 59.661 ms | 66.179 ms |
| Toolbar | Before | 630.817 ms | 153.407 ms | 201.125 ms | 218.605 ms | 227.038 ms |
| Toolbar | After | 712.555 ms | 159.014 ms | 206.701 ms | 223.486 ms | 232.418 ms |

Every integration scenario has a 20-second Tokio deadline. Process compile time
is separate; the parent coordinates cargo lifetimes. No measured sample timed
out. These two subsecond reads do not explain the archived hundreds-of-seconds
latencies. The scope deliberately excludes a seven-history or 100-trajectory
rerun, new model study, and universal long-history claims.

## Reproduce

From repository root (with pinned `target/c4-native/cohort/trajectories.json`):

```sh
cargo test -p memory_mcp --features eval-support --test long_history_stage_evidence -- --nocapture
python3 -m unittest evals.longmemeval_v2.test_long_history_diagnosis
python3 -m evals.longmemeval_v2.long_history_diagnosis \
  --cohort evals/longmemeval_v2/results/2026-11-26-stage-diagnosis/cohort.json \
  --trajectories target/c4-native/cohort/trajectories.json \
  --truth evals/longmemeval_v2/results/2026-11-26-stage-diagnosis/truth.json \
  --prompts evals/longmemeval_v2/results/2026-11-26-stage-diagnosis/post-prompts.jsonl \
  --readers evals/longmemeval_v2/results/2026-11-26-stage-diagnosis/readers.jsonl \
  --stage-receipts evals/longmemeval_v2/results/2026-11-26-stage-diagnosis/post-stage-receipts.json
```

The five frozen inputs plus optional stage receipts are hashed by the CLI.
Every supplied stage receipt must match the exact prompt-context digest;
source identity alone cannot attach a trace to a different read.
The final scoped validation is 6/6 Rust scenarios (adding a policy-isolated
source observation check) and 49 passing Python tests with 5 skips. The
additional access-policy and context-link guards each had observed RED → GREEN.
`validation.json` separates this final check from the retained paired capture.
`manifest.json` additionally hashes the source fixture and preserved captures.
No original archived artifact was modified. The pre-repair capture is historical;
current tests exercise the repaired implementation.
