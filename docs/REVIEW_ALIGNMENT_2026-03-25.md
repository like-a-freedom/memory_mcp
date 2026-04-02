# Review Alignment — 2026-03-25

**Last Updated:** 2026-03-27
**Status:** P0/P1 items complete. P2 (lifecycle, multi-source provenance) plans ready for execution.

This document records the line-by-line validation of the external review against the current repository state. It is intentionally implementation-focused: each item is marked as confirmed, partially confirmed, or not confirmed based on code inspection.

## 2026-03-27 Critical Fixes

| Issue | Status | Fix |
| --- | --- | --- |
| `namespace_for_scope("ORG")` silent fallback | ✅ Fixed | Now normalizes scope to lowercase before prefix matching; logs warn for unknown scopes |
| `select_entities_batch` unused in hot path | ✅ Not an issue | Already used in `expand_query_with_aliases()` for O(1) batch lookup |
| Entity aliases not normalized | ✅ Not an issue | Aliases normalized via `normalize_text()` at write time; `CONTAINSANY` works correctly |
| `cosine_similarity` silent fail in release | ✅ Fixed | Replaced `debug_assert` with `tracing::warn!` for production logging |
| Entity extraction lacks `person`/`technology` types | ✅ Fixed | Multi-word names → `person`; CamelCase single tokens → `technology` |

## Validation summary

| Review item | Status | Current evidence | Documentation consequence |
| --- | --- | --- | --- |
| Temporal fields are `TYPE string` instead of `TYPE datetime` | ✅ Resolved | `src/migrations/__Initial.surql` uses `datetime` / `option<datetime>` and `src/storage.rs::build_set_assignments()` coerces write payloads through `type::datetime(...)` | Remove stale temporal-schema gap claims |
| FTS index exists but is not used as the only retrieval path | ✅ Resolved | `src/storage.rs::select_facts_filtered()` is the lexical retrieval entry point; community/graph expansion implemented | Document retrieval as lexical/community/graph hybrid |
| Provenance is ignored in fact persistence | ✅ Resolved | `src/service/core.rs::add_fact()` persists the supplied `provenance` payload | Promote provenance persistence from roadmap to implemented |
| Provenance is ignored in edge persistence | ✅ Resolved | `src/service/episode.rs::store_edge()` persists edge provenance through `relate_edge()` | Same as above |
| Edge indexes on `from_id` / `to_id` are missing | ✅ Resolved | `src/migrations/__Initial.surql` now defines `edge_from_id` and `edge_to_id` | Remove stale index-gap claims |
| `find_entity_record()` performs full table scan | ✅ Resolved | `src/service/core.rs::find_entity_record()` now calls `select_entity_lookup()` | Promote indexed entity lookup to implemented |
| `explain()` is a pass-through adapter | ✅ Resolved | `src/service/core.rs::explain()` expands items back to source episodes and includes provenance/citation context | Promote explainability from partial to implemented baseline |
| `find_intro_chain()` loads all edges into memory before BFS | ✅ Resolved | `src/service/core.rs::find_intro_chain()` now uses DB-side neighbor lookups | Update graph traversal notes to reflect current pushdown |
| Edges are stored as flat records, not native `RELATE` edges | ✅ Resolved | `src/migrations/__Initial.surql` defines `edge TYPE RELATION`; `src/storage.rs::relate_edge()` uses `RELATE` | Promote native relation storage to implemented |
| Embeddings are absent | ⚠️ Superseded | Embedding fields were scaffolded but **removed** per `SIMPLIFIED_SEARCH_REDESIGN_SPEC.md` — lexical-first approach | Document semantic retrieval as intentionally removed |
| Entity extraction is regex-only and Anglo-centric | ✅ Resolved (2026-03-26) | `src/service/entity_extraction.rs` now uses Unicode-aware regex: `[\p{Lu}][\p{Ll}]+` for Cyrillic/Latin support | Promote Unicode extraction to implemented |
| Edge invalidation on conflict is not implemented | ✅ Resolved for active triple versions | `src/service/episode.rs::invalidate_conflicting_edges()` invalidates prior active versions before insert | Document remaining contradiction work as broader semantic follow-up |
| Community detection is placeholder-level and not used in retrieval | ✅ Resolved | `src/service/episode.rs::update_communities()` builds connected components and `src/service/context.rs` reads community summaries during retrieval | Promote community retrieval to implemented baseline |
| Migration checksum/versioning is not implemented | ✅ Resolved | `src/storage.rs::apply_migrations_impl()` records `script_name`, `checksum`, and `executed_at`, and rejects modified applied migrations | Promote migration bookkeeping to implemented |
| `Mutex<Surreal<Db>>` / `Mutex<Surreal<Client>>` serializes DB access | ⚠️ Still confirmed | `src/storage.rs::DbEngine` still wraps both engines in `tokio::sync::Mutex` | Keep concurrency limitation documented |
| 10K edge limit silently truncates community detection | ✅ Resolved (2026-03-26) | Warning logged at `db.select_edges_filtered.limit_hit` when limit reached | Document operational monitoring requirement |
| No background jobs for lifecycle (decay, archival) | 📋 Planned (2026-03-26) | Implementation plan: `docs/superpowers/plans/2026-03-26-lifecycle-background-jobs.md` | Execute lifecycle plan |
| Multi-source provenance in explain() | 📋 Planned (2026-03-26) | Implementation plan: `docs/superpowers/plans/2026-03-26-multi-source-provenance.md` | Execute provenance plan |

## What changed in the specification

The main changes now reflected in `docs/MEMORY_SYSTEM_SPEC.md` are:

- preserved the remaining partial gaps (richer extraction, lifecycle follow-up, security hardening, client-sharing throughput)
- promoted implemented remediation work: provenance persistence, indexed entity lookup, native `RELATE` edges, DB-side intro traversal, semantic scaffolding, community-aware retrieval, and checksum-enforced migrations
- updated local-operation and stdio host examples to match the current Rust workspace layout
- recorded verification commands and exact pass counts from this remediation pass

## Documentation-only conclusions

1. The external review was materially accurate at the start of the remediation pass.
2. Tasks 3-7 closed most of the correctness and graph/storage gaps called out by that review.
3. Documentation must still distinguish:
   - **implemented now**
   - **partially implemented / correctness gap**
   - **target architecture / roadmap**
4. The next engineering pass should focus on lifecycle automation, richer extraction quality, client-sharing throughput, and production deployment controls.

## Remaining Work (HEAD-aligned)

| Item | Status | Notes |
| --- | --- | --- |
| Entity → episode traversal bypasses `DbClient` | 📋 Planned (Wave 2) | Helper works, but storage logic still lives in `src/service/core.rs` |
| `fact.entity_links` still uses string IDs | 📋 Planned (Wave 3) | Migration to typed record refs should preserve compatibility |
| `embedding.rs` contains test-only dead helpers | 📋 Planned (Wave 4) | Remove only confirmed dead helpers, keep live providers |
| MCP tool descriptions still have friction points | 📋 Planned (Wave 5) | Improve descriptions and invalid-parameter guidance without breaking schema |
| Entity-graph expansion in `assemble_context` | 📋 Planned (Wave 6) | Wire NER → entity graph → fact lookup for multi-hop retrieval |
| `AssembledContextItem` lacks temporal fields | 📋 Planned (Wave 7) | Add `t_ref`/`t_valid` for LLM temporal reasoning |
| In-memory eval stability | 📋 Planned (Wave 8) | Per-batch recycling for SurrealDB embedded |

### Excluded from implementation

- GLiNER sigmoid before threshold — already fixed in `src/service/gliner_entity_extractor.rs`
- BM25 snowball analyzer — already present in migrations
- `suggested_next_action` — already covered by `guidance`
- `find_episodes_via_entity` stub claim — outdated; helper exists in `src/service/core.rs`
- Multi-hop reasoning chains — LLM-level concern; server provides 1-hop entity expansion (Wave 6)
- Abstention tuning — LongMemEval oracle has no abstention test cases (dataset artifact)

## Execution Options

All P0/P1 items from the independent review are now **complete**.

For future enhancements (P2/P3 follow-ups):

1. **Subagent-Driven** (recommended) — Fresh subagent per task with review checkpoints
2. **Inline Execution** — Batch execution with checkpoints in current session
