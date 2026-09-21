# ADR-0057: Additive browser authentication methods

## Status

Accepted, 2026-09-21. This decision amends the exclusivity clause of
[ADR-0055](0055-local-admin-authentication-for-remote-deployment.md) (§Deployment
authentication modes and its Alternatives entry "Enable local and OIDC
authentication together") and supersedes the single-mode reading of the browser
authentication vocabulary in [`CONTEXT.md`](../../CONTEXT.md). The rest of
ADR-0055 — local administrator identity, CLI bootstrap and recovery, client
provisioning, key issuance, audit and isolation requirements — remains in force.

Implementation status. The set-of-methods half is implemented and verified: the
configuration contract, independent route mounting, the single policy writer,
the method disclosure, the login page, the admin CLI, and the single-file
deployment. The **identity linking for Accounts** half is specified here but not
yet built; the current `POST /api/v1/account/identity_links` still accepts the
identity from the request body rather than from a provider round-trip, and
`unlink` is still unconditional. Treat that section as the requirement it must
meet, not as a description of the shipped code.

## Context

### The deployment story that does not work today

A deployment often starts without an identity provider and gains one later. The
operator bootstraps with a local administrator, provisions clients, issues their
API keys — and then an OIDC provider appears and they want their users signing in
through it. Today that path is not merely awkward, it is closed:

- The control registry holds one `browser_auth_policy` row whose `mode` is
  `local`. `join_oidc_policy` throws `mode_mismatch` against it, so the server
  refuses to start in OIDC mode
  (`crates/memory-mcp/src/http/registry/surreal_store.rs`, `join_oidc_policy`;
  `local_admin.rs`, `join_local_policy`). A configuration flip does not migrate a
  deployment; it stops it.
- The migration the design anticipated was never built. The specification states
  that switching modes is an offline operator operation with "no new mode-switch
  CLI in this scope"
  (`docs/superpowers/specs/2026-09-18-local-admin-auth.md`), and no code path
  rewrites the policy row or increments its `epoch` — the runbook records that
  the epoch "is created as `1` and stays there"
  (`docs/operations/LOCAL_ADMIN.md` §9.2).
- The two principals do not convert into each other. A Local Administrator has no
  `external_identity` row, and operator authorization is resolved *through* one
  (`crates/memory-mcp/src/http/middleware/auth.rs`), so the administrator who
  bootstrapped the deployment cannot carry their administration into OIDC mode.

The result is that adding an identity provider to a running deployment looks like
a re-provisioning event rather than an evolution. Nothing in the product requires
that: API keys, Accounts and Tenants carry no mode, and the data plane keeps
working across any browser-auth change.

### What comparable deployments do

Every comparable self-hosted product treats login methods as **additive**, and
keeps a local credential as an explicitly protected escape hatch:

| Product | Pattern |
| --- | --- |
| Grafana | Several providers configured at once (at most one per type). Identity is keyed on the provider's unique subject, not on email; same-email lookup across providers exists but is opt-in and flagged as reducing security. `protected_roles` deliberately keeps privileged roles from adopting a new auth scheme. |
| Vault | One *entity* per person, one *alias* per provider account: "Each user may have multiple accounts with various identity providers". A login through any backend creates the entity and its alias. |
| Kliv, Tallyfy, TraceApps | "Require SSO" is a separate, explicit operation performed *after* SSO is verified working; it closes the other doors and never the provider's own. |
| Kliv | Break-glass administrators keep password + MFA; requiring SSO is refused until one exists, and the **last eligible** break-glass administrator cannot be removed, disabled or stripped of MFA. |
| grimmory, frem.sh | The local login form is always available to administrators as a safety net, even under full SSO. |
| Neops, librariarr | The login page lists every enabled method side by side; users pick the one meant for them. |
| NIST-aligned account-linking guidance | Key identity on `(issuer, subject)`, never on email. Attach a new provider login only inside an already-authenticated session with explicit confirmation, and log it. |

`memory_mcp` already keys external identities on `(issuer, subject_verifier)`
with no uniqueness on `account_id` (`migrations/001_registry.surql`), which is the
shape that guidance describes. The singular mode is the part that does not match.

## Decision

### Authentication methods are a set, not a mode

An enabled control plane serves a **set** of browser authentication methods,
currently `oidc` and `local`. Enabling a second method is configuration, not a
migration. A deployment that enables both mounts both surfaces; the operator
chooses nothing globally, and each browser user picks a method at the login page.

The environment contract is one CSV variable:

```text
MEMORY_MCP_HTTP_AUTH_METHODS=oidc,local
```

`MEMORY_MCP_HTTP_AUTH_MODE` remains accepted as a deprecated alias for one
release (`local` → `local`, `oidc` → `oidc`; supplying both variables with
conflicting values fails startup). The check that made local mode reject OIDC
configuration outright — `local mode must not have OIDC configuration`
(`crates/memory-mcp/src/http/config/types.rs` and its validator mirror) — is
**deleted**: when `oidc` is in the set, that configuration is required rather than
forbidden. The three HMAC keys derived from the session key in local mode
(`derive_local_key`) are derived **only** while `local` is the sole method; with
`oidc` in the set they are required in the environment as before.

### The durable policy records the set

`browser_auth_policy` gains the enabled `methods` alongside its existing `epoch`
and local key fingerprints. A legacy row (`mode = 'local'` or `'oidc'`) is read
as a single-method set, so a running deployment upgrades without intervention and
without a data migration. The fingerprints stay: they describe the local method's
key material, not "the current mode", which is why returning to a
password-enabled set still requires the identical `SESSION_KEY`/`CSRF_KEY`.

The row is read deterministically and must be exactly one row. Today the index is
`FIELDS mode UNIQUE` — one row *per method*, not one row — while every read is
`SELECT … LIMIT 1` with no id and no ordering, so a second row would make the
deployment's behaviour depend on the storage engine's row order. That is
unreachable while the joins reject a foreign mode, but it is the invariant this
whole model rests on and it is hardened as part of this decision.

### Removing a method is an explicit, guarded operation

Turning a method off is how a deployment reaches "SSO only", and it is the only
operation resembling the old "switch". It is explicit configuration — never
inferred from the presence or absence of other variables — and it is guarded by a
**last-administrator rule**: removing `local` is refused while no operator
identity is configured, because that would leave no route to administering the
deployment. This is the analogue of the industry rule that SSO cannot be required
before a break-glass administrator exists. The existing "no administrator
removal" non-goal already prevents deleting the last Local Administrator.

### The local method is the break-glass door

`local` is the only method that does not depend on an external service, so it
stays available as the escape hatch when an identity provider is unreachable or
misconfigured. Two consequences are accepted deliberately:

- It is enabled **only by explicit configuration** and is never implied by
  another method's absence, and it must not be exposed to public ingress.
- MFA remains out of scope (ADR-0055), so this door is password-only, which is
  weaker than the industry norm of pairing break-glass with a second factor. The
  residual risk is documented rather than silently carried; adding a second
  factor is a separate decision, not a prerequisite for this one.

### One login surface

`GET /api/v1/auth/config` returns the enabled methods instead of a single mode,
and the login page renders every enabled method side by side: the identity
provider first, then the administrator form, labelled as the deployment
administrator's door rather than a user's. This is an internal endpoint consumed
only by the embedded console, so the field is replaced rather than versioned.

### Identity linking for Accounts

An Account may hold several External Identities. Attaching one is a
proof-of-ownership flow, not an assertion: the link is created only from an
already-authenticated session, through a provider round-trip that carries the
link intent, so the identity presented is verified by the provider rather than
accepted from a request body as the current route does. The last identity of an
Account cannot be unlinked, and link and unlink are audited with both identities,
the actor and the timestamp.

## Implementation notes

Recorded the day this decision landed, because three details are checkable and
one of them is a behaviour a reader would otherwise have to infer.

- **One writer, not two.** The two joins this decision replaced
  (`join_oidc_policy`, `join_local_policy`) became
  `RegistryStore::reconcile_browser_policy(desired, local)`. The row holds the
  whole configured set after a single transaction, so two methods cannot race
  for the singleton and the local method's fingerprints are written by the same
  statement that writes the set. The stored set is written in canonical order
  (`local` before `oidc`), so it is one value however the caller enumerated the
  methods.
- **Material for a disabled method is refused, not ignored.** A provider that is
  configured but not enabled is a deployment that believes it has SSO when it
  does not, so the server names the offending variable and tells the operator to
  add `oidc` to `MEMORY_MCP_HTTP_AUTH_METHODS`. This is the rule the deleted
  `local mode must not have OIDC configuration` check was replaced by, and it is
  conditioned on the set rather than on `local`.
- **The `free` plan follows `oidc`.** A `local`-only deployment publishes no
  version-1 `free` plan at all, because that plan backs the tenants `oidc`
  signup creates. A deployment that enables `oidc` beside `local` publishes it
  again on the same restart.

## Consequences

- A deployment can start with a local administrator, add OIDC later, and run both
  for as long as it likes without re-provisioning clients, rotating API keys or
  rebuilding the image. Adding the provider is an environment change and a
  restart.
- `local` and `oidc` routes are mounted independently, so the assertions that each
  mode's routes are absent in the other mode — the router tests, both integration
  suites and the Compose smoke check — are replaced by assertions that each
  *disabled* method is absent.
- The protected-route surface grows by exactly the methods enabled, and the
  login page becomes the place where a deployment's authentication shape is
  visible. A misconfigured deployment is now visible rather than fatal: an
  unreachable provider no longer prevents the operator from reaching the console.
- The privileged door does not move. Local Administrators remain a separate
  principal, are still not Accounts, and still gain nothing from an identity
  provider — so a provider compromise cannot escalate to deployment
  administration.
- The effort is concentrated in configuration, routing and the policy row. No
  change to the data plane, Accounts, Tenants, API keys or the eight-tool MCP
  surface.

## Alternatives considered

- **Keep one mode per deployment and implement the switch.** This is the design
  the specification anticipated. It solves the operator's immediate blocker but
  leaves the product as the only comparable one that cannot run a local
  credential beside a provider, and it requires the same policy work plus a
  bespoke offline procedure. Rejected: it implements the migration instead of
  removing the need for it.
- **Declare the transition in the environment and let startup perform it**
  (`AUTH_MODE_MIGRATE_FROM=local`). Convenient for Compose, but it makes the
  server rewrite durable state from configuration, and a variable left behind in a
  file can fire again. Rejected in favour of explicit, audited operations.
- **Link an External Identity to a Local Administrator** so one person signs in
  as an administrator either way. This is the literal "add Google sign-in to my
  account" model, but the privileged principal is exactly the one that should not
  gain a path through an external service, and Grafana's `protected_roles` shows
  the opposite convention is deliberate. Rejected; an operator who wants
  OIDC-based administration uses the operator identity allowlist.
- **Auto-merge Accounts by email across providers.** The industry default is to
  key on `(issuer, subject)` and flag email matching as insecure, and the
  guidance is explicit that email is not proof of ownership. Rejected; the
  existing schema already models the safe shape.
- **Mount both surfaces without a guard on removal.** One environment edit would
  be enough to close the last route to administration. Rejected; the
  last-administrator rule is the whole point of a break-glass door.

## Relationships

This decision amends ADR-0055's exclusivity clause. ADR-0055's OIDC-mode and
External Identity rules continue to describe the `oidc` method, and its local
administrator rules continue to describe the `local` method; what changes is that
the two are no longer alternatives.

[ADR-0052](0052-streamable-http-saas-profile.md)'s transport, tenancy and HTTP
boundaries remain in force. [ADR-0053](0053-explicit-http-storage-and-migration-composition.md)
continues to govern explicit storage composition: the policy change is additive
and reads legacy rows, so no destructive migration is introduced.
[ADR-0056](0056-two-build-profiles.md) governs the build surface and is
unaffected — both methods ship in the `streamable-http` profile.
