-- Optional explicit source lineage for episodes ingested by the filesystem
-- watcher. Present only for internal filesystem ingestion; the public ingest
-- path never sets it. Claim projection prefers this stable lineage over the
-- versioned episode `source_id`.
DEFINE FIELD source_lineage ON episode TYPE option<string>;
