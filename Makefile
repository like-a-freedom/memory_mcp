TEST_THREADS ?= 1
EVAL_CAPTURE ?= target/eval-baseline/latest.txt
EVAL_CAPTURE_PREV := $(EVAL_CAPTURE).prev

EVAL_RETRIEVAL = cargo test --test eval_retrieval run_retrieval_evals -- --ignored --exact --nocapture --test-threads=$(TEST_THREADS)
EVAL_LONGMEMEVAL = cargo test --test eval_external_retrieval run_longmemeval_retrieval -- --ignored --exact --nocapture --test-threads=$(TEST_THREADS)
EVAL_LOCOMO = cargo test --test eval_external_retrieval run_locomo_retrieval -- --ignored --exact --nocapture --test-threads=$(TEST_THREADS)
EVAL_EXTRACTION = cargo test --test eval_extraction run_extraction_evals -- --ignored --exact --nocapture --test-threads=$(TEST_THREADS)
EVAL_LATENCY = cargo test --test eval_latency run_latency_evals -- --ignored --exact --nocapture --test-threads=$(TEST_THREADS)
EVAL_CLAIMS = cargo test --test eval_claim_reconciliation run_claim_reconciliation_evals -- --ignored --exact --nocapture --test-threads=$(TEST_THREADS)
EVAL_AGENT_MEMORY_LIFECYCLE_SURFACE = cargo test --test eval_agent_memory_lifecycle public_surface_snapshot lifecycle_fixture_covers_core_risks -- --exact --nocapture

.PHONY: eval-baseline eval-quick eval-compare serve-release eval-ner-latency eval-ner-contention eval-claims eval-agent-memory-lifecycle-surface eval-pr

eval-baseline:
	@$(EVAL_RETRIEVAL)
	@$(EVAL_LONGMEMEVAL)
	@$(EVAL_LOCOMO)
	@$(EVAL_EXTRACTION)
	@$(EVAL_LATENCY)

eval-quick:
	@$(EVAL_RETRIEVAL)
	@$(EVAL_EXTRACTION)

eval-compare:
	@mkdir -p $(dir $(EVAL_CAPTURE))
	@if [ -f "$(EVAL_CAPTURE)" ]; then cp "$(EVAL_CAPTURE)" "$(EVAL_CAPTURE_PREV)"; fi
	@$(MAKE) --no-print-directory eval-baseline | tee "$(EVAL_CAPTURE)"
	@if [ -f "$(EVAL_CAPTURE_PREV)" ]; then \
		echo "== diff: $(EVAL_CAPTURE_PREV) vs $(EVAL_CAPTURE) =="; \
		diff -u "$(EVAL_CAPTURE_PREV)" "$(EVAL_CAPTURE)" || true; \
	else \
		echo "Saved first capture to $(EVAL_CAPTURE)"; \
	fi

serve-release:
	cargo run --release -- serve

eval-ner-latency:
	cargo test --locked --release --test eval_ner_latency run_gliner_latency_eval -- --ignored --exact --nocapture --test-threads=1

eval-ner-contention:
	cargo test --locked --release --test eval_ner_latency run_contention_eval -- --ignored --exact --nocapture --test-threads=1

eval-agent-memory-lifecycle-surface:
	@$(EVAL_AGENT_MEMORY_LIFECYCLE_SURFACE)

eval-claims:
	@$(EVAL_CLAIMS)

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
