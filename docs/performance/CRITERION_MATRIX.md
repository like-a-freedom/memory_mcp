# Criterion benchmark matrix

The benchmark commands are intentionally separate from correctness profiles.
Correctness profiles (`make eval-pr`, `make eval-release`, `make eval-nightly`,
`make eval-response-size`, and `make eval-ner-quality`) must pass before a
performance result is compared with a previous run.

| Target | Scope | Required inputs | Command |
|---|---|---|---|
| `pipeline` | ingest, extract, retrieval, metric overhead | embedded test DB | `make bench-cpu` |
| `contention` | 1/2/4 concurrent clients | embedded test DB | `make bench-cpu` |
| `ner_cpu` | regex, Anno, Anno-ONNX, GLiNER, VAGO | all local model fixtures | `MEMORY_MCP_BENCH_REQUIRE_FIXTURES=1 cargo bench -p eval-harness --bench ner_cpu` |
| `ner_metal` | Apple Silicon production path | macOS arm64 and local Metal/model assets | `make bench-metal` |

`make bench-check` only compiles targets. CI's scheduled `make bench-cpu`
sets `MEMORY_MCP_BENCH_REQUIRE_FIXTURES=1`; missing or failing model smoke
probes therefore fail the job rather than producing a green skipped result.
Record commit, profile, fixture revisions, host, and Criterion output under
the run artifact when publishing a comparison.
