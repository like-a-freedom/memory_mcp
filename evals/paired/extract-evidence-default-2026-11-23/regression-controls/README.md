# Current product regression controls — 2026-11-23

These controls were run after making `extract.include_fact_evidence` the
default. They verify current deterministic retrieval, claim reconciliation and
source-authority behavior; they do not estimate the effect of the response
change or prove general memory/agent improvement.

## Product and evaluator identity

- Repository base revision: `1ae1da4a4e3120f2de94753fc1a949caee3aced7`; the
  working tree was intentionally dirty and authorized for this work.
- Product executable SHA-256: `3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`.
- Evaluator executable SHA-256: `79a4a175153cf5c0b743f2d68c55b596a8645311d0547caaddb62b8494336716`.
- Rust: `rustc 1.100.0 (0ef1bc386 2026-11-09)`, macOS/aarch64.
- Source-authority v4 profile SHA-256:
  `f864d14d29a25f73785fbabb148f6ee5213a238677e156914a4cd4ca7502ddc8`.
- Retrieval profile digest: `b70944ad549141b0af63a11181e83cb75f0964e91adf380fb57512e8b729ce63`.
- Claim profile digest: `ddd441c1a4e2dfe82e932a2a39ce7429b5c4be154d833974e6881dcece87d433`.

The retrieval profile fingerprint binds the E5 model bundle, reranker bundle,
corpus, configuration, and index. The run used the local pinned
`intfloat/multilingual-e5-small` and mMARCO MiniLM bundles; it required no
network model provider, Codex executable, account, or API key.

## Results

| Control | Coverage | Result |
| --- | ---: | --- |
| Claim reconciliation | 41/41, 0 invalid | claim F1, precision and recall all 1.0000 |
| Retrieval quality | 37/37, 0 invalid | All six registered quality metrics 1.0000: precision (32 applicable), required-group recall (46), unknown abstention (5), answerable non-abstention (32), duplicate-free (37), prohibited-free (37) |
| Source authority v4, first capture | 4/4 primary cases | decision, evidence and independent probe success 4/4; temporal subscore 3/4 |
| Source authority v4, second capture | 4/4 primary cases | same scores as first capture |

The source-authority replay comparator accepted the pair as matching. Primary
case results and both temporal observations are retained in the two capture
directories. The 3/4 temporal subscore is visible and is not hidden by the 4/4
primary score. This fixed profile demonstrates reproducibility and these
fixture outcomes only; source-authority workflow usability and non-inferiority
remain open.

## Artifacts

- [`claims-41.json`](claims-41.json), SHA-256
  `bf8055d8c2ea78e50f6de14938f2e6a672a08e0c11a7b27c96f5a51e3fdd675a`
- [`retrieval-quality-37.json`](retrieval-quality-37.json), SHA-256
  `27ce93ac7d6975356da3178aa62f5bc364abd40ae034a2a0668d827d2b7d9d3b`
- [`source-authority-first/index.json`](source-authority-first/index.json), SHA-256
  `0a8d6f1e85e179f9064101c14b672092779ad9d19fa85d748a129fe5a090d6ca`
- [`source-authority-second/index.json`](source-authority-second/index.json), SHA-256
  `2f8ea64d6205e5dd7fd6ced3d64abcf0e3bce2c716ee573b59183b0648397cce`

Each source-authority index binds the same product binary, input schema,
corpus and config hashes. Per-case receipts are included beside each index.

## Reproduction

Build the product and evaluator from the checkout, then run from the repository
root. Set a fresh artifact root per source-authority capture.

```sh
MEMORY_DECISIONS_SERVER_BIN="$PWD/target/debug/memory_mcp" \
MEMORY_DECISIONS_PRODUCT_REVISION="dirty-default-evidence-2026-11-23" \
MEMORY_DECISIONS_ARTIFACT_ROOT="/tmp/memory-source-authority-first" \
cargo run -p eval-harness --locked -- run \
  --profile evals/profiles/memory_decisions_source_authority_v4.json \
  --artifact /tmp/memory-source-authority-first.json

MEMORY_DECISIONS_SERVER_BIN="$PWD/target/debug/memory_mcp" \
MEMORY_DECISIONS_PRODUCT_REVISION="dirty-default-evidence-2026-11-23" \
MEMORY_DECISIONS_ARTIFACT_ROOT="/tmp/memory-source-authority-second" \
cargo run -p eval-harness --locked -- run \
  --profile evals/profiles/memory_decisions_source_authority_v4.json \
  --artifact /tmp/memory-source-authority-second.json

cargo run -p eval-harness --locked -- compare-decision-replays \
  --left /tmp/memory-source-authority-first/index.json \
  --right /tmp/memory-source-authority-second/index.json
```

The response-default-specific contract and fixed-query replay are documented
in the parent [`extract-evidence-default report`](../README.md). The broader
claim-stage comparison is preserved there separately and uses a different
product executable identity.
