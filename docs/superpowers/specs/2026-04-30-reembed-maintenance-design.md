# Embedding Rebuild Maintenance Design

> **Purpose:** add a safe, explicit maintenance workflow for rewriting every stored `fact.embedding` after an embedding provider or model switch, while keeping normal MCP operation simple and preserving compatibility with existing databases.

**Date:** 2026-04-30  
**Status:** Proposed target-state design  
**Scope:** CLI maintenance mode, storage metadata, migration compatibility, index lifecycle, startup safety, progress reporting, and retry/resume behavior

---

## 1. Why this document exists

`memory_mcp` already supports switching embedding providers through environment variables, but the current runtime shape leaves two operational gaps:

1. old vectors remain in the semantic space of the previous provider/model;
2. migration `008_fact_semantic_embeddings.surql` renders `__FACT_EMBEDDING_DIMENSION__` into SQL that is then checksummed, so changing the configured dimension can make an already-applied migration appear “modified after execution.”

That means provider switching is currently only partially supported: configuration can change, but existing fact embeddings do not become correct automatically, and startup compatibility around the historical HNSW migration is too fragile.

The user-approved goal is to make provider/model switches operationally safe without expanding the MCP tool surface and without editing historical migration files.

---

## 2. User-approved requirements

The design MUST satisfy all of the following:

- rewrite embeddings for **all facts**, including invalidated / historical facts;
- run the rewrite **outside normal MCP serve mode**;
- keep compatibility with current databases;
- never edit existing migrations; add new migrations only;
- show visible progress in percent and/or counts plus ETA;
- emit thorough structured logs for startup checks, maintenance phases, storage transitions, and failures so operators can debug unexpected behavior without instrumenting the code after the fact;
- prefer KISS, DRY, YAGNI, and repository-fit DDD boundaries;
- avoid guessing on unclear requirements.

The only ambiguity that materially affected the architecture was rewrite scope. That was resolved explicitly: **rewrite every `fact` row, not only active facts**.

---

## 3. Current architecture facts this design must respect

The design must fit the repository as it exists today:

- embeddings are stored only on `fact.embedding`;
- `add_fact()` writes embeddings inline during fact creation;
- a single embedding provider is selected at startup and stored on `MemoryService`;
- CLI currently exposes `serve` (default) and `watch` modes only;
- migration validation is strict and checksum-based via `script_migration` records;
- the public MCP contract should remain unchanged — this is an operator / binary workflow, not a new MCP tool.

One additional repository fact matters for safety: `MemoryService` holds a **single** embedding provider shared across all configured namespaces. That means startup cannot safely enable semantic retrieval for one namespace and disable it for another within the same process. Semantic enablement is therefore a **global process decision** derived from namespace state.

---

## 4. Design options considered

### Option A — hard in-place rewrite, no state

Run a one-shot maintenance pass that:
- starts a provider,
- scans every fact,
- overwrites `fact.embedding`,
- recreates the HNSW index,
- exits.

Pros:
- smallest implementation surface;
- easy to explain.

Cons:
- poor crash recovery;
- no durable progress state;
- difficult to distinguish completed from partial rewrites;
- weak operator UX for large databases or remote providers.

### Option B — blue/green embeddings

Keep old and new embeddings in parallel fields and atomically switch the active one after a full rewrite.

Pros:
- strongest isolation;
- minimal mixed-state risk while the job is running.

Cons:
- extra schema and query complexity;
- more storage cost;
- over-designed for the repository’s current single-binary maintenance use case.

### Option C — persisted maintenance job + startup gating **(recommended)**

Add a dedicated maintenance command that:
- persists rewrite state and progress,
- rewrites facts in place,
- explicitly manages the index lifecycle,
- disables semantic retrieval in normal startup when the configured provider is not known-safe for the current database.

Pros:
- operator-safe without blue/green complexity;
- durable progress / ETA / retry behavior;
- keeps normal runtime simple;
- preserves compatibility with historical databases and historical migrations.

Cons:
- requires a small embedding control plane in storage;
- adds one more CLI mode.

**Decision:** adopt Option C.

---

## 5. Hard constraints

### 5.1 Historical migrations stay frozen

The design MUST NOT edit:

- `migrations/__Initial.surql`
- `migrations/008_fact_semantic_embeddings.surql`

Any required schema change lands in a new migration only.

### 5.2 Normal MCP runtime remains normal

`serve` and `watch` must not silently perform a long-running rewrite. The rewrite belongs to a dedicated maintenance mode.

### 5.3 Public MCP contract remains unchanged

No new MCP tools are introduced for rewrite, progress, or maintenance orchestration.

### 5.4 Compatibility wins over optimistic semantics

When the repository cannot prove that the configured provider/model matches the stored embedding corpus, it should prefer:
- continuing with lexical / graph retrieval only,
- logging an explicit warning,

over attempting semantic retrieval with stale vectors.

### 5.5 Debuggability is a first-class requirement

The maintenance flow MUST be observable enough to answer all of the following from logs alone:

- which provider/model/dimension the process resolved;
- why semantic retrieval was enabled, bootstrapped, or disabled;
- whether a `reembed` run started fresh or resumed;
- which namespace and phase the job reached before failure;
- whether a failed fact was retried or skipped on resume;
- whether index drop / recreate succeeded;
- what exact error and cursor position caused the run to stop.

This observability requirement applies to normal startup, `reembed`, and restart recovery.

### 5.6 All facts means all facts

The rewrite scope includes:
- active facts,
- invalidated facts,
- historical facts reachable via `as_of` queries.

This is necessary because the repository supports bi-temporal retrieval and old facts remain relevant for past-time views.

---

## 6. Chosen architecture

### 6.1 CLI surface

Add a dedicated maintenance subcommand:

`memory_mcp reembed`

This is preferred over a long modifier flag such as `--force-embedding-recreation` because:

- the action is operationally separate from `serve`;
- it matches the existing CLI shape (`watch` is already a subcommand);
- it expresses operator intent clearly;
- it leaves room for future maintenance-only extensions without polluting normal startup.

`serve` and `watch` continue to mean “normal runtime.” `reembed` is the only mode allowed to rewrite stored embeddings.

### 6.2 Target signature and resolved dimension

Define a deterministic `embedding_signature` from non-secret provider identity fields plus the **resolved** embedding dimension:

- provider kind;
- model name;
- normalized base URL for remote providers;
- resolved dimension.

API keys must never participate in the signature.

The resolved dimension should be determined automatically whenever the operator did **not** explicitly set `SURREALDB_EMBEDDING_DIMENSION`.

Auto-detect rules:

- `local-candle` — derive the dimension from loaded model metadata (preferred) or, if needed, from a single probe embedding generated during provider bootstrap;
- remote providers (`openai-compatible`, `ollama`) — issue one probe embedding request during provider bootstrap and set the resolved dimension from the returned vector length;
- explicit `SURREALDB_EMBEDDING_DIMENSION` remains supported as a strict override / validation guard.

If an explicit dimension override is present and the provider returns a different vector length, startup or `reembed` must fail fast with a clear error instead of silently trusting the provider.

The signature becomes the single comparison key used for:

- detecting stale facts;
- deciding whether startup may safely enable semantic retrieval;
- making reruns idempotent;
- resuming interrupted maintenance runs;
- explaining what changed in operator logs.

### 6.3 Storage metadata

#### 6.3.1 `fact` additions

Add the following optional fields to `fact`:

- `embedding_provider: option<string>`
- `embedding_model: option<string>`
- `embedding_dimension: option<int>`
- `embedding_updated_at: option<datetime>`
- `embedding_signature: option<string>`

These are **storage metadata only**. They are not added to the public MCP response surface.

#### 6.3.2 `embedding_state` table (per namespace)

Create one row per namespace with record id `embedding_state:fact`.

Fields:

- `status: string` (`ready`, `rebuilding`, `failed`)
- `active_signature: option<string>`
- `provider: option<string>`
- `model: option<string>`
- `dimension: option<int>`
- `last_job_id: option<string>`
- `updated_at: datetime`

This row answers one runtime question:

> Is semantic retrieval safe to enable for this namespace under the current config?

#### 6.3.3 `embedding_job` table (control plane)

Persist one cross-namespace maintenance record in the default namespace (that is, the repository's current `default_namespace()`, which today resolves to the first configured namespace).

Fields:

- `job_id`
- `status` (`running`, `completed`, `failed`)
- `requested_at`
- `started_at`
- `updated_at`
- `finished_at`
- `target_signature`
- `provider`
- `model`
- `dimension`
- `namespaces`
- `total_facts`
- `processed_facts`
- `succeeded_facts`
- `failed_facts`
- `facts_per_second`
- `eta_seconds`
- `current_namespace`
- `namespace_progress`
- `last_error`

This table is not a public MCP feature. It exists solely for operator safety, progress persistence, retry behavior, and diagnostics.

Normal startup safety does **not** depend on reading `embedding_job`. Runtime enablement decisions come only from per-namespace `embedding_state`. `embedding_job` is for the maintenance runner, restart recovery, and operator diagnostics.

`namespace_progress` stores per-namespace counters and the most recent stable cursor, for example:

```json
{
    "org": {
        "processed": 12480,
        "succeeded": 12470,
        "failed": 10,
        "last_completed_fact_id": "fact:abc123"
    },
    "personal": {
        "processed": 320,
        "succeeded": 320,
        "failed": 0,
        "last_completed_fact_id": "fact:def456"
    }
}
```

`namespace_progress` is a structured object keyed by namespace. `last_completed_fact_id` means the last fact durably rewritten to the target signature. It must never advance past a failed row.

### 6.4 Normal startup behavior

Normal `serve` and `watch` never rewrite embeddings.

After migrations are applied, startup evaluates `embedding_state` across **all** configured namespaces:

- if every namespace is `ready` and `active_signature == current_signature`, semantic retrieval is enabled normally;
- if any namespace is `rebuilding`, semantic retrieval is disabled globally;
- if any namespace is `failed`, semantic retrieval is disabled globally;
- if any namespace is `ready` but its `active_signature != current_signature`, semantic retrieval is disabled globally and startup logs that `memory_mcp reembed` is required.

Startup must evaluate **every configured namespace individually**. Mixed cases are legal: one namespace may already have persisted `embedding_state`, while another may still be legacy and need bootstrap. The presence of metadata in one namespace must not be treated as proof that all namespaces are already migrated.

Recommended startup sequence:

1. apply migrations in every configured namespace;
2. load `embedding_state`, fact counts, and legacy embedding samples for every configured namespace;
3. resolve the target embedding dimension and target signature from current config plus provider preflight;
4. classify every namespace as `ready`, `bootstrap-ready`, or `rebuild-required`;
5. if any namespace is `rebuild-required`, disable semantic retrieval globally;
6. otherwise enable semantic retrieval and bootstrap only the namespaces that still lack ready state.

If target identity / dimension preflight fails during normal `serve` or `watch` startup, the process should degrade to lexical / graph retrieval only and log the error. That preflight failure is fatal only in `reembed` mode, where a valid target identity is mandatory.

Disabling semantic retrieval means:

- keep the process available;
- substitute a disabled embedding provider for runtime query embedding generation;
- continue lexical / graph retrieval;
- avoid mixed-space or mixed-dimension ANN behavior.

This is the compatibility guard that keeps existing databases usable even while a maintenance rewrite is pending or incomplete.

### 6.5 Legacy bootstrap behavior

Existing databases do not yet have `embedding_state` or `embedding_signature` metadata, so the first startup after this feature lands needs an explicit compatibility rule.

Bootstrap algorithm per namespace:

1. If the namespace has no facts, create `embedding_state:fact` as `ready` for the current signature.
2. Otherwise, sample a bounded deterministic prefix of facts with non-null `embedding` values in `fact_id ASC` order and compute the observed vector lengths from stored arrays.
3. If that sample is empty, has mixed lengths, or any sampled length differs from the current **resolved** dimension, do **not** mark the namespace ready; disable semantic retrieval and require `reembed`.
4. If every sampled embedding length matches the current **resolved** dimension, bootstrap `embedding_state:fact` to `ready` with the current signature and log a warning that legacy provider identity was assumed from current config.

Important limitation:

- historical same-dimension provider/model switches on pre-metadata databases cannot be inferred automatically. The repository can compare vector length, but it cannot recover the old provider/model identity after the fact.
- therefore, for legacy databases, a same-dimension provider switch still requires operator discipline: if the provider/model changed, run `memory_mcp reembed` explicitly.

This trade-off is acceptable because it is explicit, documented, and still preserves database compatibility.

### 6.6 Migration compatibility and index lifecycle

#### 6.6.1 Historical migrations remain historical

Add a new migration `019_embedding_rebuild_maintenance.surql` for all new schema.

Do not repurpose historical migration files to encode current embedding state.

#### 6.6.2 Targeted compatibility fix for migration `008`

Checksum validation should remain strict for all normal migrations.

However, `008_fact_semantic_embeddings.surql` is special: its historical applied checksum may legitimately differ between databases because the dimension placeholder was rendered with the runtime dimension at the time it first ran.

Target rule:

- keep strict checksum validation for every migration except known dimension-rendered embedding migrations;
- accept an existing `008_fact_semantic_embeddings.surql` bookkeeping record even if the currently rendered checksum differs;
- move current index shape management out of migration bookkeeping and into explicit runtime reconcile.

This is a narrow compatibility exception, not a general weakening of migration safety.

#### 6.6.3 Explicit index reconcile inside `reembed`

`reembed` owns the index lifecycle:

1. mark all namespace states `rebuilding`;
2. remove `fact_embedding_hnsw` in every namespace;
3. rewrite every fact to the target signature;
4. only if **every** fact succeeds, recreate `fact_embedding_hnsw` with the target dimension in every namespace;
5. mark all namespace states `ready`.

If any fact fails:

- do not recreate the index;
- leave namespace state as `failed`;
- require the operator to rerun `reembed` after fixing provider availability or configuration.

This avoids partial mixed-dimension indexes and keeps failure handling simple.

### 6.7 Reembed algorithm

1. Load config, resolve the target embedding provider, and auto-detect the target dimension when no explicit override is set (maintenance mode bypasses normal startup gating).
2. Compute `target_signature` from provider identity plus resolved dimension.
3. Refuse to run if embeddings are disabled.
4. Acquire a single active maintenance job in the default namespace; reject concurrent `reembed` runs. If a persisted running/failed job is resumed, require its stored `namespaces` set to match the current configured namespaces exactly.
5. Count the current rewrite workset (`embedding_signature IS NONE OR embedding_signature != $target_signature`) across all namespaces and persist that as `total_facts`; on resume, reuse the stored total for stable percent/ETA reporting.
6. Upsert the control-plane job record and set namespace states to `rebuilding`.
7. Drop the embedding index in every namespace.
8. For each namespace, repeatedly select the next batch of facts where `embedding_signature IS NONE OR embedding_signature != $target_signature`, ordered by `fact_id ASC`, using the per-namespace `last_completed_fact_id` cursor stored in `namespace_progress`.
9. Recompute the canonical embedding input using the same text format used by normal fact creation.
10. Update the record in place:
    - `embedding`
    - `embedding_provider`
    - `embedding_model`
    - `embedding_dimension`
    - `embedding_updated_at`
    - `embedding_signature`
11. Advance the stable cursor only after a fact rewrite is durably persisted. A failed row must not move the cursor past itself.
12. Periodically persist progress and emit progress logs.
13. If all batches succeed, recreate indexes and mark the job completed.
14. If any row fails, persist failure details, stop the current run, mark the job failed, keep indexes absent, and require a rerun.

### 6.8 Canonical embedding input

Normal write path and maintenance rewrite must use the same canonical input builder.

Target rule:

- factor the current `format!("{fact_type}\n{content}\n{quote}")` logic into one helper;
- `add_fact()` and `reembed` both call that helper;
- no duplicated string-building logic remains.

This guarantees that a freshly created fact and a rewritten fact are embedded from identical text.

### 6.9 Structured operational logging, progress, and ETA

The design requires structured logs for every meaningful operation in the startup and maintenance flow. This is broader than periodic progress reporting.

#### 6.9.1 Logging model

Use the repository's existing structured logger shape:

- one event per meaningful operation;
- stable `op=...` names suitable for grep and log-based debugging;
- log levels that distinguish normal milestones from high-volume trace detail;
- machine-parsable key/value fields rather than prose-only messages.

Required common fields where applicable:

- `op`
- `job_id`
- `namespace`
- `target_signature`
- `provider`
- `model`
- `target_dimension`
- `status`
- `duration_ms`
- `reason`
- `error`

Fields may be omitted only when they are genuinely unknown for that event.

#### 6.9.2 Minimum event coverage

Normal startup must log:

- embedding target preflight start / success / failure;
- per-namespace startup state inspection;
- the final startup decision (`enabled`, `bootstrap namespaces`, `disabled`);
- legacy bootstrap writes to `embedding_state`.

`reembed` must log:

- command start;
- job acquisition (`fresh` vs `resume`);
- namespace phase start / finish;
- workset counts;
- index drop / recreate attempts and outcomes;
- batch fetches with cursor position and batch size;
- stable cursor advancement;
- row rewrite failures with `fact_id`;
- terminal job completion or failure summary.

The logging requirement is intentionally stronger than "only log failures". A successful but surprising run must still be explainable.

#### 6.9.3 Terminal summary event

After `reembed` finishes, logs must contain one terminal summary event for the whole run:

- `reembed.job_completed` on success;
- `reembed.job_failed` on terminal failure.

That summary must include at least:

- `job_id`
- `status`
- `started_at` / `finished_at` or `duration_ms`
- `processed_facts`
- `succeeded_facts`
- `failed_facts`
- `total_facts`
- `provider`
- `model`
- `target_dimension`
- `target_signature`
- whether the run was `fresh` or `resumed`
- effective average throughput (facts/sec)

The final summary should let an operator answer, from a single log line, what target was applied, how much work was done, how long it took, and whether the run finished cleanly.

#### 6.9.4 Log-level policy

Recommended level policy:

- `info` — operator-visible milestones, job start/resume, namespace start/end, periodic progress, completion, final disablement reason;
- `debug` — startup classification inputs/outputs, sampled legacy dimensions, batch boundaries, workset counts, index DDL attempts, persisted control-plane writes;
- `trace` — per-fact rewrite attempts, per-fact skip reasons, stable cursor movement;
- `warn` — degraded startup, retryable provider/storage issues, failed fact rewrites, failed index operations that leave the system in lexical / graph-only mode;
- `error` — process-level aborts that terminate the current `reembed` run.

Per-fact success logging should remain `trace` to avoid overwhelming normal operator logs while still making deep debugging available.

#### 6.9.5 Redaction and payload safety

Logs must **not** include:

- API keys or auth headers;
- raw embedding vectors;
- full source document bodies;
- large opaque payload dumps when a compact identifier or count is enough.

Use identifiers and summaries instead, for example:

- `fact_id`
- character count / token count
- namespace
- signature
- dimension
- provider/model labels
- sampled vector length

The goal is high debuggability without leaking secrets or making logs unusably noisy.

#### 6.9.6 Progress and ETA

`reembed` must emit operator-friendly progress in both persisted state and stdout logs.

Required visible fields:

- percent complete;
- `processed / total`;
- `succeeded / failed`;
- current namespace;
- throughput in facts per second;
- ETA.

Recommended log shape:

`op=reembed.progress percent=23.4 processed=12480 total=53291 ok=12470 failed=10 rate_fps=181.2 eta_seconds=222`

ETA should be derived from a smoothed throughput estimate (EWMA or sliding window), not from a single early snapshot.

`total` refers to the persisted rewrite workset captured at job start, not an unfiltered count of every fact row in the namespace.

Recommended formatting:

- round `percent` to one decimal place;
- round `rate_fps` to one decimal place for logs;
- keep `eta_seconds` as an integer.

Update cadence:

- every N facts (for example 250), or
- every T seconds (for example 3),

whichever happens first.

### 6.10 Resume and idempotence

The rewrite must be safe to resume after interruption, including:

- process crash;
- lost DB connection;
- machine reboot;
- OOM kill;
- operator restart.

Resume rule:

- `reembed` persists a stable `last_completed_fact_id` cursor per namespace for forward progress inside a running job;
- if a new process resumes a job with the same target signature and the same configured namespace set, the runner reloads `embedding_job` + `namespace_progress` from the database and continues from the stored cursor;
- if a row failed in the previous run, the stable cursor remains at the last successful row, so the failed row is retried instead of being skipped;
- if a failed job is restarted from the beginning, already rewritten facts are still skipped automatically because their `embedding_signature == target_signature`.

This gives:

- safe retries after process crash;
- idempotent reruns for the same target signature;
- simple operator UX (“run `memory_mcp reembed` again”).

### 6.11 Module boundaries

Repository-fit DDD split:

- `src/cli.rs` / `src/main.rs` — parse and dispatch `reembed`
- `src/config/embedding.rs` — parse operator overrides and signature helpers
- `src/service/embedding.rs` — resolve target identity / dimension and construct the runtime provider
- `src/service/startup.rs` / `src/service/core/builder.rs` — startup gating and legacy bootstrap
- `src/service/reembed.rs` — orchestration of the maintenance job
- `src/storage/queries.rs` — batch selection, counts, and index DDL builders
- `src/storage/client.rs` / `src/storage/migrations.rs` — migration compatibility and DB execution details
- `migrations/019_embedding_rebuild_maintenance.surql` — new schema only

No new public MCP tool is added.

---

## 7. Out of scope

This design intentionally does **not** include:

- online / live rewrite while `serve` continues semantic retrieval;
- blue/green dual embedding fields;
- per-provider historical vector retention;
- new MCP tools for maintenance or progress;
- cross-process distributed workers;
- automatic inference of same-dimension provider switches on legacy pre-metadata databases.

---

## 8. Acceptance criteria for the implementation plan

The implementation plan derived from this design must verify at least:

1. `memory_mcp reembed` exists as a dedicated CLI mode.
2. No existing migration files are edited; only a new migration is added.
3. Existing databases no longer fail startup solely because `008_fact_semantic_embeddings.surql` was historically applied with a different rendered dimension.
4. `serve` and `watch` remain available during pending or failed rebuilds by falling back to lexical / graph retrieval without semantic search.
5. Every `fact` row, including invalidated facts, is rewritten to the new embedding signature during a successful run.
6. Progress output shows percent, counts, and ETA.
7. When `SURREALDB_EMBEDDING_DIMENSION` is unset, the target dimension is auto-detected from the provider/model and used consistently for signatures, metadata, and index rebuild.
8. A new process started after interruption resumes remaining work from persisted job state, replays the first failed row if necessary, and/or skips already rewritten facts for the same signature.
9. `fact_embedding_hnsw` is recreated only after a fully successful run.
10. The public MCP surface does not change.
11. Structured logs make it possible to reconstruct startup decisions, namespace transitions, cursor movement, retries, and terminal failures without adding ad hoc debug prints.
12. README/operator docs explain the maintenance workflow, auto-detect behavior, and the legacy bootstrap limitation.

---

## 9. Decision summary

Adopt Option C: a dedicated `reembed` maintenance mode backed by persisted state, targeted migration compatibility for historical dynamic index migration `008`, startup safety that disables semantic retrieval when a rebuild is pending, and an idempotent batch rewrite keyed by `embedding_signature`.

This keeps the binary simple in normal operation, gives operators a clear path after provider/model switches, preserves compatibility with existing databases, and avoids expanding the MCP API just to support one maintenance workflow.
