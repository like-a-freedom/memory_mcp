# Review Alignment — 2026-03-25

This document records the line-by-line validation of the external review against the current repository state. It is intentionally implementation-focused: each item is marked as confirmed, partially confirmed, or not confirmed based on code inspection.

## Validation summary

| Review item | Status | Current evidence | Documentation consequence |
| --- | --- | --- | --- |
| Temporal fields are `TYPE string` instead of `TYPE datetime` | Confirmed | `src/migrations/__Initial.surql` declares `t_ref`, `t_ingested`, `t_valid`, `t_invalid`, and related fields as `string` / `option<string>` | Treat native datetime migration as P0; mark bi-temporal support as partial rather than done |
| FTS index exists but is not used as the real query path | Confirmed | `src/migrations/__Initial.surql` defines `fact_content_search`; `src/storage.rs::select_facts_filtered()` still executes `SELECT * FROM fact` and filters in Rust when `query_contains` is present | Downgrade FTS / `as_of` retrieval claims to partial |
| Provenance is ignored in fact persistence | Confirmed | `src/service/core.rs::add_fact()` takes `_provenance` and writes `"provenance": {}` | Mark auditability / explainability as partial |
| Provenance is ignored in edge persistence | Confirmed | `src/service/episode.rs::store_edge()` writes `"provenance": {}` | Same as above |
| Edge indexes on `from_id` / `to_id` are missing | Confirmed | Only `edge_relation` index exists in `src/migrations/__Initial.surql` | Add index work to first implementation wave |
| `find_entity_record()` performs full table scan | Confirmed | `src/service/core.rs::find_entity_record()` calls `select_table("entity", namespace)` and scans in Rust | Downgrade entity resolution claims to partial |
| `explain()` is a pass-through adapter | Confirmed | `src/service/core.rs::explain()` only maps `context_pack` to `ExplainItem` | Mark `explain` API as partial |
| `find_intro_chain()` loads all edges into memory before BFS | Confirmed | `src/service/core.rs::find_intro_chain()` loads `select_edges_filtered()` results into `HashMap<String, Vec<String>>` | Document scalability limitation and native graph roadmap |
| Edges are stored as flat records, not native `RELATE` edges | Confirmed | `src/service/episode.rs::store_edge()` writes records into `edge`; schema uses `DEFINE TABLE edge`, not `TYPE RELATION` | Clarify that graph support is currently logical, not native Surreal graph traversal |
| Embeddings are absent | Confirmed | No `embedding` fields in `src/models.rs` or `src/migrations/__Initial.surql`; no provider trait exists | Mark hybrid semantic retrieval as roadmap |
| Entity extraction is regex-only and Anglo-centric | Confirmed | `src/service/core.rs` compiles `[A-Z][a-z]+(?:\s+[A-Z][a-z]+)+`; `src/service/episode.rs::extract_entities()` resolves only `person` / `company` | Downgrade extraction completeness to partial |
| Edge invalidation on conflict is not implemented | Confirmed | `src/service/episode.rs::store_edge()` returns early when the deterministic edge already exists | Mark FR-GR-05 as not done |
| Community detection is placeholder-level and not used in retrieval | Confirmed | `src/service/episode.rs::update_communities()` groups entities from one episode; `src/service/context.rs` never reads `community` | Mark communities as partial and retrieval integration as pending |
| Migration checksum/versioning is not implemented | Confirmed | `script_migration.checksum` exists in schema, but `src/storage.rs::apply_migrations_impl()` only runs `__Initial.surql` | Mark migrations/versioning as partial |
| `Mutex<Surreal<Db>>` / `Mutex<Surreal<Client>>` serializes DB access | Confirmed | `src/storage.rs::DbEngine` wraps both engines in `tokio::sync::Mutex` | Capture concurrency limitation in roadmap |

## What changed in the specification

The main changes were applied to `docs/MEMORY_SYSTEM_SPEC.md`:

- corrected inflated `✅ Done` statuses to `⚠️ Partial` or `❌ Not done`
- added an explicit implementation reality-check section
- clarified that current context assembly is recency-first, not true hybrid retrieval
- clarified that `explain` is not yet provenance expansion
- moved embeddings, native `RELATE`, and checksummed migrations into explicit pending work

## Documentation-only conclusions

1. The external review is materially accurate and still current.
2. The repository has a solid Rust/MCP foundation, but several architectural claims in the spec described target-state behavior rather than implemented behavior.
3. Documentation must distinguish:
   - **implemented now**
   - **partially implemented / correctness gap**
   - **target architecture / roadmap**
4. The next engineering pass should start with correctness and observability fixes before semantic-layer expansion.
