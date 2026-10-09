# B3 reconciliation invalidation RED

The worker regression test prepares claims through the ordinary deterministic projection path, leases a reconciliation job, and asserts that a durable page commit precedes cache invalidation.

Command: `rtk cargo test -p memory_mcp --locked --lib successful_background_reconciliation_invalidates_after_commit -- --exact --nocapture`

The first command used `--exact` without the full module path and ran zero tests; it is not counted as RED. The corrected command omitted `--exact` and failed to compile with E0061 because `run_next_leased_job` had no cache invalidator input. RED log SHA-256: `da08416f3e9103b35cd7ebffa87cced1796b55fdbb7031678d7cb68943514748`. Source hash at RED was not captured.

The inline projection race test uses bounded oneshot coordination: it warms the cache while projection is blocked, then requires the entry to be gone after projection completion.

Command: `rtk cargo test -p memory_mcp --locked --lib claim_projection_completion_clears_results_warmed_during_projection -- --nocapture`

Exit: 101. Expected E0425: `project_claims_then_invalidate_cache` did not yet exist. RED log SHA-256: `5501914d70aeb8720fe78d38d652958628a6d3085a6c633559961868c93393d9`. Source hash at RED was not captured.
