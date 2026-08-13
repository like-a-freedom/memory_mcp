-- Migration 033: persist the versioned claim identity contract.
--
-- New claim projections write v2 identity. Existing claims are not rewritten;
-- missing identity_version remains readable as legacy v1 in serde and lookup code.
DEFINE FIELD OVERWRITE identity_version ON claim TYPE option<string>;
