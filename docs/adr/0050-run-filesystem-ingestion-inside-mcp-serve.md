# ADR-0050: Run Filesystem Ingestion Inside the MCP Server

## Status

Accepted — 2026-08-24.

## Context

The optional `watch` CLI mode requires a user to run and supervise a second long-lived process beside each stdio MCP server. That does not fit the product's actual usage: agents invoke the MCP tools or one-shot CLI operations, while users reasonably expect an MCP host to own the lifetime of supporting background work. The existing watcher also ingests only an episode, despite documentation promising `ingest → extract`, and one file error terminates the entire watcher.

Multiple stdio clients normally launch separate Memory MCP processes. Embedded RocksDB cannot be shared concurrently through one data directory, so each such process needs its own `SURREALDB_DATA_DIR`; changing only the database name or Active Namespace does not avoid directory ownership conflicts.

## Decision

Filesystem Watch becomes an optional background capability of `serve`, activated only when `MEMORY_INGESTION_INBOX` is present. The variable must contain a non-empty absolute path to an existing readable directory that is not a symlink. Shell constructs such as `~` and `$HOME` are not expanded. When the variable is absent, startup and runtime behavior remain unchanged.

Remove the standalone `watch` command and rename the additive Cargo feature from `cli-watch` to `fs-watch`, without a compatibility alias. Package defaults remain empty, while official release binaries include `fs-watch`. A binary compiled without the feature rejects a configured inbox with an actionable startup error.

One process watches one inbox recursively. Startup validates and attaches the OS watcher before making the MCP transport ready, then scans existing files in the background so files cannot fall into a scan-to-watch race. Startup does not wait for extraction. Supported formats remain PDF, DOCX, XLSX, PPTX, Markdown, plain text, and EML; unsupported files and all symlinks are skipped. Files are processed only after size and modification time stabilize. The watcher never moves, deletes, or rewrites user files, and deletion does not invalidate memory.

Each logical document has an Inbox Source Lineage derived from its normalized path relative to the inbox. Moving or renaming a file begins a new lineage. Each distinct set of source bytes is an immutable Inbox Revision identified by a content hash. Filesystem ingestion stores a versioned episode `source_id` and a separate stable `source_lineage`; claim projection prefers explicit episode lineage and otherwise retains the existing fallback to episode `source_id`. This preserves source continuity for authorized reconciliation without exposing the filesystem-specific identity syntax to generic claim code or expanding the public `ingest` request.

For EML, `t_ref` comes from a valid structured `Date` header. Other formats use filesystem modification time, falling back to observation time. `t_ingested` remains the actual ingestion transaction time. Inbox content remains ordinary source-labeled external data and receives no elevated trust or privileged policy tags.

A narrow internal Inbox Revision store owns durable states `discovered`, `processing`, `processed`, and `failed`, plus content identity, episode ID, attempts, lease, timestamps, and bounded failure classification. Processing is sequential and lease-based. A revision is `processed` only after `ingest → extract` succeeds. A crash or partial extraction is recovered by idempotently rerunning the pipeline after lease expiry; already-created derived records may remain visible because revision completion is an operational status, not a transaction boundary over the general extraction pipeline.

Transient I/O, storage, model, timeout, and watcher-backend failures receive bounded exponential retries. Deterministic validation or corrupt-content failures do not retry during the current cycle. A failed revision receives at most one new bounded retry cycle on the next process startup, allowing recovery after configuration repair without an additional public command. One file failure never stops filesystem ingestion or MCP service. The OS watcher itself is recreated with bounded backoff and then enters a logged degraded state while MCP and already queued work remain available.

Shutdown stops discovery and dequeue, grants the current revision 30 seconds, then cancels the task without marking shutdown as a domain failure; the lease permits later recovery. Embedded directory lock/resource-busy failures are translated into an actionable error explaining that each stdio process needs a unique `SURREALDB_DATA_DIR` or remote SurrealDB.

No MCP tool is added and the frozen eight-tool surface and One Active Namespace contract remain unchanged. `memory_mcp init` may show filesystem ingestion only as optional guidance and must not enable it automatically.

## Observability

Structured events cover readiness, startup scan summary, revision start/retry/success/failure, backend retry, degraded operation, and shutdown. Revision events contain relative paths and short revision prefixes only; file contents never appear in logs, and the absolute inbox root appears only in startup diagnostics.

Prometheus instrumentation uses bounded labels only:

- `memory_fs_watch_revisions_total{outcome}` for `processed`, `failed`, `skipped_duplicate`, and `interrupted`;
- `memory_fs_watch_retries_total{stage,reason}`, with bounded stages `backend`, `read`, `ingest`, and `extract`, and bounded transient reason classes;
- `memory_fs_watch_scan_files_total{outcome}` for enqueued and bounded skip/failure classes;
- gauges `memory_fs_watch_queue_depth`, `memory_fs_watch_inflight`, and `memory_fs_watch_degraded`;
- `memory_fs_watch_revision_duration_seconds{outcome}` with bounded outcomes.

Paths, hashes, episode IDs, and error text are never metric labels.

## Consequences

- Filesystem ingestion now follows the lifecycle users already delegate to their MCP host and produces queryable facts without requiring an agent to notice and extract new episodes.
- Existing inbox files are imported when the capability is first enabled; users opt into this explicitly by configuring the directory.
- The durable revision store requires an append-only migration and a narrow storage owner.
- Release, CI, README, `AGENTS.md`, `memory-cli`, and `init` guidance must replace `cli-watch`/standalone-watch instructions.
- Raw-byte hashing may create a new revision when an Office document is technically re-saved without a visible text change. This is preferred to missing a meaningful change.
- Partial extraction can be visible before a revision reaches `processed`; introducing staged fact visibility is outside this decision.

## Alternatives considered

### Keep a standalone watcher process

Rejected because neither an agent nor a typical user reliably starts and supervises the second process, and separate service construction duplicates lifecycle ownership.

### Keep the old command as a compatibility wrapper

Rejected because it preserves the invalid product model, separate shutdown path, and testing surface. The removal is an intentional breaking change.

### Use only in-memory deduplication

Rejected because it cannot distinguish completed work from a crash between ingest and extract, cannot safely scan on every startup, and cannot provide bounded retry cycles.

### Identify versions only by `t_ref` or parse lineage from `source_id`

Rejected because valid time is not revision identity and generic claim projection must not depend on a filesystem source-ID grammar.

### Add a ninth MCP status or retry tool

Rejected because logs, bounded metrics, durable recovery, and restart retry satisfy the initial operational need without changing the frozen MCP surface.

## Delivery gate

The decision is implemented only when tests demonstrate disabled-mode compatibility, non-blocking startup scan, full `ingest → extract`, revision idempotency and lineage continuity, rename/delete semantics, stability checks, lease recovery, bounded file and backend failures, restart retry, bounded shutdown, independent data directories, actionable embedded lock errors, feature-disabled configuration errors, unchanged MCP surface, and removal of the old command and feature. Formatting, workspace tests, and zero-warning Clippy with `fs-watch,mcp-apps` must pass.
