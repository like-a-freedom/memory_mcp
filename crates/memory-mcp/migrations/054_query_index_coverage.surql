-- Index coverage for the context-assembly, explain and entity-resolution reads
-- (plan 2026-10-09 query-index-coverage).
--
-- Two gaps the earlier record-cast work left open:
--
-- 1. The `ORDER BY <time> LIMIT n` reads (recent facts, active facts, episode
--    fallback, the edge page) had no index on the time column, so each scanned
--    and sorted its whole table.
-- 2. An array index declared `COLUMNS field` serves only exact-array equality
--    (`field = [...]`). It is never used for `CONTAINS`/`CONTAINSANY`. The
--    membership form is an array-element index, `FIELDS field.*`, and only
--    `CONTAINSANY` uses it. `entity_aliases` and `community_members` existed in
--    the non-membership form and so were dead for the queries that used them.
--
-- `fact.entity_links` deliberately gets no index of its own: its queries always
-- carry the bi-temporal bound, and with `fact_t_valid` present the planner
-- range-scans that index and filters `entity_links`, so a second fact index
-- would only add write cost.
DEFINE INDEX IF NOT EXISTS fact_t_valid ON TABLE fact FIELDS t_valid;
DEFINE INDEX IF NOT EXISTS episode_t_ref ON TABLE episode FIELDS t_ref;
DEFINE INDEX IF NOT EXISTS edge_t_valid ON TABLE edge FIELDS t_valid;
DEFINE INDEX OVERWRITE entity_aliases ON TABLE entity FIELDS aliases.*;
DEFINE INDEX OVERWRITE community_members ON TABLE community FIELDS member_entities.*;