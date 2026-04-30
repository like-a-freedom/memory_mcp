-- Allow per-namespace progress keys inside schemafull embedding_job records.

DEFINE FIELD OVERWRITE namespace_progress ON embedding_job TYPE object FLEXIBLE;
