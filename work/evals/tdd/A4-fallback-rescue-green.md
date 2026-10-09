# A4 GREEN evidence — episode fallback and first-person rescue

- rtk cargo test -p memory_mcp --locked --features eval-support --test assemble_context_semantic_quality — PASS, 7 tests. The strengthened episode case uses isolated embedded SurrealDB with production migrations and ingestion; it returns the grounded episode and excludes the one-keyword episode.
- rtk cargo test -p memory_mcp --locked --lib memory::retrieval::rescue::tests — PASS, 30 tests. The unadmitted first-person candidate is not appended; admitted rescue scenarios remain covered.
- rtk cargo test -p memory_mcp --locked --lib memory::retrieval::admission::tests — PASS, 20 tests. Shared lexical/reference policy and seed limit cases pass.

These are component/integration checks, not native-corpus retrieval-quality acceptance.
