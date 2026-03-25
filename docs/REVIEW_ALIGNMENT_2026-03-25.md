# Review Alignment — 2026-03-25

This document records the line-by-line validation of the external review against the current repository state. It is intentionally implementation-focused: each item is marked as confirmed, partially confirmed, or not confirmed based on code inspection.

## Validation summary

| Review item | Status | Current evidence | Documentation consequence |
| --- | --- | --- | --- |
| Temporal fields are `TYPE string` instead of `TYPE datetime` | Resolved | `src/migrations/__Initial.surql` uses `datetime` / `option<datetime>` and `src/storage.rs::build_set_assignments()` coerces write payloads through `type::datetime(...)` | Remove stale temporal-schema gap claims |
| FTS index exists but is not used as the only retrieval path | Still confirmed | `src/storage.rs::select_facts_filtered()` remains the lexical retrieval entry point | Document retrieval as lexical/community/graph hybrid, not pure vector ranking |
| Provenance is ignored in fact persistence | Resolved | `src/service/core.rs::add_fact()` persists the supplied `provenance` payload | Promote provenance persistence from roadmap to implemented |
| Provenance is ignored in edge persistence | Resolved | `src/service/episode.rs::store_edge()` persists edge provenance through `relate_edge()` | Same as above |
| Edge indexes on `from_id` / `to_id` are missing | Resolved | `src/migrations/__Initial.surql` now defines `edge_from_id` and `edge_to_id` | Remove stale index-gap claims |
| `find_entity_record()` performs full table scan | Resolved | `src/service/core.rs::find_entity_record()` now calls `select_entity_lookup()` | Promote indexed entity lookup to implemented |
| `explain()` is a pass-through adapter | Resolved | `src/service/core.rs::explain()` expands items back to source episodes and includes provenance/citation context | Promote explainability from partial to implemented baseline |
| `find_intro_chain()` loads all edges into memory before BFS | Resolved | `src/service/core.rs::find_intro_chain()` now uses DB-side neighbor lookups | Update graph traversal notes to reflect current pushdown |
| Edges are stored as flat records, not native `RELATE` edges | Resolved | `src/migrations/__Initial.surql` defines `edge TYPE RELATION`; `src/storage.rs::relate_edge()` uses `RELATE` | Promote native relation storage to implemented |
| Embeddings are absent | Partially resolved | `src/models.rs`, `src/migrations/__Initial.surql`, and `src/service/embedding.rs` now provide embedding fields, indexes, and a `NullEmbedder` scaffold | Document semantic retrieval as scaffolded but disabled by default |
| Entity extraction is regex-only and Anglo-centric | Still confirmed | `src/service/entity_extraction.rs` keeps regex fallback as the default extractor | Keep extraction completeness marked partial |
| Edge invalidation on conflict is not implemented | Resolved for active triple versions | `src/service/episode.rs::invalidate_conflicting_edges()` invalidates prior active versions before insert | Document remaining contradiction work as broader semantic follow-up |
| Community detection is placeholder-level and not used in retrieval | Resolved | `src/service/episode.rs::update_communities()` builds connected components and `src/service/context.rs` reads community summaries during retrieval | Promote community retrieval from pending to implemented baseline |
| Migration checksum/versioning is not implemented | Resolved | `src/storage.rs::apply_migrations_impl()` records `script_name`, `checksum`, and `executed_at`, and rejects modified applied migrations | Promote migration bookkeeping from partial to implemented |
| `Mutex<Surreal<Db>>` / `Mutex<Surreal<Client>>` serializes DB access | Still confirmed | `src/storage.rs::DbEngine` still wraps both engines in `tokio::sync::Mutex` | Keep concurrency limitation documented |

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
