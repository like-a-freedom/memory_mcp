# Typed conflict annotation probe (2026-11-24)

## Question and scope

Does the independent fixed-query scorer distinguish sufficient source
coverage from source coverage that also carries the preregistered typed
conflict annotation, at a one-item context budget?

This is a deterministic evaluator control on one newly authored synthetic
history. It is not an agent task, a powered cohort, a population estimate, or a
memory-treatment effect. No Codex invocation, model provider, API key, real
user content, or real memory was used.

## Frozen inputs and observed result

`history.json` contains two synthetic same-time capacity claims for Asterline
Freight Yard: an initial 18-tonne record and a retroactive correction to 12
tonnes. The preregistered `oracle.json` requires the correction event and, on
that returned event, `evidence_status: conflicting` plus the relation
`contradiction/conflict`. Both stage inputs use the same history, query,
snapshot and budget of one; only the registered `claim_rollout_stage` differs.

| Stage | Returned source | Required annotation present | Deterministic score |
| --- | --- | --- | --- |
| `evidence` | `asterline-correction` | Yes | `evidence_success: true` |
| `shadow` | `asterline-correction` | No | `evidence_success: false` |

The two public captures used separate fresh stores and the same memory product
binary, SHA-256
`3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`.
The evidence capture records the expected `conflicting` status and
`contradiction/conflict` relation. The shadow capture returns the same bound
source but no typed claim annotation. Thus this fixture exposes a measurable
direct-memory distinction at budget one, where a source-ID-only oracle would
have passed both arms.

## Reproduction

From the repository root, build `memory-eval` and use the checked-in synthetic
history, inputs and oracle. Each capture needs a new, nonexistent store path.

```sh
cargo build --locked -p eval-harness --bin memory-eval

cargo run --locked -p eval-harness --bin memory-eval -- capture-independent-probe --transport stdio \
  --input evals/paired/typed-evidence-probe-2026-11-24/evidence-input.json \
  --history evals/paired/typed-evidence-probe-2026-11-24/history.json \
  --server-binary /absolute/path/to/memory_mcp \
  --product-sha256 3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586 \
  --store /absolute/path/to/new-evidence-store \
  --claim-rollout-stage evidence \
  --output /absolute/path/to/evidence-capture.json
```

Repeat with `shadow-input.json`, a different new store path, stage `shadow`,
and a distinct output path. Extract each capture's `observation` object to a
standalone JSON file, then score offline:

```sh
cargo run --locked -p eval-harness --bin memory-eval -- score-independent-probe \
  --input evals/paired/typed-evidence-probe-2026-11-24/evidence-input.json \
  --oracle evals/paired/typed-evidence-probe-2026-11-24/oracle.json \
  --observation /absolute/path/to/evidence-observation.json \
  --product-sha256 3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586 \
  --output /absolute/path/to/evidence-score.json
```

Repeat the score command with the shadow input, observation and output. Offline
scoring needs no memory executable, Codex login, model, provider, or network.
The repository integration test `fixed_stdio_probe_scores_typed_conflict_under_a_one_item_budget`
runs capture and score through the public stdio interface with a synthetic
fixture; the adjacent negative case proves that missing typed annotation fails
even when source coverage passes. Capture v1 remains readable under its
historical source-only semantics; new captures use v2.

## Integrity

Product binary SHA-256:
`3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`.

Evaluator binary SHA-256 used for the final offline score replay:
`a51bb5da9d06e6b1c1f853e5b0eba9f0e3e3cb69fc16074399316e9fed227bfd`.

Input, oracle, captures, observations, initial scores and scores replayed by
the final evaluator are listed in `SHA256SUMS`. Capture receipts include raw
public-tool request/response and non-secret launch metadata. This fixture
contains synthetic facts only.

## Decision boundary

This result closes the evaluator gap where fixed probes could score a required
source without checking whether the same returned claim exposed its conflict
state. It validates one fixture, one context budget, and the current product
binary. It does not show that an agent notices the annotation, answers more
correctly, asks fewer repair questions, or saves time. Those outcomes require a
separate frozen agent study with attainable headroom, a justified user-impact
margin, ordinary-workflow/resource guards, disjoint families, and a joint power
analysis.
