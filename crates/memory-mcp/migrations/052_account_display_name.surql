-- 052_account_display_name — give an Account the human name its provider asserts
--
-- An Account was created from an OIDC identity and has been addressed by id ever
-- since: `acct_…`, `ten_…`, RFC 3339 timestamps. Every identifier on the account
-- page was mechanical, because the identity mapping stores nothing a person can
-- read — the registry keeps `issuer` and a keyed blind index of the OIDC `sub`
-- (ADR-0052), and never the subject itself. There was nowhere for the name the
-- provider issued to land.
--
-- The specification treats a display name as an optional minimized attribute and
-- never as an identity key, so it is stored on the Account rather than on the
-- External Identity: a name belongs to the person, not to one login method, and
-- an Account's session records only `account_id`.
--
-- Additive and optional: rows written before this migration carry no value for
-- the field, which is the same shape as "this provider asserted no name". Email is
-- deliberately not stored here — the specification requires it encrypted, and no
-- privacy policy exists in this repository to govern it.

DEFINE FIELD IF NOT EXISTS display_name ON account TYPE option<string>
    ASSERT $value = NONE OR ($value != "" AND string::len($value) <= 200);
