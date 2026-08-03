# Capability Seam Completion Plan

> Status: ✅ Executed (2026-08-03) via Card 6 of `2026-08-01-architecture-hardening-round-2.md` — capabilities are now `pub`, all consumers call capability structs directly, and the `MemoryService` delegates are removed.
> Related: ADR-0016 AD-2 (frozen public surface)
> Audit candidate: 1 (finish the capability seam)

## Context

`MemoryService` is a 22-field struct with 231 pub/pub(crate) methods across `core.rs`, `reembed.rs`, `core/builder.rs`, plus free functions in `context.rs`, `episode.rs`, `fact.rs`, `ingestion.rs`, `explanation.rs` that accept `&MemoryService`. Every tool in `src/tools/` takes `&MemoryService` directly — except `tools/invalidate.rs`, which already delegates to `InvalidateCapability::invalidate(&ServiceContext, ...)`.

`ServiceContext` is the intended seam: a narrow struct holding only the
infrastructure a capability needs. Its doc says it exists "to avoid exposing
the full `MemoryService` surface." But it is built and almost unused — only 1
of 6 core operations migrated.

The good news: the domain logic has **already** been extracted into narrow
services — `IngestionService`, `ExplanationService`, `EntityResolver`,
`FactService`, `EntityService`. `MemoryService::ingest` just delegates to
`self.ingestion_service.ingest(...)`. The remaining friction is purely at the
**interface**: tools reach past the seam into the god-object.

## Audit: what tools actually call

`src/tools/*.rs` call exactly 8 methods on `MemoryService`:

| Method | Delegates to | ServiceContext field needed |
|--------|-------------|------------------------------|
| `ingest` | `self.ingestion_service` | `ingestion_service` |
| `extract` | (inline in core.rs) | `entity_extractor`, `embedding_provider`, `triple_extractor` |
| `resolve` | `self.entity_resolver` | `entity_resolver`, `entity_service` |
| `assemble_context` | (free fn in context.rs) | `context_cache`, `embedding_provider`, `query_embedding_cache` |
| `explain` | `self.explanation_service` | `explanation_service` |
| `invalidate` | `InvalidateCapability` (already migrated) | — |
| `log_tool_event` | `self.logger` | `logger` |
| `log_tool_event_with_duration` | `self.logger` | `logger` |

`ServiceContext` currently holds: `db_client`, `namespaces`, `rate_limiter`,
`context_cache`, `claim_store`. To migrate the remaining 5 capabilities, it
needs: `logger`, `ingestion_service`, `explanation_service`, `entity_resolver`,
`entity_service`, `entity_extractor`, `embedding_provider`,
`query_embedding_cache`, `triple_extractor`, `default_namespace`.

## Plan

### Principle

This is a **finish-what-we-started** migration, not a greenfield redesign. The
pattern is proven by `InvalidateCapability`. Each step is independently
mergeable and testable.

### Step 1 — Extend `ServiceContext`

Add the fields the unmigrated capabilities need. Constructed by
`MemoryService::build_context()` (already exists, just add fields).

```rust
pub struct ServiceContext {
    // existing
    pub db_client: Arc<dyn DbClient>,
    pub namespaces: Vec<String>,
    pub rate_limiter: Arc<RateLimiter>,
    pub context_cache: Arc<RwLock<LruCache<CacheKey, Vec<AssembledContextItem>>>>,
    pub claim_store: Option<Arc<dyn ClaimStore>>,
    // new
    pub default_namespace: String,
    pub logger: StdoutLogger,
    pub ingestion_service: IngestionService,
    pub explanation_service: ExplanationService,
    pub entity_resolver: EntityResolver,
    pub entity_service: EntityService,
    pub entity_extractor: Arc<dyn EntityExtractor>,
    pub embedding_provider: Arc<dyn EmbeddingProvider>,
    pub query_embedding_cache: Arc<Mutex<LruCache<String, CachedQueryEmbedding>>>,
    pub triple_extractor: Arc<dyn TripleExtractor>,
}
```

### Step 2 — Migrate `assemble_context` (highest-value)

`assemble_context` is the largest capability and the hardest to test today
(requires full `MemoryService` bootstrap). Move it to
`AssembleContextCapability::assemble_context(&ServiceContext, request)`.

The function already lives in `context.rs` as a free function taking
`&MemoryService`. The migration changes the signature to `&ServiceContext` and
updates the `pipeline::*` helpers it calls to take `&ServiceContext` too (they
already take `service: &MemoryService` — same change).

**Test payoff:** `assemble_context` becomes testable with a bare
`MockDbClient` + the embedding/extractor handles, no full service.

### Step 3 — Migrate `ingest`, `explain`, `resolve`

These are the easiest: `MemoryService::ingest/explain/resolve` already delegate
to `IngestionService`/`ExplanationService`/`EntityResolver`. The capability just
takes `&ServiceContext`, calls `enforce_rate_limit`, logs, and delegates.

```text
IngestCapability::ingest(&ServiceContext, IngestRequest, access) -> Result<String>
ExplainCapability::explain(&ServiceContext, ExplainRequest, access) -> Result<Vec<ExplainItem>>
ResolveCapability::resolve(&ServiceContext, EntityCandidate, access) -> Result<String>
```

### Step 4 — Migrate `extract`

`extract` is the most coupled (inline in `core.rs`, uses entity_extractor,
embedding, triple_extractor). Move to `ExtractCapability::extract` taking
`&ServiceContext`. The `episode::extract_from_episode` helper currently takes
`&MemoryService` — change to `&ServiceContext`.

### Step 5 — Update `tools/*.rs`

Each tool switches from `service: &MemoryService` to `ctx: &ServiceContext` and
calls the matching capability. `tools/invalidate.rs` is the reference.

### Step 6 — Slim `MemoryService`

> **Status: Deferred.** `MemoryService` methods (`ingest`, `extract`, `resolve`,
> `explain`, `assemble_context`, `add_fact`) were kept as thin delegators
> (`let ctx = self.build_context(); Capability::method(&ctx, ...).await`)
> for backward compatibility with internal callers (`commit_ingestion_review`,
> lifecycle worker, MCP handlers). The god-object surface did not collapse to
> ~10 methods; it stayed at ~30. This is a defensible deviation — removing the
> delegators would require updating all internal callers to use `build_context()`
> + capability directly. File a follow-up to complete this if the method count
> becomes a maintenance burden.

After migration, `MemoryService` keeps only:
- construction (`builder.rs`)
- `build_context()` (the seam constructor)
- worker lifecycle (`start_claim_workers`, `start_lifecycle_worker`,
  `lifecycle_capture`)
- `log_tool_event` / `log_tool_event_with_duration` (or move these to
  `ServiceContext::log`)

The 231 methods collapse to ~10. The god-object becomes a thin construction +
runtime holder.

## Sequencing

Each step is a standalone PR. Step 1 unblocks 2–4. Steps 2–4 can be done in any
order. Step 5 requires all of 2–4. Step 6 is cleanup after 5.

```text
Step 1 (extend ServiceContext)
   │
   ├── Step 2 (assemble_context)
   ├── Step 3 (ingest/explain/resolve)
   └── Step 4 (extract)
         │
         └── Step 5 (update tools/)
               │
               └── Step 6 (slim MemoryService)
```

## ADR needed?

No. This does not contradict ADR-0016 AD-2 (internal capabilities call "the same
service/tool modules"). `InvalidateCapability` already proves the pattern. No
new public surface, no new decision — just finishing an established migration.

## Risk

- `extract` (Step 4) is the most invasive — it touches `episode.rs` helpers that
  take `&MemoryService`. Do it last, after Steps 2–3 prove the pattern at scale.
- `assemble_context` pipeline has many helpers (`pipeline::*`,
  `ranking::*`, `semantic::*`) that all take `&MemoryService`. The signature
  change is mechanical but touches many files — do it in one focused PR.
