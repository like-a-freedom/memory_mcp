# Changelog

All notable changes to this project will be documented in this file.

## [1.40.0] - 2026-11-18

### Changed
- Source integrations that configure SurrealDB directly must replace the removed `kv-rocksdb` feature and `RocksDb` type with `kv-surrealkv` and `SurrealKv`.
- Existing embedded v3 stores require a one-way conversion. Stop all writers and make a cold backup of every distinct local store. Verify the backup by restoring it with v1.39.0 on a disposable copy. Before starting v1.40.0, run the maintenance command with `SURREAL_EXPERIMENTAL_UPGRADE_FROM_3X=true`:

  ```bash
  # Stdio primary store
  env SURREAL_EXPERIMENTAL_UPGRADE_FROM_3X=true memory_mcp surrealdb-v4-maintenance --role primary

  # HTTP Registry and configured ready tenant stores
  env SURREAL_EXPERIMENTAL_UPGRADE_FROM_3X=true memory_mcp_http surrealdb-v4-maintenance
  ```

  Keep the application offline until the command completes. If it is interrupted, keep the flag enabled and retry. Remove the flag before starting the service. Never point a v3 binary at converted data.
- To roll back, stop all v4 processes and restore the complete pre-upgrade backup set into clean paths. Keep the external backup and `_rocksdb_backup/`; the latter is not a substitute. Writes made after the backup will be lost. See the [upgrade guide](README.md#upgrade-to-v1400) and test the restore before upgrading.
- An interrupted index rebuild leaves an aborted build and an incomplete maintenance claim. After stopping every other v4 process and confirming no server-side rebuild is running, rerun the maintenance command with `--recover-ambiguous --confirm-no-prior-submitter-can-resume`. One rebuild is bounded by `SURREALDB_V4_MAINTENANCE_TIMEOUT_SECS` (default `1800`), which a large fact or embedding table may need raised.
- `surrealkv://` is the preferred local file-backed URL. The existing `rocksdb://` application URL remains accepted as an alias, but now opens SurrealKV.
- The v1.40.0 SDK works with stable remote SurrealDB 3.x and 4.x; tests cover 3.3.5 and 4.0.1. Remote v3 servers do not need an upgrade. Remote v4 catalog and index maintenance requires `SURREALDB_ALLOW_REMOTE_V4_INDEX_REBUILD=true`, all datastore-sharing nodes on v4, and a coordinated window with older writers stopped. See the [upgrade guide](README.md#upgrade-to-v1400) for the pre-start commands.
- Lexical scores and near-tie vector rankings may differ under SurrealDB 4.

### Evaluation
- The PR and release profiles passed 150/150 and 154/154 cases. The response-size profile passed 61/61, with overall savings of 31.0% for context assembly and 17.9% for explanations.
- The v1.39.0 retrieval-quality run was invalid because no embedding provider was configured, so there is no valid before-and-after retrieval comparison.

## [1.39.0] - 2026-11-16

### Added
- Remote SurrealDB connections now use TLS for `wss://` and `https://` endpoints.
- The optional `eval-support` feature exposes a bound remote client constructor with an explicit embedding dimension for evaluation fixtures.

### Changed
- **Breaking for source builds:** the minimum supported Rust version is now 1.100.0 (up from 1.99.0). CI and Docker builds follow rolling stable.

### Fixed
- Triple-based retrieval now selects source fact IDs as values so related facts resolve correctly.

## [1.38.0] - 2026-11-09

### Other
- Updated workspace Rust dependencies to the latest crates.io releases (`rmcp` 3.9, `lopdf` 0.47, `zip` 9.4, `notify` 8.2.2, `uuid` 1.29.1 and the transitive tree).
- `surrealdb` deliberately stays on 3.x: 4.0.0 removed the `kv-rocksdb` storage engine behind the documented `rocksdb://` connection URLs, and upgrading it requires a storage-migration decision.

## [1.37.2] - 2026-11-03

### Fixed
- ZIP-based document extraction and release archive verification now handle the fallible entry-name API in `zip` 9.

### Other
- Updated workspace Rust dependencies to the latest compatible crates.io releases.

## [1.37.1] - 2026-11-03

### Fixed
- Prevented unrelated free-text records and ambiguous subjects from producing false contradictions.
- Preserved genuine conflict warnings while reducing repeated fact reads during extraction.
- Prevented dates, percentages and unresolved grammatical subjects from being promoted as automatic entity identities.

## [1.37.0] - 2026-10-18

### Added
- The HTTP server starts and remains available while the configured OIDC provider is offline. Discovery and signing-key refresh retry in the background, while local admin and API-key authentication remain available.
- Readiness and Prometheus metrics expose OIDC discovery and signing-key health, including provider recovery.

### Changed
- **Breaking:** OAuth token validation now uses the shared OIDC client. Bearer-principal resolution returns provider outages separately from rejected credentials.

### Fixed
- Temporary OIDC failures are reported as retryable service errors instead of invalid credentials. Provider requests have bounded time and response sizes.

## [1.36.0] - 2026-10-16

### Added
- Periodic stock-level background worker that stamps active-fact, archival-candidate and community counts as gauges on the decay cadence, so the product dashboard's stock curves stay current without opening the lifecycle view.

### Changed
- Reorganized the product dashboard into clearer sections and added a "Reading the product dashboard" guide covering the freshness stamp and the stock curves.
- Collapsed optional variables behind a collapsible block in the environment-variable index, and moved non-operator variables (eval-harness, tests, build) into their own table.

### Fixed
- Docker Compose now forwards `MEMORY_MCP_HTTP_SECRET_KEY` and reads an unset individual secret as empty, so the documented root-secret-only flow works instead of failing at interpolation.
- Corrected environment-variable documentation the code contradicted: the HTTP profile rejects `MEMORY_INGESTION_INBOX` rather than ignoring it, `SURREALDB_FS_WATCH_INBOX` is inert rather than rejected, plan-seed limits parse as `u32`/`u64`, and only the two concurrency limits reject `0`.
- Fixed stale evaluation commands, an incorrect `-p memory-mcp` package name, nonexistent `watch` CLI references, and broken ADR links.

## [1.35.0] - 2026-10-15

### Added
- Added a bounded, configurable pool for reusable tenant maintenance sessions, with idle expiry, shutdown draining and Prometheus metrics.
- Added dimension-aware tenant binding and backfill page decisions to preserve schema and embedding backfill correctness.

### Changed
- **Breaking:** `HttpConfig` adds three public fields required by downstream struct-literal construction.
- **Breaking:** Replaced the public `lifecycle_scheduler_job` function and callable `SchedulerJob` alias with categorized scheduler jobs and `lifecycle_scheduler_jobs`.
- Grouped retrieval, graph, archival and embedding-backfill reads, and narrowed projections to reduce repeated SurrealDB query work.
- Bounded and fairly dispatched maintenance scans; ready-tenant schemas are upgraded and verified before use.

### Fixed
- Preserved bi-temporal validity in lexical fallback and sequence continuity during outbox repair and schema upgrades.

[1.40.0]: https://github.com/like-a-freedom/memory_mcp/compare/v1.39.0...v1.40.0
[1.39.0]: https://github.com/like-a-freedom/memory_mcp/compare/v1.38.0...v1.39.0
[1.38.0]: https://github.com/like-a-freedom/memory_mcp/compare/v1.37.2...v1.38.0
[1.37.2]: https://github.com/like-a-freedom/memory_mcp/compare/v1.37.1...v1.37.2
[1.37.1]: https://github.com/like-a-freedom/memory_mcp/compare/v1.37.0...v1.37.1
[1.37.0]: https://github.com/like-a-freedom/memory_mcp/compare/v1.36.0...v1.37.0
[1.36.0]: https://github.com/like-a-freedom/memory_mcp/compare/v1.35.0...v1.36.0
[1.35.0]: https://github.com/like-a-freedom/memory_mcp/compare/v1.34.2...v1.35.0
