.PHONY: eval-pr eval-release eval-nightly eval-response-size eval-ner-quality eval-external-longmemeval eval-external-locomo eval-external-personamem eval-external-prefeval prepare-eval-corpora bench-check bench-cpu bench-cpu-core bench-metal serve-release

serve-release:
	cargo run --release --features fs-watch -- serve

eval-pr:
	@mkdir -p target/evals
	cargo run -p eval-harness --bin memory-eval -- run \
		--profile evals/profiles/pr.json \
		--artifact target/evals/pr.json \
		--baseline evals/baselines/one-active-namespace-pr.json

eval-release:
	@mkdir -p target/evals
	cargo run -p eval-harness --bin memory-eval -- run \
		--profile evals/profiles/release.json \
		--artifact target/evals/release.json \
		--baseline evals/baselines/one-active-namespace-release.json

eval-nightly:
	@mkdir -p target/evals
	cargo run -p eval-harness --bin memory-eval -- run \
		--profile evals/profiles/nightly.json \
		--artifact target/evals/nightly.json

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

prepare-eval-corpora:
	@mkdir -p target/eval-corpora
	for manifest in evals/corpora/longmemeval.json evals/corpora/locomo.json evals/corpora/personamem.json evals/corpora/prefeval.json; do \
		cargo run -p eval-harness --bin memory-eval -- prepare-corpus --manifest "$$manifest" --output-root target/eval-corpora || exit $$?; \
	done

eval-external-longmemeval:
	@mkdir -p target/evals
	cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/external_longmemeval.json --artifact target/evals/external-longmemeval.json

eval-external-locomo:
	@mkdir -p target/evals
	cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/external_locomo.json --artifact target/evals/external-locomo.json

eval-external-personamem:
	@mkdir -p target/evals
	cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/external_personamem.json --artifact target/evals/external-personamem.json

eval-external-prefeval:
	@mkdir -p target/evals
	cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/external_prefeval.json --artifact target/evals/external-prefeval.json

bench-check:
	cargo bench -p eval-harness --no-run --locked

bench-cpu:
	$(MAKE) bench-cpu-core
	MEMORY_MCP_BENCH_REQUIRE_FIXTURES=1 cargo bench -p eval-harness --bench ner_cpu --locked

bench-cpu-core:
	cargo bench -p eval-harness --bench pipeline --locked
	cargo bench -p eval-harness --bench contention --locked

bench-metal:
	@if [ "$$(uname -s)" != "Darwin" ] || [ "$$(uname -m)" != "arm64" ]; then \
		echo "bench-metal requires macOS arm64 and local Metal/model assets" >&2; exit 2; \
	fi
	MEMORY_MCP_BENCH_REQUIRE_FIXTURES=1 cargo bench -p eval-harness --features memory_mcp/metal --bench ner_metal --locked
