-- Migration 035: make legacy claim partition metadata optional.
--
-- New v2 claims are scoped by the bound Active Namespace and policy fingerprint;
-- historical scope/project values remain readable when present. This additive
-- expansion permits new claims to omit those legacy fields without rewriting
-- existing claims.
DEFINE FIELD OVERWRITE scope ON claim TYPE option<string>;
DEFINE FIELD OVERWRITE project_identity ON claim TYPE option<string>;
