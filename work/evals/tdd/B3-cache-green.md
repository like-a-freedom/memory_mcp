# B3 cache and invalidation GREEN

GREEN checks after implementation:

- `rtk cargo test -p memory_mcp --locked --lib memory::context_cache::tests -- --nocapture` — 10 passed.
- `rtk cargo test -p memory_mcp --locked --lib successful_background_reconciliation_invalidates_after_commit -- --nocapture` — 1 passed; the invalidator observed the durable commit first.
- `rtk cargo test -p memory_mcp --locked --lib claim_projection_completion_clears_results_warmed_during_projection -- --nocapture` — 1 passed; the cache remains populated while projection is blocked and clears after completion.
- `rtk cargo test -p memory_mcp --locked --features test-fixtures --test embedded_context_cache -- --nocapture --test-threads=1` — 5 passed, including both historical cutoff orders, warmed-empty ingestion, explicit invalidation, and cache hit stability.

These are unit and embedded-storage/service contract checks. They do not satisfy reviewed-corpus, provider-signature, or eval-harness release acceptance.
