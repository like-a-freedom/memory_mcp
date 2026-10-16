# Changelog

All notable changes to this project will be documented in this file.

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

[1.36.0]: https://github.com/like-a-freedom/memory_mcp/compare/v1.35.0...v1.36.0
[1.35.0]: https://github.com/like-a-freedom/memory_mcp/compare/v1.34.2...v1.35.0
