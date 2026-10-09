# Query index coverage (CONTAINSANY + time-order) — Implementation Plan

**Goal:** Make the remaining context-assembly, explain and entity-resolution reads index-backed instead of full table scans, after the 2026-10-09 record-cast plan. Every claim here is an `EXPLAIN` measured on the embedded SurrealDB 3.3.0 the tests use.

## Evidence

`EXPLAIN` of the exact production queries (before this change):

| Read | Query shape | Plan |
|------|-------------|------|
| fact retrieval | `fact … WHERE <bi-temporal> ORDER BY t_valid DESC, fact_id ASC LIMIT n` | `TableScan [fact]` + Sort |
| active facts | `fact … ORDER BY t_valid ASC LIMIT n` | `TableScan [fact]` |
| facts by entity | `fact … WHERE <bi-temporal> AND entity_links CONTAINSANY $e ORDER BY t_valid DESC LIMIT n` | `TableScan [fact]` |
| episode fallback | `episode … ORDER BY t_ref DESC, episode_id ASC LIMIT n` | `TableScan [episode]` + Sort |
| edge page | `edge … ORDER BY in, out, t_valid DESC LIMIT n START s` | `TableScan [edge]` + Sort |
| communities by member | `community WHERE member_entities CONTAINSANY $m` | `TableScan [community]` |
| entity by alias | `entity WHERE aliases CONTAINS $a LIMIT 1` | `TableScan [entity]` |

Two root causes:

1. **A scalar index on a time column is not present**, so the `ORDER BY <time> LIMIT n` reads scan and sort the whole table.
2. **An array index declared `COLUMNS field` only serves exact-array equality** (`field = [...]`). It does **not** serve `CONTAINS` / `CONTAINSANY` / `INSIDE`. The supported form for membership is an **array-element index, `FIELDS field.*`** (SurrealDB ≥ 3.1), and only `CONTAINSANY` uses it — `CONTAINS` never uses an index. This is why the existing `entity_aliases` and `community_members` indexes are dead for their queries, and why the comment at `entity_store.rs:48` (“`CONTAINS` … is index-aware”) is false.

Verified after the fix: `fact_t_valid`, `episode_t_ref`, `edge_t_valid` (new), and `entity_aliases`/`community_members` rebuilt as `FIELDS x.*` all flip the rows above to `IndexScan`/`UnionIndexScan`. `fact.entity_links` needs no index of its own: with `fact_t_valid` present the planner range-scans that index and filters `entity_links`, so a second fact index would only add write cost.

## Change

**Migration 054 (`054_query_index_coverage.surql`)**

```surql
DEFINE INDEX IF NOT EXISTS fact_t_valid ON TABLE fact FIELDS t_valid;
DEFINE INDEX IF NOT EXISTS episode_t_ref ON TABLE episode FIELDS t_ref;
DEFINE INDEX IF NOT EXISTS edge_t_valid ON TABLE edge FIELDS t_valid;
DEFINE INDEX OVERWRITE entity_aliases ON TABLE entity FIELDS aliases.*;
DEFINE INDEX OVERWRITE community_members ON TABLE community FIELDS member_entities.*;
```

Registered in `storage::migrations::versioned_migrations()` after 053; `latest_registered_migration_is_expected` updated.

**Alias query** — `find_entity_id_by_alias` switches from `aliases CONTAINS $alias` to `aliases CONTAINSANY [$alias]`, built by a new `build_select_entity_by_alias_query` in `knowledge/queries.rs` (the owner of the SQL, matching its siblings) instead of an inline string in the store. Single-element `CONTAINSANY` is the same membership test.

## Tests (TDD)

- `tests/query_index_coverage.rs` (new): one `EXPLAIN` per read above asserting `IndexScan` and the expected index name; the alias one asserts `entity_aliases`. All fail before migration 054, pass after.
- `knowledge::queries` unit: `build_select_entity_by_alias_query` emits `aliases CONTAINSANY [$alias]` and binds `alias`.
- Existing behavior tests (`find_entity_id_by_alias_returns_matching_entity`, `find_overlapping_communities …`) stay green, pinning that the rewrite preserves results.

## Deferred (recorded, not done)

- `entity` prefix lookup (`string::starts_with(canonical_name_normalized, $p)`) is unindexable in SurrealDB; would need a different search structure.
- Entity↔fact links are stored twice — as `fact.entity_links` and as `involved_in` edges — and `add_fact` writes only the array while extraction writes both. Unifying them is a data-model change worth its own plan.

## Follow-up review pass (same day)

A review against the original timeout report added:

- **Readiness gate**: the five new indexes were added to `required_schema_indexes`, so a silent index-build failure now fails startup (`storage/migrations.rs`).
- **Request-deadline-aware retry** (`platform/request_budget.rs`): the HTTP deadline middleware installs an ambient deadline; the DB retry loop narrows each attempt to the time the request has left, so a stalled query answers within the budget instead of running out a fixed 30s attempt and retrying into cancellation. The client-side per-attempt timeout default is unchanged (30s) — lowering it to a p99 value is an operator decision deferred to the runbook, because a blanket change would risk slow maintenance queries.
- **Explain fan-out**: each distinct episode and fact id is now resolved once per `explain` call (was once per item, plus a second fact fetch in the provenance phase), via per-call caches in `memory/explanation.rs`. Parallelising phases (`join_all`) was not done: it would raise concurrent DB load, the opposite of the goal, and the dedup already removes the duplicate work.
- **Compose memory limits** (`docker-compose.yml`): `mem_limit` on both services, env-overridable (`SURREALDB_MEM_LIMIT`, `MEMORY_MCP_MEM_LIMIT`), addressing the host-swap part of the report.

