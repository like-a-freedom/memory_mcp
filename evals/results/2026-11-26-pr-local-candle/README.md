# Local `pr` profile run — 2026-11-26

## Result

Run `run-1795646460` started at `2026-11-25T22:41:00.318943Z` on macOS
Apple Silicon and finished in 223.344 seconds. The registered `pr` profile
passed: 150/150 cases, 0 failed, 0 invalid, all 10 gates passed, and the
600-second budget passed.

| Suite | Passed | Result |
| --- | ---: | --- |
| Local retrieval | 61/61 | Legacy-v1 MRR@budget 1.0; required-group recall@budget 1.0 across 71 groups; top-1 hit rate@budget 1.0. Diagnostic metrics. |
| Extraction | 9/9 | Entity F1 0.75; precision 0.60; recall 1.0. |
| Claim reconciliation | 41/41 | Precision, recall and F1 all 1.0. |
| External mini retrieval | 2/2 | Answer-presence proxy@5 1.0; this is not answer correctness. |
| Retrieval quality | 37/37 | All six configured floors 1.0. Applicable denominators: precision 32, required groups 46, unknown abstention 5, answerable non-abstention 32, duplicate-free 37, prohibited-free 37. |

The run used the pinned E5 and mMARCO bundles through the native Rust
`local-candle` provider. It did not use LM Studio, an agent runner, a hosted
model, or a model API key. `run.json` records the E5 file hashes and the
reranking signature; the tracked manifests in `evals/models/` record both
models' revisions and file hashes.

Two earlier attempts are retained as execution preflights, not quality
results. `preflight-provider-unavailable.json` has 113 earlier-suite passes
and 37 invalid retrieval-quality cases because no native embedding provider
was configured. `preflight-stage-mismatch.json` has the same invalid-case
pattern because its runtime claim stage was `disabled` while the prepared
retrieval corpus required `evidence`. Neither preflight is counted in the
accepted 150-case result; neither records a product-quality failure.

## Baseline limits

The registered PR baseline is `run-1788548898` from 2026-09-04 at commit
`0b1d1f1bcde9c95bf0ed3c75b8cb47922d1b552a`. It contains 113 cases across the
four earlier suites. This run contains 150 cases because the current profile
adds 37 retrieval-quality cases.

Only three of the current profile's 10 gates had matching baseline values:
entity F1, claim precision, and claim recall. All three matched their baseline
values exactly. The external answer-presence gate and all six retrieval-quality
metric gates had no baseline value and passed their configured hard floors.
This is a successful current-profile run with fixed-fixture quality gates; it is
**not** a 150-case before/after comparison or a full-cohort no-regression
claim. The 37 retrieval-quality cases are development regressions, not held-out
accuracy estimates.

## Reproduction

From the repository root, prepare the pinned bundles using the commands in
[`docs/evals/README.md`](../../../docs/evals/README.md#reproduce-the-native-pr-profile-locally).
Set the listed provider variables, including
`MEMORY_CLAIM_ROLLOUT_STAGE=evidence`, and run:

```sh
mkdir -p target/evals
cargo run -p eval-harness --locked --bin memory-eval -- run \
  --profile evals/profiles/pr.json \
  --baseline evals/baselines/one-active-namespace-pr.json \
  --artifact target/evals/pr.json
```

The profile, model manifests, corpus and baseline are tracked inputs. The
original run used the verified bundles under `work/evals/models`; a fresh run
may use the `target/eval-models` cache shown in the reproduction instructions.

## Artifact identity

- `run.json` SHA-256: `0a919f31aba342984c9208e7c93b49055ddf425af3ca376e49560308ee151119`
- `preflight-provider-unavailable.json` SHA-256: `4f2a2e9129be4b041c2c4674176b6295fe5b0343f27fa49ce0989017e072b8f5`
- `preflight-stage-mismatch.json` SHA-256: `b3c16ee4096fc8a1243fb8e634dff210b079ca16561c0b16c5128548f6a62fc1`
- PR baseline SHA-256: `8d929b53ff625b7b15563b7ac3e73e08ee9d211e076d5a22a523f68c20ace3bc`
- Profile digest: `774f2f4b836b060a645a77c04acdc0f5a405b7b6a643c3758162b74698618bd0`
- Retrieval-quality corpus: `retrieval-quality-v1/r27-upstream-session-order`, manifest SHA-256 `ec720673f9dc0ea2f77f278ca3224a4672fea10f2af812ef8a5b9ca945363079`
- Evaluator commit: `cfd54cd64b9798a55cfbf4d54b140dd51133c456`; the working tree had uncommitted changes.
- Tracked Rust production-source patch SHA-256 at capture: `64096cfe1f5503fe0bcb94d96b6334e5d7fd4887973bdfa8431364dde4756323` (`git diff --binary HEAD -- crates/eval-harness/src crates/memory-mcp/src`; test files excluded).
- Host fingerprint: `macos/aarch64`; rustc `1.100.0 (0ef1bc386 2026-11-09)`.

The source patch digest is an integrity check, not a reconstructible source
snapshot. The run used modified Rust sources; `run.json`'s `git_commit` field
identifies HEAD, and the patch itself is not archived here. The regression test
was strengthened after this run and is not part of the captured binary source.
Reproducing the exact captured source state therefore requires the
corresponding working-tree changes. The measurement is local evidence for this
profile and fixture set; it makes no production-population or
memory-treatment-effect claim.
