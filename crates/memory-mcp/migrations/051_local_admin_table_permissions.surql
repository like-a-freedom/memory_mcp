-- 051_local_admin_table_permissions.surql
--
-- Migration 047 created the nine local-admin and browser-auth-policy tables
-- without the unprivileged-access denial every registry table in 001 carries,
-- so a less-privileged SurrealDB user could read `local_admin.password_phc`
-- and `local_admin_session.cookie_verifier` (the raw session secret) directly.
-- This additive migration redefines each table with the 001 deny-everything
-- permissions. It deliberately omits `IF NOT EXISTS`: the whole point is to
-- update the existing definitions (`ALTER TABLE`, since `DEFINE TABLE`
-- refuses an already-defined table). Migration 047 is never edited
-- (append-only history).
--
-- Table permissions bind non-privileged database users only; the application
-- connects with privileged credentials and is unaffected.

ALTER TABLE browser_auth_policy SCHEMAFULL
    PERMISSIONS
        FOR select WHERE false
        FOR create, update, delete WHERE false;

ALTER TABLE local_admin SCHEMAFULL
    PERMISSIONS
        FOR select WHERE false
        FOR create, update, delete WHERE false;

ALTER TABLE local_admin_challenge SCHEMAFULL
    PERMISSIONS
        FOR select WHERE false
        FOR create, update, delete WHERE false;

ALTER TABLE local_admin_session SCHEMAFULL
    PERMISSIONS
        FOR select WHERE false
        FOR create, update, delete WHERE false;

ALTER TABLE local_admin_rate_bucket SCHEMAFULL
    PERMISSIONS
        FOR select WHERE false
        FOR create, update, delete WHERE false;

ALTER TABLE local_admin_client SCHEMAFULL
    PERMISSIONS
        FOR select WHERE false
        FOR create, update, delete WHERE false;

ALTER TABLE local_admin_client_key SCHEMAFULL
    PERMISSIONS
        FOR select WHERE false
        FOR create, update, delete WHERE false;

ALTER TABLE local_admin_operation SCHEMAFULL
    PERMISSIONS
        FOR select WHERE false
        FOR create, update, delete WHERE false;

ALTER TABLE local_admin_audit SCHEMAFULL
    PERMISSIONS
        FOR select WHERE false
        FOR create, update, delete WHERE false;
