# Criterion benchmark matrix

The benchmark commands are intentionally separate from correctness profiles.
Run PR, release, and nightly correctness profiles with `memory-eval run`; PR
and release require their checked-in regression baselines. The smaller
`make eval-response-size` and `make eval-ner-quality` targets remain available.
All applicable correctness profiles must pass before comparing a performance
result with a previous run. See the Evaluation section in `README.md` for the
exact profile commands and baseline paths.

| Target | Scope | Required inputs | Command |
|---|---|---|---|
| `pipeline` | ingest, extract, retrieval, metric overhead | embedded test DB | `make bench-cpu` |
| `contention` | 1/2/4 concurrent clients | embedded test DB | `make bench-cpu` |
| `ner_cpu` | regex, Anno, Anno-ONNX, GLiNER, VAGO | all local model fixtures | `MEMORY_MCP_BENCH_REQUIRE_FIXTURES=1 cargo bench -p eval-harness --features eval-harness/bench --bench ner_cpu` |
| `ner_metal` | Apple Silicon production path | macOS arm64 and local Metal/model assets | `make bench-metal` |

`make bench-check` only compiles targets. CI's scheduled `make bench-cpu`
sets `MEMORY_MCP_BENCH_REQUIRE_FIXTURES=1`; missing or failing model smoke
probes therefore fail the job rather than producing a green skipped result.
Record commit, profile, fixture revisions, host, and Criterion output under
the run artifact when publishing a comparison.
