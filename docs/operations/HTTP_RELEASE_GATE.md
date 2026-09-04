# HTTP Release Gate

The release gate for the HTTP SaaS profile combines an automated
local pass (run in CI on every push) with an external pass (run
before tagging a release) and produces a single release decision.

The matrix rows are captured per-run by
[`scripts/http_release_evidence.sh`](../../scripts/http_release_evidence.sh);
the resulting `target/http-release-evidence/<ts>/gates.tsv` is the
authoritative gate matrix for that run.

## Automated local gates

These run on every push via CI. They exercise the in-repo
conformance fixtures against an embedded SurrealDB instance and do
not require an external environment.

| Gate | Command | Commit | Timestamp | Environment | Result | Evidence path |
|---|---|---|---|---|---|---|
| fmt | `cargo fmt --all --check` | | | local | Not executed — release blocked | |
| clippy | `cargo clippy -p memory_mcp --all-targets --features fs-watch,mcp-apps,streamable-http,control-plane,prometheus,test-fixtures --locked -- -D warnings` | | | local | Not executed — release blocked | |
| http_proto_conformance | `cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_proto_conformance -- --test-threads=1` | | | local | Not executed — release blocked | |
| http_isolation | `cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_isolation -- --test-threads=1` | | | local | Not executed — release blocked | |
| http_proxy_streaming | `cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_proxy_streaming -- --test-threads=1` | | | local | Not executed — release blocked | |
| http_control_plane | `cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_control_plane -- --test-threads=1` | | | local | Not executed — release blocked | |
| http_crash_recovery | `cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_crash_recovery -- --test-threads=1` | | | local | Not executed — release blocked | |
| http_durable_tasks | `cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_durable_tasks -- --test-threads=1` | | | local | Not executed — release blocked | |
| http_subscription_replica | `cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_subscription_replica -- --test-threads=1` | | | local | Not executed — release blocked | |
| http_load_concurrency | `cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_load_concurrency -- --test-threads=1` | | | local | Not executed — release blocked | |
| http_registry_storage | `cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_registry_storage -- --test-threads=1` | | | local | Not executed — release blocked | |

## External environment gates

These run before tagging a release. Each requires a specific
environment variable to enable; when the variable is unset the
script records `not_executed` and exits nonzero in `release` mode.

| Gate | Command | Commit | Timestamp | Environment | Result | Evidence path |
|---|---|---|---|---|---|---|
| http_proxy_streaming_proxy_gate | `MEMORY_MCP_TEST_PROXY_BIN=<proxy> cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_proxy_streaming http_proxy_streaming_proxy_gate -- --test-threads=1 --ignored` | | | MEMORY_MCP_HTTP_PROXY_BIN | Not executed — release blocked | |
| http_load_concurrency_500 | `MEMORY_MCP_HTTP_500_TENANT=1 cargo test -p memory_mcp --features streamable-http,mcp-apps,control-plane,test-fixtures --test http_load_concurrency load_500_tenants_under_contingency_qps -- --test-threads=1 --ignored` | | | MEMORY_MCP_HTTP_500_TENANT | Not executed — release blocked | |
| http_interop_matrix_clients | `<interop-clients-dir>/run.sh --manifest docs/operations/HTTP_INTEROP_MATRIX.md` | | | MEMORY_MCP_HTTP_INTEROP_CLIENTS_DIR | Not executed — release blocked | |
| restore_drill | `scripts/restore_drill.sh <target-db>` | | | MEMORY_MCP_HTTP_RESTORE_DRILL_DB | Not executed — release blocked | |
| credential_rotation | `scripts/credential_rotation.sh <target-deployment>` | | | MEMORY_MCP_HTTP_CREDENTIAL_ROTATION_TARGET | Not executed — release blocked | |

## Release decision

The release can be tagged when:

1. Every row in the **Automated local gates** section has
   `Result = Pass`. The CI run enforces this and rejects a push
   that fails any local gate.
2. Every row in the **External environment gates** section has
   `Result = Pass`. The evidence script enforces this in
   `release` mode: it exits nonzero if any external row is
   `not_executed` or `Fail`.

Concretely:

```bash
# Local: run all gates that can be exercised on a laptop.
scripts/http_release_evidence.sh local

# Release: same matrix, but external gates must show evidence.
# The script exits 1 if any external gate is `not_executed`.
scripts/http_release_evidence.sh release
```

The most recent run's `target/http-release-evidence/<ts>/gates.tsv`
is the canonical record for the release decision. Commit the TSV
into the release tag's evidence bundle.