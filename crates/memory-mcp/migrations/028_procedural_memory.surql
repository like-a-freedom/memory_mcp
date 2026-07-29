-- Migration 028: Procedural memory candidates (gated).
--
-- This migration is append-only and adds procedure candidate storage. The
-- procedure gate (see docs/evals/AGENT_MEMORY_LIFECYCLE.md) must pass before
-- procedure promotion is enabled. Until then, candidates remain shadow-only.
--
-- Candidates derive only from accepted lesson evidence linked to trusted
-- outcomes. They group deterministically, append evidence, derive a Beta
-- posterior from counts, and never auto-promote.

DEFINE TABLE OVERWRITE procedure_candidate SCHEMALESS;

DEFINE FIELD OVERWRITE candidate_id ON procedure_candidate TYPE string;
DEFINE FIELD OVERWRITE namespace ON procedure_candidate TYPE string;
DEFINE FIELD OVERWRITE scope ON procedure_candidate TYPE string;
DEFINE FIELD OVERWRITE project ON procedure_candidate TYPE option<string>;
DEFINE FIELD OVERWRITE task_fingerprint ON procedure_candidate TYPE string;
DEFINE FIELD OVERWRITE normalized_task ON procedure_candidate TYPE string;
DEFINE FIELD OVERWRITE status ON procedure_candidate TYPE string;
DEFINE FIELD OVERWRITE trust_floor ON procedure_candidate TYPE string;
DEFINE FIELD OVERWRITE success_count ON procedure_candidate TYPE int DEFAULT 0;
DEFINE FIELD OVERWRITE failure_count ON procedure_candidate TYPE int DEFAULT 0;
DEFINE FIELD OVERWRITE evidence_count ON procedure_candidate TYPE int DEFAULT 0;
DEFINE FIELD OVERWRITE origin_kind ON procedure_candidate TYPE string;
DEFINE FIELD OVERWRITE created_at ON procedure_candidate TYPE string;
DEFINE FIELD OVERWRITE updated_at ON procedure_candidate TYPE string;
DEFINE FIELD OVERWRITE promoted_at ON procedure_candidate TYPE option<string>;
DEFINE FIELD OVERWRITE deprecated_at ON procedure_candidate TYPE option<string>;
DEFINE FIELD OVERWRITE expires_at ON procedure_candidate TYPE option<string>;

DEFINE INDEX OVERWRITE procedure_candidate_id ON TABLE procedure_candidate COLUMNS candidate_id UNIQUE;
DEFINE INDEX OVERWRITE procedure_candidate_scope_project ON TABLE procedure_candidate COLUMNS scope, project;
DEFINE INDEX OVERWRITE procedure_candidate_status ON TABLE procedure_candidate COLUMNS status, updated_at;
