# A2 semantic evidence and eval-service final local GREEN

- rtk cargo test -p memory_mcp --locked --features eval-support,test-fixtures --test assemble_context_semantic_quality -- --test-threads=1: 8 passed.
- rtk cargo test -p memory_mcp --locked --lib ann_threshold_rejects_0699_and_admits_exactly_0700: 1 passed. Production ANN retrieval against the embedded store rejects 0.699 and admits the configured 0.700 boundary.
- rtk cargo test -p memory_mcp --locked --features eval-support,test-fixtures --test assemble_context_semantic_quality provider_outage_keeps_the_lexical_retrieval_path_available -- --exact --test-threads=1: 1 passed. A controlled provider failure is exercised through production assembly and a grounded lexical fact remains available.
- rtk cargo test -p memory_mcp --locked --lib: included in the final quality gate artifact.
- These are deterministic component and embedded-store checks. The controlled provider tests do not count as model-quality acceptance.
