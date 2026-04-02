-- Migration 010: Coerce string t_ingested to datetime for episode, fact, and edge tables.
-- Fixes schema mismatch where old records store t_ingested as string instead of datetime.

-- Convert string t_ingested values to datetime on episode table
UPDATE episode SET t_ingested = type::datetime(t_ingested) WHERE type::is_string(t_ingested);

-- Convert string t_ingested values to datetime on fact table
UPDATE fact SET t_ingested = type::datetime(t_ingested) WHERE type::is_string(t_ingested);

-- Convert string t_ingested values to datetime on edge table
UPDATE edge SET t_ingested = type::datetime(t_ingested) WHERE type::is_string(t_ingested);
