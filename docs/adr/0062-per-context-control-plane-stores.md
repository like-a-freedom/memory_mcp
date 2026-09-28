# The control-plane persistence seam is per-context stores composed at bootstrap

The registry had one trait, `RegistryStore`, that bundled all eight table-owner
traits behind a single `Arc`. A caller was handed all 51 methods to read two of
them. It was the only seam in the crate with two real adapters — `InMemoryStore`
and `SurrealRegistryStore` — so the width was a real cost rather than a
hypothetical one, and the trait's own docstring conceded it existed "so the two
store implementations and the existing call sites are unaffected by the split,
not because the omnibus capability is wanted."

That split is now done. `RegistryStores` holds one `Arc` per owner trait, and
every consumer names the capabilities it actually uses.

## What each consumer takes

| Consumer | Owner traits |
|---|---|
| `AccountResolver` | `TenantStore` |
| `Authenticator` | `AccountStore`, `ApiKeyStore` |
| `RegistryAuthMethodPolicy` | `BrowserPolicyStore` |
| `RegistryDeletionRecoveryAdapter` | `AccountStore`, `TenantStore`, `ProvisioningStore` |
| `TenantAndProvisioning` (the migration worker) | `TenantStore`, `ProvisioningStore` |
| `OidcSignup` | `AccountStore`, `ProvisioningStore` |
| `ControlSessionInvitationAdapter` | `AccountStore`, `SessionStore` |
| `RegistryApiKeyIssuance` | `AccountStore`, `TenantStore`, `UsageStore`, `ApiKeyStore` |

The last one is the only genuine four-way consumer, and it is four for a
reason: resolving an account's owner is an account read, its tenant's plan is a
tenant read, the cap is plan data, and the write is a key write. A consumer
needing one store now takes one trait. The adapters each lost capabilities they
were never entitled to use — the invitation-session adapter, for instance, could
previously issue API keys.

## Three decisions worth recording

**`ping` moved to its own port.** It was the only method on `RegistryStore` that
was not a table operation: it asks whether the connection answers, not whether
a table can be read. It is now `StoreHealth`, with one method and one consumer
(`/health`). Leaving it on the bundle would have kept the bundle alive for a
readiness probe, which is not a reason for the bundle to exist.

**Cross-owner atomic ports compose their own owners.**
`platform::persistence::control` keeps `AccountBundleTx`, `IdentityLinkTx`,
`IdentityLookup` and `AccountDeletionTx`; they are implemented in
`http/registry/control_impl.rs` over a pairing of exactly the owner traits each
crosses. The pairing is visible in one place, so a reader can see that account
deletion spans `account` and `tenant` and nothing else. It does not become a
universal registry: no port exposes a raw transaction or a `query(sql)` hatch,
and the set is the closed list of cross-owner writes the spec permits.

**The aggregator is not a new abstraction layer.** It is a struct of eight
`Arc`s, built once by fan-out in `RegistryStores::from_backend` from a single
`Arc<dyn RegistryBackend>`. Both adapters implement the backend trait, so there
is still exactly one implementation of each store per backend — the aggregator
adds no second set of adapters to keep in step. The `RegistryBackend` bound is
what makes that checkable: an adapter missing an owner trait fails to compile at
composition rather than in whichever context later asked for the part it lacks.

## What did not change

The physical control database is still shared and still one SurrealDB
connection. What changed is logical write ownership, which is what the spec
distinguishes: the eight owner traits and both adapters are untouched, and no
query text moved. `registry_query_shape` pins the multi-row and single-result
read shapes that broke once before; it is unchanged and still passes.

The spec's §Rename surface and §Migration inventory are unaffected. The eight
consumer ports in `identity`, `tenancy`, `provisioning` and `operations` are
untouched by this change — they are the narrow ports, and they keep working; the
question of whether thirteen one-adapter ports is the right granularity is a
separate one.
