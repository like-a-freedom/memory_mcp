# `extract` default-evidence deterministic check

**Purpose:** verify the bounded `extract.include_fact_evidence` default and
check that changing this response default does not change a frozen retrieval
result. This is a product-contract and narrow no-regression check; it is not an
agent-utility or stored-memory-quality study.

## Product-path preflight

The operator-import preflight used the default-on product binary with eight
synthetic text fixtures. Each case ran the real CLI file import and extraction,
then an independent `assemble_context` query against the same isolated store.
The content-free case report is stored at
[`operator-import-probes-v1.json`](../../paired/source-authority-agent-workflow-v1/operator-import-probes-v1.json).

| Check | Result |
| --- | ---: |
| Imports succeeded | 8/8 |
| Extractions succeeded | 8/8 |
| Complete default evidence pairs | 8/8 |
| Independent probes found the authorized source | 8/8 |
| Independent probes supported the expected value | 8/8 |
| Largest serialized evidence addition | 139 bytes |
| Largest serialized extraction result | 1,023 bytes |

The fixture profile disables embeddings and uses the deterministic regex
extractor. These synthetic cases verify the public product path for the
registered inputs; they do not measure arbitrary-language extraction quality.

## Frozen retrieval before/after check

The same stored history and request were captured twice with the pre-default
product binary and twice with the default-on binary. The request fingerprint
is `8a99661eb5a8c2fd8145269d180cb0626f019bee07d15c07af81acd916a874ac`.
All four captures completed, used one result, and returned `source-a`. Their
normalized result fingerprint was identical:
`ce075fc5ca05494a0a0757ffe130f3ea62688185e0647b1678a3f3baf262bd1d`.

The tool schema changed as expected because the parameter description/default
changed; it remains an eight-tool MCP surface. The before/after product and
schema hashes, input hash, repeat outcomes, and normalized semantics are in
[`repeatability.json`](repeatability.json). Raw captures remain alongside it
for independent inspection.

## Limits

- The fixed retrieval check shows unchanged semantics for one frozen history,
  query, and runtime profile. It is not a broad retrieval-quality benchmark.
- The import preflight measures evidence availability and source binding, not
  agent task success.
- The response-only default does not change stored facts or retrieval
  algorithms; no stored-memory quality gain is claimed.
- The earlier v6 agent-maintenance all-correct gate remains **NO-GO** for
  agent-workflow promotion. It is not overwritten by these product checks.
- Third-party callers that reject additive response fields should pass
  `include_fact_evidence=false`; the CLI provides `--compact`.
