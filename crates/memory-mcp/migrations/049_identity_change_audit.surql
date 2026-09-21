-- 049_identity_change_audit — name the resource an audit event acted on
--
-- 045 records who did what to which Account; the Account is the subject of
-- every event it writes, so no field was needed for a resource *inside* an
-- Account. An identity change (ADR-0057) is exactly that: the Account holds
-- the action, but the External Identity is what was attached or detached, and
-- an unlink deletes the row that named it. Recording the id here is what keeps
-- the audit row readable after the identity is gone.
--
-- Additive and optional: rows written before this migration carry no value for
-- the field, which is the same shape as "no non-account resource".

DEFINE FIELD IF NOT EXISTS target_identity_id ON audit_event TYPE option<string>
    ASSERT $value = NONE OR $value != "";
