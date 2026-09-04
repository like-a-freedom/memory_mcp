# HTTP Hobby Validation

`memory_mcp_http` is maintained as a single-user hobby project. It does not
claim SaaS certification, a concurrent-tenant SLA, or a disaster-recovery
objective.

Before merging a meaningful HTTP change, run:

```bash
cargo fmt --all --check
cargo test -p memory_mcp --locked
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http,control-plane --locked -- -D warnings
```

Changes to HTTP concurrency or tenant isolation additionally run the 20-tenant
in-memory regression:

```bash
cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_load_concurrency load_20_active_tenants_under_expected_qps -- --test-threads=1
```

The commit SHA and CI logs are sufficient evidence. The project does not
generate or commit a separate release gate matrix.

Proxy, SDK interoperability, restore, credential rotation, and capacity
checks become required only before a shared remote deployment is exposed to
other users. Define the workload and recovery objective first.
