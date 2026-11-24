# Ingestion source-authority evaluation archive

This archive separates the historical one-case source-substitution baseline
from the four-case v3 migration profile. Runs use fresh stores, the public MCP
path, a fixed corpus and deterministic scoring; they need no Codex executable,
model account, API key or network request.

## Historical one-case baseline

`evals/profiles/memory_decisions_source_authority.json` stores an existing local
file path as literal `ingest.content`. The fixture contents do not contain the
path. The oracle requires the exact reference bound to the original event.
Before the literal-content change, two runs on binary
`e9c15dc133e38252064e9814e35aa8af49f65f828b10c173f56857993aae5034` both
completed validly with 0/1 evidence and decision success. Their normalized
replays matched. This identifies the historical source-reference substitution
defect; it is not a broad retrieval estimate or security certification.

The original checkout had `HEAD=1ae1da4a4e3120f2de94753fc1a949caee3aced7`
and uncommitted work. The binary hash, not the Git revision label, identifies
the tested build. Original reports and receipts remain `baseline-a*` and
`baseline-b*`.

## Four-case v3 profile — temporally invalid for ordinary recall

The registered v3 profile adds literal path and URL recall, an ordinary fact
control, and an unknown-reference abstention control. Its path and URL events
use `t_ref=2026-11-22T12:00:00Z`, while ordinary recall ran around 06:16 UTC.
They were future-dated and correctly excluded from primary recall. The
historical v3 score of 2/4 is therefore not evidence that both retrieval
behaviors were broken. V3 artifacts remain unchanged; see the corrected v4
replay for a valid retrieval comparison.

The final post-lineage candidate used binary
`5ddf5f8db2a4ce74c8093a29b827f2c60fc5eaf2f2f470920bf2ba0be05b2218` and was
captured twice with distinct fresh stores. Both runs were valid, scored 2/4 on
the primary channel and had matching normalized replay indexes:

| Case | Result |
|---|---|
| Literal path preservation | Failed: missing evidence / incorrect disposition |
| Literal URL preservation | Failed: missing evidence / incorrect disposition |
| Ordinary fact control | Passed |
| Unknown-reference control | Passed |

The separate temporal-revision diagnostic scored 3/4 decisions and 4/4
evidence, but asks at a different cutoff and cannot repair the primary
profile's time mismatch. No memory-quality or agent-usability claim is made
from v3.

Current reports and receipts are `candidate-final-a.json` plus
`candidate-final-a/`, and `candidate-final-b.json` plus
`candidate-final-b/`. The earlier `candidate-v3-*` artifacts retain their own
binary identity and results.

## Final default-feature binary replay

After the full product package test rebuilt the default-feature executable,
the v3 profile was repeated twice on the final source state. Both captures
record product binary SHA-256
`976c4562dfeedf8f187f2c88442b397a6bd5ec849b397057c1ee12e94e6ad66c`, use four
registered cases with no invalid outcomes, and match under
`memory-eval compare-decision-replays`:

| Case | Result |
|---|---|
| Literal path preservation | Failed: missing evidence / incorrect disposition |
| Literal URL preservation | Failed: missing evidence / incorrect disposition |
| Ordinary fact control | Passed |
| Unknown-reference control | Passed |

Both runs score 2/4 on the primary channel. The temporal diagnostic remains
3/4 decisions and 4/4 evidence. The raw reference is still persisted
literally, but ordinary primary context assembly does not reliably retrieve
it. Report SHA-256 values are `526b52714d9395011f43c438eeb14ad812e66f7939564a5eabb1dc9b5504819d`
for `candidate-final-build-a.json` and
`ae915bc0634099520f9d429d5822f09de3d18c09d4f26a96a6ec9bc94b4a927b` for
`candidate-final-build-b.json`; raw receipts and indexes are retained next to
each report. This closes reproducibility for the latest default-feature
build. Its 2/4 source-reference result is temporally invalid for ordinary
recall; the corrected v4 profile supersedes it for current retrieval diagnosis.

Separate deterministic product tests verify literal storage for an existing
path and a loopback URL, high-confidence credential refusal before persistence,
root-confined file imports, symlink/traversal rejection, the 4 MiB size limit,
and URL credential-query refusal before network access. The corrected v4
replay is documented separately. Later stdio and HTTP process regressions
cover literal path/URL ingest and recall, no loopback fetch, credential
refusal, and representative diagnostic redaction; hook forwarding and watcher
snapshot preparation have separate boundary tests. Remaining gates are
mid-extraction partial-write behavior, controlled redirect refusal, broader
diagnostic coverage, and legitimate-workflow non-inferiority.

## Reproduction

From the repository root, build both binaries, set
`MEMORY_DECISIONS_SERVER_BIN`, `MEMORY_DECISIONS_PRODUCT_REVISION`, and a fresh
`MEMORY_DECISIONS_ARTIFACT_ROOT` for each capture, then run
`evals/profiles/memory_decisions_source_authority_v3.json`. Allow exit status 1
for a completed quality-failed diagnostic; status greater than 1 is an
execution failure. Compare each pair of `index.json` files with
`memory-eval compare-decision-replays`.
