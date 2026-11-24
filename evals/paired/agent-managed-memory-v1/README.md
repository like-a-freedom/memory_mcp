# Agent-managed memory feasibility pilot

This archive measures whether the optional paired runner can execute the
formation → fresh-session recall → independent-probe protocol under its
registered resource limits. It is a development/feasibility study, not a
candidate promotion test, memory treatment comparison, usability study, or
population estimate.

## Frozen protocol and identities

The v5 manifest uses eight disjoint synthetic one-fact histories. Each arm and
family gets a fresh isolated store. Formation enables only `ingest` and
`extract`, with one call to each; it cannot read memory or invalidate facts.
Recall uses a new Codex process with `assemble_context` only, then the evaluator
runs a fixed public MCP probe against the same store. The oracle stays in the
evaluator. Receipts retain hashes and aggregate measurements, not source text.
Read-only is still the default Codex profile.

V5 was preregistered in [`plan.json`](plan.json),
[`tasks-v5.json`](tasks-v5.json), and
[`preregistration.md`](preregistration.md). The immutable v4 files are
[`plan-v4.json`](plan-v4.json),
[`fingerprints-v4.json`](fingerprints-v4.json), and
[`preregistration-v4.md`](preregistration-v4.md). Verify the plan, manifests,
policies and prompt files from this directory with:

```sh
shasum -a 256 -c SHA256SUMS.txt
```

V5 pinned the Codex CLI SHA-256, evaluator binary SHA-256, product binary
SHA-256, prompt/policy/protocol hashes, and canonical task/oracle fingerprints
in [`fingerprints.json`](fingerprints.json). The memory product SHA is
`cc0e08b7b1327a515677b756faebefcd27fd7c7881d888bf150eba1677de82da` in both
arms and both repeats. Codex CLI was `0.171.0`, using the existing local
ChatGPT login through `--codex-home`; the run did not require `CODEX_API_KEY`.
The captured evaluator binary SHA was
`054b396c2a7917e73ed2ca6b016ecb8e92b9da07024896276c72c8fe680a2d0f`.

## Results

| Measure | V4, two repeats | V5, two repeats |
|---|---:|---:|
| Formation phases completed | 0/32 | 32/32 |
| Mean formation input tokens per trial | 77,480 | 38,770 |
| Maximum formation input tokens | 80,833 | 38,827 |
| Successful ingests / extracts | 64 / 64, each duplicated | 32 / 32, exactly once |
| Returned episode IDs mapped | 0/32 | 32/32 |
| Independent probes supported | 0/32 | 32/32 |
| Task success | 0/32 | 32/32 |
| Unsupported or prohibited probe sources | 32 unsupported, 0 prohibited | 0 unsupported, 0 prohibited |
| Hard violations | 32 | 0 |

V4's read/write profile exposed `assemble_context`, `ingest`, `extract`, and
`invalidate` to formation. The agent repeatedly read and wrote the same fact;
the formation phase exceeded its 65,536-token input budget in every trial, and
the Codex MCP result's JSON text wrapper was not parsed for its episode ID.
Those two independent harness defects explain why no v4 probe could credit a
write. V4's full case-level artifacts remain under
[`v4-repeat-01/`](v4-repeat-01/) and
[`v4-repeat-02/`](v4-repeat-02/).

V5 narrowed the write-phase tool surface and prompt, and taught the runner to
parse only the opaque episode ID from the JSON text wrapper. Across the two
fresh invocations, all 32 formation phases completed; there were exactly 32
successful `ingest` calls and 32 successful `extract` calls, one each per
family trial. All 32 IDs were linked to the hidden task event, and every
independent probe found its registered source. There were no unsupported or
prohibited sources, hard violations, or formation budget exits. Formation
input averaged 38,770 tokens per trial and peaked at 38,827 against the
65,536-token ceiling. Formation wall time averaged 19.3 seconds per trial.
Each isolated store retained about 34 MB after its one-fact setup; this is
measured store size, not process memory consumption.

The same-product task/direct-memory outcomes matched on both repeats. The
registered `summarize-pilot` output is
[`reliability-v5.json`](reliability-v5.json); both `compare-agent-managed`
reports are in the run directories. Re-running both comparators and the pilot
summary from the frozen inputs produced byte-identical outputs.

## Interpretation and limits

V5 demonstrates that the runner can perform one controlled write, capture its
episode ID, start a fresh reader, and independently verify source support
without exceeding its registered budget on these eight synthetic cases. It
does not compare different product builds: incumbent and candidate are the
same binary. Therefore the 0/8 → 8/8 change from v4 to v5 is attributable to
the evaluation protocol and harness fixes, not an improvement to memory
product behavior.

The cohort is small and synthetic; Codex alias resolution may change between
invocations; the study uses one initial fact per task. It does not measure
maintenance of existing memories, correction/conflict handling, long-running
store growth, ordinary operator effort, real workflow non-inferiority, or agent
productivity. Keep maintenance and powered agent-benefit studies separate.

## Deterministic report replay

The offline comparator and pilot summary can be regenerated from the archived
artifacts. Their output is byte-identical to the checked-in reports:

```sh
cargo build --locked -p eval-harness --bin memory-eval

target/debug/memory-eval compare-agent-managed \
  --plan evals/paired/agent-managed-memory-v1/plan.json \
  --incumbent evals/paired/agent-managed-memory-v1/v5-repeat-01/incumbent.json \
  --candidate evals/paired/agent-managed-memory-v1/v5-repeat-01/candidate.json \
  --incumbent-stages evals/paired/agent-managed-memory-v1/v5-repeat-01/agent-managed-incumbent.json \
  --candidate-stages evals/paired/agent-managed-memory-v1/v5-repeat-01/agent-managed-candidate.json \
  --output /absolute/path/to/new-v5-comparison.json

target/debug/memory-eval summarize-pilot \
  --plan evals/paired/agent-managed-memory-v1/plan.json \
  --first-run-directory evals/paired/agent-managed-memory-v1/v5-repeat-01 \
  --second-run-directory evals/paired/agent-managed-memory-v1/v5-repeat-02 \
  --output /absolute/path/to/new-v5-reliability.json
```

The live runs used the frozen evaluator binary SHA listed above. A later,
behavior-preserving Clippy cleanup changed the current rebuilt evaluator SHA to
`749a0d8b6f9d043211c06136fef3133305497c3822e404b7047c8714b6c0daf4`; the
current binary replays both comparisons and the summary byte-for-byte, but the
v5 live-run preflight correctly rejects it against the frozen evaluator
fingerprint. Any new live agent captures require a new plan ID and fingerprints.
