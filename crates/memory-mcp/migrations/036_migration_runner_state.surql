-- Migration 036: durable migration-runner state.
--
-- The runner reserves a migration ledger row before executing its body. This
-- makes crash recovery and concurrent remote starters observable without adding
-- a new environment variable or a second control-plane store.
DEFINE FIELD OVERWRITE status ON script_migration TYPE string;
DEFINE FIELD OVERWRITE owner ON script_migration TYPE option<string>;
DEFINE FIELD OVERWRITE lease_expires_at ON script_migration TYPE option<datetime>;
DEFINE FIELD OVERWRITE started_at ON script_migration TYPE option<datetime>;
DEFINE FIELD OVERWRITE last_error ON script_migration TYPE option<string>;
