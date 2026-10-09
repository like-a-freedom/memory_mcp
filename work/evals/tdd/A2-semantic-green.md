# A2 semantic evidence and eval-service GREEN

`rtk cargo test -p memory_mcp --locked --features eval-support,test-fixtures --test assemble_context_semantic_quality` passes 4 integration/component checks against an isolated in-memory SurrealDB with production migrations applied:

- non-finite and out-of-range similarity thresholds are rejected;
- multi-value namespace and unsupported claim stage are rejected;
- a valid factory constructs a service and `assemble_context` calls the injected provider;
- a provider dimension inconsistent with the persisted HNSW index is rejected by the real SurrealDB query path.

The separate `raw_similarity_survives_fusion` component test and fact-ID evidence-merge tests pass as part of the current `memory_mcp --lib` run recorded in the work log. These controlled providers are restricted to component/integration infrastructure; they are not accepted by the quality evaluation suite.
