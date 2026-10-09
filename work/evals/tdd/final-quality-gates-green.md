# Retrieval quality implementation — final local gates

Validated on the implementation worktree with the following commands:

```text
rtk cargo test -p memory_mcp --locked
2298 passed, 24 ignored (121 suites)

rtk cargo test -p eval-harness --locked
234 passed, 13 ignored (12 suites)

rtk cargo test -p memory_mcp --locked --features eval-support,test-fixtures --test assemble_context_semantic_quality -- --test-threads=1
8 passed

rtk cargo test -p memory_mcp --locked --test service_acceptance -- --test-threads=1
23 passed

rtk cargo test -p memory_mcp --locked --test service_integration -- --test-threads=1
51 passed

rtk cargo test -p memory_mcp --locked --lib temporal_relation_reads -- --nocapture
2 passed

rtk cargo test -p memory_mcp --locked --features test-fixtures --test assemble_context_reconciliation exposes_supersession_on_the_predecessor_item -- --exact --test-threads=1
1 passed

rtk cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
No issues found

rtk cargo fmt --all --check
passed

rtk git diff --check
passed
```

These component, embedded-store, integration, harness, format, and lint gates
are green. They do not establish native corpus quality: a source-reviewed,
frozen retrieval corpus with independently reviewed labels and matching
provider/index signatures is still required before claiming release acceptance.
The legacy 61-case diagnostic also retains four recall failures and is not
counted as an acceptance gate.
