-- 048_browser_auth_policy_methods — Additive browser authentication methods
--
-- ADR-0057 replaces the exclusive browser authentication mode with a set of
-- methods. This migration is additive and does not rewrite a single row: it
-- declares where the set lives, and the reader treats a row without `methods`
-- as the one-method set named by `mode`. A deployment that already holds a
-- single-mode row therefore keeps working across the upgrade, and a downgrade
-- still finds the `mode` value it understands.
--
-- `mode` is retained as the value the row was created with. It is no longer the
-- source of truth for which methods are enabled; the durable set is `methods`.
--
-- The set is stored as an array rather than a delimited string so membership is
-- a comparison (`array::includes`) instead of a substring test.

DEFINE FIELD IF NOT EXISTS methods ON browser_auth_policy TYPE option<array<string>>;
