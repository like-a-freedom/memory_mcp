-- 050_browser_auth_method_removal — name the method an audit event removed
--
-- Removing a browser authentication method (ADR-0057) is a deployment-level
-- action taken from the administrator CLI, not an Account action, so it is
-- recorded in the operator audit log rather than in `audit_event` (whose subject
-- is always an Account). `local_admin_audit` already carries the actor, the
-- action, the outcome and the request id; what it lacks is the method the action
-- concerned, because until now every action it recorded targeted an
-- administrator, a client or a key.
--
-- Additive and optional: rows written before this migration carry no value for
-- the field, which is the same shape as "this action named no method".

DEFINE FIELD IF NOT EXISTS target_method ON local_admin_audit TYPE option<string>
    ASSERT $value = NONE OR $value != "";
