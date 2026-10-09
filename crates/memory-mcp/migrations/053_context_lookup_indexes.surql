-- Context-lookup indexes for the plan 2026-10-09 query-performance work.
--
-- `episode_id` backs the final stage of select_episodes_via_entity, which
-- reads a bound id list rather than scanning the table. `source_episode`
-- carries the fact-to-episode provenance link that explain follows for every
-- item it reports, so it is read once per item.
--
-- `fact.fact_id` deliberately has no index here: migration 029 already defines
-- fact_claim_backfill_cursor_idx ON fact COLUMNS fact_id, and a second index
-- on the same column would only duplicate the write cost.
DEFINE INDEX IF NOT EXISTS episode_episode_id ON TABLE episode COLUMNS episode_id;
DEFINE INDEX IF NOT EXISTS fact_source_episode ON TABLE fact COLUMNS source_episode;