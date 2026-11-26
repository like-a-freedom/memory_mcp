# Frozen long-history stage diagnosis

2026-11-26; development diagnostic; no new held-out confirmation or general
long-history quality claim. All seven frozen November 26 contexts and the
paired concise-reader answers were analyzed without new reader/judge calls.

## Reproduce the archive analysis

From the repository root, with the original pinned staged trajectories present:

```sh
python3 -m evals.longmemeval_v2.long_history_diagnosis \
  --cohort evals/longmemeval_v2/small-web-text-cohort.json \
  --trajectories target/c4-native/cohort/trajectories.json \
  --truth evals/longmemeval_v2/results/2026-11-26-long-history-diagnosis/reviewed-truth.json \
  --prompts evals/longmemeval_v2/results/2026-11-26-current-master-repeat/run-01/prompt_rows.jsonl \
  --readers evals/longmemeval_v2/results/2026-11-26-current-master-repeat/run-01/scoring-replay-concise-v1/reader_outputs.jsonl
```

[archived-report.json](archived-report.json) records all five input SHA-256
values and paths. Source spans are checked against the previously frozen
cohort's reviewed bounds and hashes. Every saved reader's context digest must
match the actual prompt-row context before reader use can be localized.
[reviewed-truth.json](reviewed-truth.json) defines independent source-backed
answers and support markers from the three reviewed source states. These labels
come from complete source menus/lists/toolbars, not the reader's answer or an
upstream scorer pass. The diagnostic was developed after the frozen outputs
existed; it is not a preregistered confirmation test.

## What the seven cases establish

All 7/7 contexts are nonempty, 1,473–3,598 tokens, with archived truncation flags
false. Complete reviewed spans occur in 2/7. Exact registered answer contracts
pass 2/7, including 2/4 positive/list/count cases and 0/3 false-premise cases.
The original pinned scorer's 3/7 diagnostic replay includes the unsupported
extra “Past hour”; strict answer-set equality rejects it.

| Case | Reader answer | Complete source span | Supported interpretation |
| --- | --- | --- | --- |
| `0c6b0c60` | Past hour + five expected ranges | No | Incorrect extra; only Past 24 hours among required range markers is delivered. Formation/selection/packaging cause unknown. |
| `6d975a08` | Forums, Tags, Alphabetical | Yes | Correct list from delivered complete evidence. |
| `89a20f48` | Past year | No | False-premise failure; reviewed terminal All time menu evidence not delivered. Earlier cause unknown. |
| `922a0c56` | Send Email | No | False-premise failure; reviewed Reset/Create Order button markers absent. Earlier cause unknown. |
| `d6da3938` | Reset Password | No | Correct; Create Order and Reset Password markers delivered. Demonstrates full-span coverage's conservative false negative. |
| `f43926d9` | Forums | Yes | Reader-use failure: complete list explicitly has Tags immediately followed by Alphabetical, yet reader invents Forums between them. |
| `f61a096f` | two | No | Incorrect against nine source toolbar buttons; complete counting evidence not delivered. Earlier cause unknown. |

Only `f43926d9` isolates an observed failing stage: reader use with complete
reviewed evidence. Formation/extraction outputs, candidate-selection receipts,
and the prepackaging representation were not archived for these reads. All
three stages remain **unknown** for every case. Nonempty output and the false
truncation flag cannot show which earlier stage lost evidence. Marker matches
alone never establish semantic sufficiency or correct ordering.

The seven archived query durations have median **423.736693 s**, maximum
**991.328374 s**. These are query-level durations; no extraction, retrieval,
reranking or packaging latency decomposition is available. No archived timeout
was discarded. Official aggregate metrics remain unavailable after the
original judge error; this report only analyzes the preserved paired replay.

## Bounded public-path comparison

[public-comparator.py](public-comparator.py) invokes the existing
`MemoryMcpBackend.insert/query` public adapter on a temporary isolated store.
It publicly ingests/extracts the nine-line reviewed directory source, queries
it alone, adds two unrelated short histories, and repeats the same query.
The original full store is never opened or mutated. Each command has a 15 s
deadline, at most eight CLI calls; failure stops the experiment without a
successful retry being selected. No reader, judge, external provider or LM
Studio call is made. The existing binary, host architecture, exact history,
source hash, query and per-operation receipts are preserved.

The native arm64 run completed in **7.749 s**. Both query outputs contain the
complete reviewed span and all three browse names. Query wall times were
**0.237 s** alone and **0.215 s** with distractors; adapter aggregate query time
was 0.451 s (copy 0.014 s, CLI 0.432 s). Public formation calls took 6.430,
0.443 and 0.421 s. This is a three-source miniature using the frozen November 26
binary (`6a912216…d41`), not evidence that the current dirty source tree was
rebuilt, nor a benchmark of the hundred-trajectory frozen store.

[public-comparator-raw-run.json](public-comparator-raw-run.json) preserves the
first receipt, whose initial literal span measurement incorrectly missed JSON
newline/tab escaping. [public-comparator.json](public-comparator.json) corrects
only that measurement with the existing saved-text decoder, records the raw
receipt hash and retains identical raw outputs and timings. No re-query was
made. The script now applies that decoder directly. This correction illustrates
a measurement false negative, not product evidence loss.

The current debug product binary was then tested separately, preserving every
prior receipt. [public-comparator-current.json](public-comparator-current.json)
records the debug binary SHA-256 and supplied source revision
`9b13b9782b1e050c2cdfcacf26732119b1338f0c`. This run completed in **7.901 s**,
again returning the full reviewed span both alone and with two distractors.
Query wall times were **0.262 s** and **0.303 s**; public formation calls took
6.347, 0.464 and 0.521 s. Neither run timed out or errored. This second run is a
separate current-binary comparator, not selection of a successful retry after
a failure.

Reproduce the current comparator with a fresh output path (existing outputs
are refused):

```sh
python3 evals/longmemeval_v2/results/2026-11-26-long-history-diagnosis/public-comparator.py \
  --binary target/debug/memory_mcp \
  --binary-source-revision 9b13b9782b1e050c2cdfcacf26732119b1338f0c \
  --output /tmp/long-history-new-comparator.json
```

The bounded experiment did **not** reproduce distractor loss. It therefore
cannot identify formation, retrieval or packaging as the cause of the frozen
long-history failures, and supplies no long-history latency claim.

## Controls, limits and next actions

The public CLI tests cover wrong extra answers, valid reordered list output,
empty answers, absent reader observations, unsupported stage attribution when
all markers but no full span are present, source tampering, mismatched reader
context hashes, missing reader/context linkage, and invalid latency. The
implementation was developed through observed RED→GREEN slices for the new
CLI, reader-context identity, finite latency and missing identity behavior.

Phrase-set equality is a deliberately strict deterministic contract. English
count `nine` and digit `9` are explicitly accepted. Unlisted semantically valid
abstention paraphrases can be false negatives; this is not an autonomous
semantic judge. Full-span absence is also conservative: the correct Reset
Password case proves it is not an error verdict. The frozen false-premise
outputs are concrete invented labels, so this paraphrase limitation does not
alter their observed 0/3 result.

1. Address the observed reader-use failure with source-backed closed-list and
   adjacency reasoning; retest false-premise behavior independently of context
   availability. This diagnosis does not authorize a production repair.
2. Capture formation/extraction, selected-candidate and pre/post-packaging
   receipts with source identity and stage timings in the next bounded study.
   Without those observations, no extraction-versus-retrieval repair is
   justified by these artifacts.
3. Register any larger distractor comparison and resource budget separately.
   The tiny successful comparator does not qualify a larger confirmation run
   or change the prior C1 stop rule.
