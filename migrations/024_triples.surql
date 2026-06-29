-- Migration 024: Semantic triple extraction table.
--
-- Stores structured (subject, predicate, object) triples extracted from facts.
-- Enables structured queries like "who works at X?" or "where does Y live?".

DEFINE TABLE triple SCHEMAFULL;
DEFINE FIELD namespace ON triple TYPE string;
DEFINE FIELD subject ON triple TYPE string;
DEFINE FIELD predicate ON triple TYPE string;
DEFINE FIELD object ON triple TYPE string;
DEFINE FIELD confidence ON triple TYPE float DEFAULT 1.0;
DEFINE FIELD source_fact_id ON triple TYPE string;
DEFINE FIELD t_ingested ON triple TYPE datetime VALUE time::now() READONLY;
DEFINE FIELD t_invalid ON triple TYPE option<datetime>;
DEFINE FIELD t_invalid_ingested ON triple TYPE option<datetime>;

DEFINE INDEX triple_subject_idx ON triple COLUMNS namespace, subject;
DEFINE INDEX triple_predicate_idx ON triple COLUMNS namespace, predicate;
DEFINE INDEX triple_spo_idx ON triple COLUMNS namespace, subject, predicate;
