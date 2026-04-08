TEST_THREADS ?= 1
EVAL_CAPTURE ?= target/eval-baseline/latest.txt
EVAL_CAPTURE_PREV := $(EVAL_CAPTURE).prev

EVAL_RETRIEVAL = cargo test --test eval_retrieval run_retrieval_evals -- --ignored --exact --nocapture --test-threads=$(TEST_THREADS)
EVAL_LONGMEMEVAL = cargo test --test eval_external_retrieval run_longmemeval_retrieval -- --ignored --exact --nocapture --test-threads=$(TEST_THREADS)
EVAL_LOCOMO = cargo test --test eval_external_retrieval run_locomo_retrieval -- --ignored --exact --nocapture --test-threads=$(TEST_THREADS)
EVAL_EXTRACTION = cargo test --test eval_extraction run_extraction_evals -- --ignored --exact --nocapture --test-threads=$(TEST_THREADS)
EVAL_LATENCY = cargo test --test eval_latency run_latency_evals -- --ignored --exact --nocapture --test-threads=$(TEST_THREADS)

.PHONY: eval-baseline eval-quick eval-compare

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
