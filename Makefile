# Every target here is either invoked by a workflow or is the local
# equivalent of one. A target nothing runs is a target that rots: the profile
# it names stops matching what CI executes, and nothing notices.
#
#   eval-response-size   ci.yml `quality`, evaluations.yml
#   eval-ner-quality     evaluations.yml
#   bench-check          evaluations.yml (compiles every bench target)
#   bench-cpu-core       evaluations.yml (the Criterion run)
#   bench-cpu            the local form of `bench-cpu-core` plus `ner_cpu`
#   bench-metal          evaluations.yml `benchmark-metal`
#
# `eval-pr`, `eval-release`, `eval-nightly` and the four `eval-external-*`
# targets were one-line `cargo run` invocations that `evaluations.yml` already
# inlined, and `prepare-eval-corpora` had no caller at all. Run the profile
# directly:
#
#   cargo run -p eval-harness --bin memory-eval -- run \
#     --profile evals/profiles/<name>.json --artifact target/evals/<name>.json

.PHONY: eval-response-size eval-ner-quality bench-check bench-cpu bench-cpu-core bench-metal

eval-response-size:
	@mkdir -p target/evals
	cargo run -p eval-harness --bin memory-eval -- run \
		--profile evals/profiles/response_size.json \
		--artifact target/evals/response-size.json

eval-ner-quality:
	@mkdir -p target/evals
	cargo run -p eval-harness --bin memory-eval -- run \
		--profile evals/profiles/ner_quality.json \
		--artifact target/evals/ner-quality.json

bench-check:
	cargo bench -p eval-harness --no-run --locked --features eval-harness/bench

bench-cpu:
	$(MAKE) bench-cpu-core
	MEMORY_MCP_BENCH_REQUIRE_FIXTURES=1 cargo bench -p eval-harness --bench ner_cpu --locked --features eval-harness/bench

bench-cpu-core:
	cargo bench -p eval-harness --bench pipeline --locked --features eval-harness/bench
	cargo bench -p eval-harness --bench contention --locked --features eval-harness/bench

bench-metal:
	@if [ "$$(uname -s)" != "Darwin" ] || [ "$$(uname -m)" != "arm64" ]; then \
		echo "bench-metal requires macOS arm64 and local Metal/model assets" >&2; exit 2; \
	fi
	MEMORY_MCP_BENCH_REQUIRE_FIXTURES=1 cargo bench -p eval-harness --features memory_mcp/metal,eval-harness/bench --bench ner_metal --locked
