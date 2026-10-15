# Changelog

All notable changes to this project will be documented in this file.

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

[1.35.0]: https://github.com/like-a-freedom/memory_mcp/compare/v1.34.2...v1.35.0
