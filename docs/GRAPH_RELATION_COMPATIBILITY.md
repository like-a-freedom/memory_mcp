# Graph Relation Compatibility Note

## Intent

Task 5 upgrades graph persistence from flat edge rows to native SurrealDB relation records without breaking the existing MCP/service APIs.

## Naming strategy

The long-term target remains semantic relation tables such as:

- `mentioned_in`
- `involved_in`
- `knows`
- other relation-specific tables where the edge type is stable and queryable

## Compatibility strategy used now

The current service layer still accepts arbitrary relation labels at runtime, so this remediation wave keeps a **single physical relation table** named `edge` and stores the logical relation label in the `relation` field.

That gives us three immediate benefits:

1. native SurrealDB `RELATE` writes
2. compatibility with existing tests and IDs (`edge_id`, `from_id`, `to_id`, `relation`)
3. a migration path toward relation-specific tables later, once the public API and extraction pipeline are narrowed enough to make table names predictable

## Follow-up migration path

When Task 5 is revisited for relation-specific tables, migrate in this order:

1. keep `edge` as the compatibility/read model
2. introduce stable relation tables for known labels
3. dual-write during transition
4. switch traversal to relation-specific tables
5. remove the generic compatibility table only after backfill and verification
