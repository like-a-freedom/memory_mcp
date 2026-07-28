.PHONY: eval-pr eval-release eval-nightly serve-release

serve-release:
	cargo run --release -- serve

eval-pr:
	@mkdir -p target/evals
	cargo run -p eval-harness --bin memory-eval -- run \
		--profile evals/profiles/pr.json \
		--artifact target/evals/pr.json

eval-release:
	@mkdir -p target/evals
	cargo run -p eval-harness --bin memory-eval -- run \
		--profile evals/profiles/release.json \
		--artifact target/evals/release.json

eval-nightly:
	@mkdir -p target/evals
	cargo run -p eval-harness --bin memory-eval -- run \
		--profile evals/profiles/nightly.json \
		--artifact target/evals/nightly.json
