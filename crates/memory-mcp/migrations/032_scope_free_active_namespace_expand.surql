-- Migration 032: expand legacy partition fields for the one Active Namespace model.
--
-- The active namespace is now selected by the bound SurrealDB connection. Request
-- scope, project, and visibility_scope are no longer accepted by new writers.
-- Existing records retain those fields as legacy operational metadata for audit
-- and upgrade compatibility, but the fields must be optional so scope-free
-- records can be written to the existing SCHEMAFULL tables.
--
-- This migration is deliberately additive/expanding: it does not remove fields,
-- rewrite source episodes or facts, enumerate namespaces, or copy data between
-- namespaces. It applies independently in each namespace when that namespace is
-- selected by a server process.

-- Source evidence tables: preserve old values, permit new scope-free records.
DEFINE FIELD OVERWRITE scope ON episode TYPE option<string>;
DEFINE FIELD OVERWRITE visibility_scope ON episode TYPE option<string>;
DEFINE FIELD OVERWRITE scope ON fact TYPE option<string>;

-- Claim-derived records: old values remain readable but are no longer required
-- for new claim projections or relation records.
DEFINE FIELD OVERWRITE scope ON claim TYPE option<string>;
DEFINE FIELD OVERWRITE project ON claim TYPE option<string>;
DEFINE FIELD OVERWRITE scope ON claim_relation TYPE option<string>;
DEFINE FIELD OVERWRITE project ON claim_relation TYPE option<string>;

-- Legacy lifecycle/procedure metadata is retained without making it a write-time
-- partition requirement. These tables are schema-less today, but explicit
-- optional definitions keep the compatibility contract observable.
DEFINE FIELD OVERWRITE scope ON memory_event TYPE option<string>;
DEFINE FIELD OVERWRITE project ON memory_event TYPE option<string>;
DEFINE FIELD OVERWRITE scope ON event_projection_job TYPE option<string>;
DEFINE FIELD OVERWRITE project ON event_projection_job TYPE option<string>;
DEFINE FIELD OVERWRITE scope ON memory_capture_audit TYPE option<string>;
DEFINE FIELD OVERWRITE project ON memory_capture_audit TYPE option<string>;
DEFINE FIELD OVERWRITE scope ON procedure_candidate TYPE option<string>;
DEFINE FIELD OVERWRITE project ON procedure_candidate TYPE option<string>;
DEFINE FIELD OVERWRITE scope ON entity_extraction_projection TYPE option<string>;
DEFINE FIELD OVERWRITE scope ON query_log TYPE option<string>;

-- No data rewrite is intentional. The Active Namespace and database context are
-- the physical boundary; migration bookkeeping is handled by the application.
