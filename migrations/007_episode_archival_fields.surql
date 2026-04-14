-- Add explicit archival metadata fields for episodes.
--
-- Existing databases created before lifecycle archival shipped need both
-- fields declared so schemafull updates can succeed.

DEFINE FIELD OVERWRITE status ON episode TYPE option<string>;
DEFINE FIELD OVERWRITE archived_at ON episode TYPE option<datetime>;