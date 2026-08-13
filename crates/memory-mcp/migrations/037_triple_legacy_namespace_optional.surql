-- Migration 037: make legacy triple namespace metadata optional.
--
-- Triple rows created before the one Active Namespace model may contain a
-- namespace field. New triple writes rely on the bound database context and
-- omit this legacy routing field. Preserve old values for compatibility.
DEFINE FIELD OVERWRITE namespace ON triple TYPE option<string>;
