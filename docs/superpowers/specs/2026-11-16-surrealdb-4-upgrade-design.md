# SurrealDB 4.0 upgrade design

**Status (2026-11-24)** Implemented in the released master lineage. The current checkout is v1.41.0 and already uses SurrealDB SDK 4.0.1 with `kv-surrealkv` and `kv-mem`. The v1.40.0 target and 3.3.5 implementation baseline below describe the original upgrade design. Current operator instructions are in [the README upgrade section](../../../README.md#upgrade-to-v1400). Release publication and production datastore conversion require their own recorded evidence.

## Intent

Update the `surrealdb` Rust SDK from 3.3.5 to 4.0.1 and release Memory MCP 1.40.0 from `master`. The release must keep existing user data safe, make the one-way embedded-storage conversion explicit, and explain the required operator steps in the README and release notes.

The SDK upgrade also changes the embedded engine from RocksDB to SurrealKV. SurrealDB 4.0 can convert a 3.x RocksDB or SurrealKV datastore in place, but refuses the conversion by default. Converted data is not usable by 3.x; an old RocksDB binary can instead open an empty database at the converted path. The application must not silently opt users into that conversion.

## Goals

- Build the workspace against `surrealdb` 4.0.1, using `kv-surrealkv` and `kv-mem`.
- Preserve the current `SURREALDB_DATA_DIR` path and data layout selection for local users.
- Require an explicit, one-time application opt-in before any embedded 3.x datastore is converted.
- Give users a clear refusal error when an old embedded datastore needs the opt-in, including a backup instruction and the exact setting to use.
- Apply the opt-in to every physical embedded datastore used by the selected profile: stdio primary, or HTTP control registry and a separately configured HTTP tenant engine.
- Keep the v4 SDK usable with remote SurrealDB 3.x and 4.x servers. Verify compatibility against SurrealDB 3.3.5 and 4.0.1 without introducing a narrower minimum remote-server version. Do not require a remote server upgrade for this release.
- Preserve temporal query correctness and exercise retrieval-quality changes caused by SurrealDB 4.
- Document the one-way conversion, backup and recovery procedure in the README and v1.40.0 release notes.

## Non-goals

- Opening or converting production data as part of this work.
- Upgrading a remote SurrealDB server or changing its own deployment process.
- Changing Memory MCP's data model, tenant boundaries, MCP API, or default namespace.
- Listing every upstream SurrealDB 4 change. Document only changes relevant to this repository and its operators.
- Changing the requested application version from 1.40.0.

## Historical implementation baseline and relevant behavior

- `Cargo.toml` pins `surrealdb` 3.3.5 with `kv-rocksdb` and `kv-mem`.
- `crates/memory-mcp/src/storage/client.rs` opens the primary embedded store as `RocksDb` at the configured data directory. The in-memory path is separate.
- `crates/memory-mcp/src/http/registry/surreal_store.rs` accepts `rocksdb://...` for a file-backed local registry and creates a `RocksDb` engine. Its migration ledger is separate from the memory schema migration ledger.
- The Rust SDK's embedded datastore does not read `SURREAL_EXPERIMENTAL_UPGRADE_FROM_3X` from the process environment. It must receive `Config::datastore_option("experimental_upgrade_from_3x", true)`.
- The lexical retrieval query in `crates/memory-mcp/src/knowledge/queries.rs` ranks matches with `search::score(1)`. SurrealDB 4 changes BM25 scoring, so the order of lexical results can change, especially for common terms. The query already supplies deterministic tie-breakers.
- The repository has a managed HNSW index (`fact_embedding_hnsw`), full-text indexes (`fact_content_search`, `fact_index_keys_search`, and `community_summary_search`), and B-tree indexes containing datetime values. The temporal retrieval paths explicitly order by timestamps. SurrealDB 4 documents that some indexes built by 3.x need rebuilding for correct 4.x ordering or index behavior. The app should own this maintenance rather than leave users to discover and rebuild indexes by hand.
- Other 4.0 behavior changes, such as unspecified order without `ORDER BY`, exact float/decimal comparisons, and decimal literal precision, are not part of Memory MCP's documented query contract. The query audit found explicit ordering in the retrieval builders reviewed; tests will cover the relevant result contracts.

## Approaches considered

1. **Explicit in-place conversion with a cold backup (selected).** Stop the application, copy and verify the complete data directory, then set one opt-in for the first v4 start. SurrealDB performs its supported in-place conversion. The application runs its one-time v4 index maintenance with a durable migration record. This keeps the existing path and avoids a manual export/import workflow, while making the irreversible step deliberate.
2. **Convert into a separate directory and switch paths.** This preserves the original directory in place and gives a simpler rollback, but adds a separate migration tool or export/import procedure, requires extra disk space, and makes local and HTTP-registry setups harder to explain. Keep this as an advanced recovery/operations alternative if the upstream tooling proves more reliable for a particular deployment; it is not the default release path.
3. **Convert automatically on first start.** Rejected. A default startup must never perform a one-way conversion without an explicit operator action.

The selected approach matches the previously chosen backup-plus-opt-in flow.

## Design decisions

### Embedded conversion gate

Add `SURREAL_EXPERIMENTAL_UPGRADE_FROM_3X`, defaulting to `false`. Parse it with the project's existing boolean environment conventions. When true, pass the SDK datastore option to both embedded `SurrealKv` connections. Do not pass or apply it to remote connections; a remote server's upgrade is controlled by that server's operator.

When a recognized old embedded datastore is refused, return an actionable error explaining that conversion did not occur only if pinned-engine evidence establishes that claim, and directing the operator to stop every process that uses it, make and verify a cold copy, then retry with the variable set to `true`. If conversion is interrupted, preserve the setting for the retry as required by upstream guidance. Once startup confirms conversion and application migrations have completed, the user can remove the variable. Fresh stores do not need it.

For a rollback after conversion, stop all v4 processes and restore the pre-upgrade cold copy. Writes made after that copy are not in the restored data. A v3 binary must never be pointed at a converted directory: an old RocksDB binary may start with an empty database instead of refusing. Restore into a clean, separate directory; never overlay a backup onto converted files. Keep the external backup independently of `_rocksdb_backup/`, which is not a complete application rollback procedure.

The README will instruct users to back up every distinct physical local datastore for their profile: the stdio primary directory, or the HTTP control and tenant directories. Equivalent paths share one backup; separately configured tenant storage must not be omitted. It will warn that the first start can take a long time and needs free space for the converted datastore alongside the old files. The README will distinguish embedded storage from remote servers: the v4 SDK supports remote 3.x and 4.x, and this release does not require a server upgrade.

### Engine and URL compatibility

Use SurrealKV for all local on-disk storage. Keep `SURREALDB_DATA_DIR` and its default path unchanged. Add `surrealkv://` as the preferred scheme for file-backed local registry configuration. Continue accepting the existing application-level `rocksdb://` scheme as a deprecated alias that opens the same path using SurrealKV, so this release does not add an avoidable configuration break. Update examples and documentation to prefer `surrealkv://` and explain that the legacy scheme no longer selects RocksDB.

### Version-aware index maintenance

Before any schema or provisioning mutation, run a read-only version, provenance and authorization preflight. Apply maintenance only after each role's complete catalog, during an explicitly exclusive pre-start phase. Get the engine/server version from the SDK's dedicated version operation, not `INFO FOR DB`. Stable 3.x uses the compatible catalog path and receives no v4-only SQL. Stable 4.x follows the provenance and maintenance gates. Reject prereleases, unknown or unparsable versions, and future major versions before mutation. The tested remote versions remain 3.3.5 and 4.0.1; this policy does not set a new minimum supported 3.x version.

An endpoint reporting v4 does not prove that every node sharing its datastore has upgraded. Require the separate remote-maintenance acknowledgement before sending any catalog containing `DEFINE INDEX` (plain, `IF NOT EXISTS`, or `OVERWRITE`) or a standalone `REBUILD INDEX`. This applies to fresh v4 databases too. If version, provenance, or authorization cannot be established, stop before mutation and report the pre-start maintenance command or operator action needed.

The reviewed rebuild allowlist is deliberately narrower than every index that contains a date-like field. The same memory manifest applies to the stdio primary and HTTP tenant databases:

- `edge_temporal_idx`, `fact_t_valid`, `episode_t_ref`, and `edge_t_valid` support temporal retrieval and ordering.
- `claim_relation_left_active_idx`, `claim_relation_right_active_idx`, `claim_job_lease_idx`, and `inbox_revision_state_lease` support active-relation or lease lookups.
- `entity_aliases` (`FIELDS aliases.*`) and `community_members` (`FIELDS member_entities.*`) support production membership reads. Rebuild both array-element indexes; SurrealDB 4 may still choose a table scan for production queries with `LIMIT` or incompatible ordering, so tests assert index eligibility on the predicate rather than promise a particular production plan.
- `community_summary_search` is rebuilt because summaries can change or be deleted; `fact_embedding_hnsw` is rebuilt because re-embedding changes indexed vectors.

The registry manifest currently has no eligible rebuild index. `idx_audit_event_account` includes `occurred_at`, but the code audit found no confirmed date-range or date-order consumer. Other registry indexes do not cover mutable datetime ordering. Keep this exclusion explicit and verify the manifest against migration DDL and production queries before release.

Exclude `event_projection_job_lease` because `lease_expires_at` is stored as a string. The `query_log_*` indexes also use string `logged_at` values. Exclude `claim_relation_context_idx`, `claim_relation_left_fact_active_idx`, `claim_relation_right_fact_active_idx`, `claim_relation_schema_outcome_active_idx`, `claim_alias_lookup_idx`, `claim_policy_lookup_idx`, and `entity_extraction_projection_ingested_idx` because no production read path using their indexed lookup or ordering was found. `fact_content_search` and `fact_index_keys_search` are excluded from rebuild work because their indexed source values are immutable after creation; retrieval and evaluation tests still cover the SurrealDB 4 scoring change. Do not rebuild user-created indexes.

Key maintenance records by namespace, database, and maintenance revision. Track per-index progress to make interrupted runs recoverable and prevent completed work from repeating on every startup. Run each `REBUILD INDEX` as a standalone statement outside a transaction. Emit progress and actionable errors without logging database contents. A failure keeps readiness closed. Maintenance can lengthen the first v4 startup, especially for large fact or event tables.

### Search behavior

Keep existing retrieval queries and explicit ordering. Add focused regression coverage for lexical and semantic retrieval, and run the evaluation harness before and after the dependency change. Release notes will state that lexical scores and near-tie vector rankings may differ under the new SurrealDB engine. Do not claim that evaluation quality improved; report the measured result.

### Public documentation and release

- Add an "Upgrade to v1.40.0" section to `README.md` near storage setup, including the stop/backup/verify/opt-in/start/unset/rollback sequence and the distinction between embedded and remote storage.
- Add the same short, actionable migration instructions to `CHANGELOG.md` and the GitHub release notes. State clearly that the on-disk conversion is one-way and that a v3 binary cannot reopen the converted directory.
- Update `.env.example`, `AGENTS.md`, and the directly relevant storage/operator documentation where they still describe RocksDB as the current engine. Refresh ADR-0033's conclusion so its historical rationale is not mistaken for the new implementation. Keep historical references that explain prior behavior, labeling them as such.
- Bump the crate version to 1.40.0 and publish `v1.40.0` directly from `master`, without a PR or release branch, as requested.

## Validation and acceptance criteria

1. Unit tests prove the opt-in defaults off, is parsed consistently, and reaches both embedded constructors; invalid values follow established config behavior.
2. Tests prove a refused legacy-store startup gives the backup/opt-in guidance and does not mutate the directory.
3. Generate populated, pinned 3.3.5 RocksDB and SurrealKV fixtures reproducibly with isolated fixture-producing executables; do not add a production dependency or commit database artifacts. Both claimed conversion paths are mandatory release gates. Verify refusal without opt-in, successful conversion with it, preserved application records, restart without opt-in, and rollback by reopening a disposable backup copy with the previous application. Never rely on the old RocksDB engine refusing converted data.
4. Verify the v4 index-maintenance step runs once and can recover after interruption; verify it is skipped for remote SurrealDB 3.x. Verify the registry path follows the same conversion gate and its relevant index maintenance.
5. Run focused storage, migration, registry, and retrieval tests, then `cargo test -p memory_mcp`, `cargo check`, `cargo fmt --all --check`, and the repository-required workspace Clippy command with `-D warnings`.
6. Run the existing blocking `pr` and `release` eval-harness profiles against the baseline and upgraded build, using the same corpus and settings. Do not add or loosen thresholds for this migration. Record the measured scores and ranking shifts; both profiles must pass their existing gates.
7. Confirm no production instance or database was contacted or changed. Publish only after the pre-publication gates below pass, then verify the `v1.40.0` tag, GitHub release, assets, and Actions result.

## Safety and recovery requirements from design review

### Preflight and coordinated backups

Before opening either embedded engine with conversion enabled, validate configuration, namespace/database identifiers, and resolve the configured storage paths. Inventory HTTP control and tenant engines independently from stdio primary. Identify shared, aliased or overlapping directories (including symlinks and nonexistent path ancestors), deduplicate valid shared engines and reject conflicting simultaneous ownership before conversion. Never open the unused stdio default for HTTP preflight. Do not open an unrelated local store when that role is configured remotely.

Stop all writers, including filesystem ingestion, HTTP workers and other application instances. Back up all related embedded stores in the same stopped-writer window, preserving permissions and the complete directory contents. Verify file integrity and perform a restore drill on a disposable copy using the previous application and representative known records. Rehearse the upgrade on a separate copy before the real opt-in. A checksum alone does not establish recoverability. Where a supported 3.x export is available, retain and verify it as an additional recovery artifact; document its authentication/schema restore caveats rather than presenting export/import as a lossless shortcut.

Conversion across two stores is not atomic. If one store converts and another fails, keep the application unavailable. Either resume conversion with opt-in or restore the coordinated backup set into clean directories. Only the recognized upstream legacy-format refusal may say that the datastore was not modified. Any later startup failure must explain that conversion may already have completed.

Do not delete `_rocksdb_backup/` automatically. Document space for the external backup plus the simultaneous old/new engine files, without promising a fixed multiplier. Extend startup-probe and supervisor timeouts for the maintenance window; repeated forced restarts can restart conversion. Never suggest bypassing a datastore lock or deleting engine files to fix startup.

### Remote maintenance authorization

Remote SDK compatibility does not authorize a server upgrade or index rebuild. Keep the embedded conversion opt-in separate from remote maintenance permission. Before first maintenance of an existing v4 remote database, require an explicit operator acknowledgement that all datastore-sharing nodes are on v4, any required record-format transition is complete, and a coordinated maintenance window excludes older writers. Current remote acknowledgement uses `SURREALDB_ALLOW_REMOTE_V4_INDEX_REBUILD` as documented in the README; do not reuse `SURREAL_EXPERIMENTAL_UPGRADE_FROM_3X` for this purpose.

Without that acknowledgement, do not submit index-mutating catalog SQL, rebuild SQL or expose affected workloads as ready. This includes fresh remote v4 schema creation: a fresh database does not establish that every datastore-sharing node is on v4. Give guidance that upgrading the application against a 3.x remote server remains supported and needs no v4 maintenance. The app cannot enforce a cluster-wide write pause; this is an explicit operator prerequisite, not a guarantee inferred from this application's readiness. Purely fresh databases created by v4 need no legacy-index rebuild; establish that provenance durably rather than infer it from an absent maintenance row.

### Maintenance ownership and interrupted builds

Use an explicit pre-start maintenance phase. `memory_mcp surrealdb-v4-maintenance --role primary` handles the stdio primary. `memory_mcp_http surrealdb-v4-maintenance` runs with the HTTP deployment configuration, does not bind a listener or start workers, and handles the control registry plus all ready tenant database bindings. It deduplicates a shared local target and fails preflight before mutation if any configured target is unknown or unsafe. The command owns the complete role-specific catalog and rebuild sequence because current registry and service constructors apply migrations while connecting.

Before schema mutation, inspect server version and database provenance, validate authorization, and acquire a durable claim keyed by namespace, database and maintenance revision. `CREATE ONLY` on a fixed claim record ID provides atomic single-submitter admission; a 4.0.1 `kv-mem` probe confirmed two simultaneous submitters produce one success and one `AlreadyExists`. Store owner and attempt IDs, role, server version, current index, timestamps and state (`active`, `ambiguous`, `failed` or `completed`); never store database content. Claims do not expire. Persist per-index intent before sending standalone `REBUILD INDEX`, and mark it verified only after a successful response and postcondition checks.

Normal startup performs read-only version/provenance checks before the current eager catalog paths. It refuses an existing v4 database with absent or ambiguous upgrade evidence before issuing DDL and prints the relevant pre-start command. A verified empty database with no application schema or data can be recorded as fresh before its catalog is created; fresh provenance skips legacy rebuilds, not remote cluster-safety authorization. Stable 3.x uses only compatible catalog SQL and does not run v4 maintenance.

A timeout, disconnect, process death after submission, or failed durable completion write leaves the claim ambiguous and readiness closed. Recovery requires `--recover-ambiguous --confirm-no-prior-submitter-can-resume`, after the operator has stopped every old submitter and confirmed no server-side rebuild is still running. This confirmation is an operational assertion, not a lease or database fencing guarantee. Never take over on lease expiry. A deliberate exclusive retry may repeat a rebuild whose response was lost; exactly-once execution is not promised.

Inspect actual build state and wait for running work, but do not treat `ready` or matching DDL as proof that this upgrade rebuilt the index. Accept a successful rebuild response plus verified postconditions and durable completion; a lost response or completion write is ambiguous unless a pinned API supplies stronger build-generation evidence. Keep readiness closed in that case. Under exclusive operator recovery a deliberate retry may repeat completed work; exactly-once execution is not promised. Deliberately recover an errored build. A missing required managed index is an actionable failure; a role-inapplicable index is excluded from that role's manifest. Only mark maintenance complete after every applicable index has been verified ready. Bound waits and report how to resume after timeout while keeping readiness closed. Never remove indexes or relax uniqueness to recover automatically.

Tests must control ordering for competing submitters, a paused old submitter resuming after another completes, death before SQL submission, lost submission responses, background builds surviving process death, and a successful build followed by a failed ledger write. Use controlled adapters and barriers, plus real-engine integration coverage for SQL and build-state semantics.

### Database roles and startup boundaries

Run maintenance after the complete applicable schema catalog, before admitting requests or background workers:

- Local primary database startup.
- Newly provisioned HTTP tenant databases, after base and HTTP-specific migrations.
- Activation of existing ready tenants, including prebound clients.
- Registry startup, after the registry's independent catalog.

Maintain explicit per-role index manifests with table names and definition checks. The inventory above is a review starting point, not permission to rebuild an index on the wrong table or before it exists. Tenant maintenance readiness is separate from the schema-version number. Cover multiple databases on one engine and separately configured registry and tenant endpoints.

### SQL compatibility and release evidence

Run complete migration and query contracts against both a populated upgraded database retaining compatibility level 3 and a fresh v4 database at level 4. Do not force a compatibility-level change as part of this release. When migrations 034 and 022 are both pending on a fresh catalog, apply 034 before 022 so the `edge.provenance` parent exists for the nested fields. Honor the migration ledger when either one is already applied, and leave historical migration files unchanged. Registry migration 001 already defines the `plan.limits` parent before migration 046 adds nested fields.

Audit stored and generated SQL for nested field definitions, field changes on populated SCHEMAFULL tables, protected variables, transaction `RETURN` behavior, record IDs, datetime values, and indexed ordering with pagination or limits. In SurrealDB 4, `RETURN` does not stop later statements in a transaction. Add result and durable-state regressions for `resolve_session`, `rotate_session`, and `reserve_attempt`; the expected transaction behavior must not depend on `RETURN` terminating the script. Fix affected contracts without rewriting historical migration files unless separately approved.

The release matrix must include actual isolated remote servers pinned to 3.3.5 and 4.0.1. Cover authentication, full catalogs, memory and registry writes/transactions, datetime and record-ID round trips, lexical and vector retrieval, maintenance authorization refusal, and unknown/unparseable version errors. Prove that no v4-only SQL reaches 3.x and no embedded opt-in is passed remotely. Runtime policy accepts stable 3.x and 4.x by major, refuses prereleases and future majors, and does not claim a new 3.x minimum; 3.3.5 and 4.0.1 remain the exact tested points.

Bind test/eval evidence to the release candidate SHA, dependency lockfile, corpus and settings. Require both existing blocking eval profiles without weakening their gates. Require conversion, restart, interruption and restore-drill evidence for both embedded constructors, including partial two-store failure.

Inspect the release workflow before publication: a workflow triggered by `release: published` cannot serve as a pre-publication gate. Build and validate the exact release commit before publication and stage assets in a draft if needed. If the current workflow cannot do that, adjust its preflight/draft mechanism as part of release preparation. Do not publicly publish a release merely to start its validation. Remain on `master`, create no PR or release branch, and do not contact production storage.

## Review status

The design direction was approved and amended after code-grounded review. Task 0 has now established pinned refusal/conversion behavior, exclusive maintenance and recovery rules, rebuild-response limitations, deterministic interruption checkpoints, and fixture-runner details. Its SQL audit narrowed the original rebuild list to indexes with a confirmed mutable retrieval, ordering, or lookup consumer; the memory allowlist and exclusions are recorded above. The implemented API behavior is owned by the storage constructors and migration adapters. The README records the current conversion and remote-maintenance contract. Remote acknowledgement is `SURREALDB_ALLOW_REMOTE_V4_INDEX_REBUILD` and covers every catalog containing `DEFINE INDEX`, as well as explicit rebuild statements. Keep unresolved integration behavior, including remote atomic claim admission and full server compatibility, as release gates rather than treating local probes as proof.

## External references

- [SurrealDB 3.x to 4.0 migration guide](https://surrealdb.com/docs/build/migrating/from-old-surrealdb-versions/3x-to-4x.md)
- [SurrealDB Rust SDK migration guide](https://surrealdb.com/docs/reference/rust/migrating-to-3x.md)
