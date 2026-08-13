-- Migration 034: make edge provenance schema-valid on SurrealDB 3.
--
-- The original initial schema used `TYPE object FLEXIBLE` on the relation table.
-- SurrealDB rejects FLEXIBLE on this table shape, so the initial statement was
-- reported as a compatibility conflict and left the parent field undefined.
-- Keep the nested provenance fields from migration 022 and add the valid parent
-- object definition without rewriting existing edge evidence.
DEFINE FIELD OVERWRITE provenance ON edge TYPE object;
