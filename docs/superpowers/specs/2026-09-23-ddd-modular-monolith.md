# DDD Modular Monolith — design spec

**Date:** 2026-09-23
**Status:** Agreed in grill-with-docs design interview (all frontier decisions settled); ready for planning
**Plan:** `docs/superpowers/plans/2026-09-23-ddd-modular-monolith.md`
**ADR:** `docs/adr/0058-bounded-contexts-modular-monolith.md`

## Context

The domain model is already written down and crisp — `CONTEXT.md` defines Account,
External Identity, Tenant, Tenant Namespace, Tenant Registry, Tenant Runtime,
Account API Key, Authenticated Principal, Browser Auth Method as separate
aggregates with explicit isolation rules. The code does not express that model:
the SaaS side is a `control/` grab-bag of 20+ modules (`oidc`, `local_admin`,
`account_api`, `leases`, `session`, `registry`, `tasks`, …) where accounts,
tenancy, provisioning and delivery concerns sit side by side, and `service/` +
`storage/` form a second, parallel universe (the memory domain) outside any
shared structural contract.

The trigger was a rename request: the package `control-plane-ui` produces ugly
outward asset names (`/memory/assets/control-plane-ui-dxh8e6d38ccf6ed368.js`).
The interview escalated correctly: the name is ugly because it names a *layer*
("control plane UI") instead of a thing. Renaming alone would hide the symptom;
the cure is making the code's structure match the glossary, then naming the
console for what it is.

Operator decisions (settled across three interview rounds):

1. **Big bang**: one initiative — bounded contexts + clean architecture +
   rename. Not split.
2. **Full scope**: the memory domain (`service/`, `storage/`) is reworked too.
   "Clean architecture over half the monolith" was explicitly rejected: two
   competing dependency contracts in one crate are worse than either extreme.
3. **Contexts outside, layers inside**: `src/<context>/{domain, application,
   infra}` per context (variant A), not layers-outside/contexts-inside and not
   flat folders with facades only.
4. **Boundaries are enforced**, not decorative: per-context `api` facade +
   source-guard tests in the repo's existing pin-test style.
5. **Modules in one crate** (modular monolith): context crates are YAGNI for
   one deployable and the one-Active-Namespace invariant (ADR-0038).
6. **Rename**: package/binary `control-plane-ui` → `ui`; internal names follow
   (`ui` feature, `ui_assets.rs`, `MEMORY_MCP_UI_DIST`; no successor enable
   flag — the UI off-switch is removed entirely).
   Historical documents (ADRs, past specs/plans) stay verbatim.
7. **No runtime product-shape knobs** (creator ruling, review round 4): both
   runtime switches are deleted — `MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI`
   and `MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE` — retiring the data-plane-only
   runtime mode. The build profile is the single product-shape axis:
   `streamable-http` = data plane + control plane + console as **one product**;
   local = standalone stdio memory. Route exposure is reverse-proxy policy.
8. **Behavior freeze**: apart from the four declared changes below, this is
   strictly structural.

## Target architecture

### Bounded contexts (top level under `crates/memory-mcp/src/`)

| Context | Owns (aggregates from CONTEXT.md) | Sourced from today |
|---|---|---|
| `identity` | Account, External Identity, Browser Authentication Method, Local Administrator, Control Plane Session (incl. its cookie/CSRF hardening), principal resolution | `control/{account_api.rs (auth part), oidc.rs, oidc/, local_admin.rs, local_admin/, session.rs, recent_auth.rs, secret.rs, csrf.rs}`, `control/application/{oidc_signup.rs}`, `service/{local_admin/, credential_material.rs}`, `http/{oauth.rs, principal/, principal.rs}` |
| `tenancy` | Tenant, Tenant Namespace binding, Tenant Registry, Tenant Runtime | `http/{registry/, registry.rs, runtime/, runtime.rs, sync.rs}`, namespace-selection parts of `service/` |
| `provisioning` | Client, Account API Key, Lease, App Session, provisioning tasks | `http/{leases/, leases.rs, tasks/, tasks.rs, app_sessions/, app_sessions.rs}`, `control/application/api_keys.rs`, `service/{apps/, apps.rs}`, `control/account_api.rs` (client/key administration part) |
| `operations` | operator commands, deletion/recovery | `control/{operator.rs, deletion.rs}` |
| `memory` | Episode, recall/assembly, ingestion, agent-memory lifecycle, explain, procedural memory (candidates, ranking, review) | `service/{episode/, episode.rs, context/, capabilities/, capabilities.rs, agent_memory/, lifecycle/, lifecycle.rs, core/, core.rs, procedures/, procedures.rs, ingestion.rs, explanation.rs}` |
| `knowledge` | Entity, Fact, Claim (bi-temporal), claim reconciliation, aliases, content/entity extraction, triples, inbox revisions, search | `service/{fact.rs, entity.rs, entity_resolution.rs, claims/, conflict_resolver.rs, community.rs, content_extraction/, content_extraction.rs, entity_extraction/, entity_extraction.rs, triple_extractor.rs, query/, query.rs}` |
| `embedding` (platform context) | embedding generation and caching, embedding recovery/backfill, model artifacts and runtimes, re-embedding jobs | `service/{embedding/, embedding.rs, embedding_service.rs, embedding_recovery.rs, embedding_runtime.rs, model_artifacts/, model_artifacts.rs, model_artifact_refresh.rs, model_loader.rs, model_runtime.rs, cache/, cache.rs, reembed.rs, reembed_options.rs, reembed_progress.rs}` |
| `shared` | kernel: errors, typed ids/records, `ServiceContext`-shaped ports, SurrealDB connection and migrations, the bi-temporal close protocol | `models/`, `error.rs`, `control/error.rs`, config value types and parsing (root `config.rs` + `config/`, `http/config.rs` + `http/config/`), `service/{service_context.rs, util/, util.rs, value_helpers.rs, durable_work.rs}`, `storage/{client.rs, migrations.rs, close.rs, helpers.rs, queries.rs, types.rs}` |

Transports stay outer adapters at their current seams: `http/` (router, server,
transport, health, metrics, logging, composition, shutdown, validation,
`middleware/`, fault_injection, test harnesses — and the `subscriptions/listen`
stream machinery of `http/{subscriptions/, subscriptions.rs}`, whose
Tenant-Runtime dependencies go through `tenancy::api`), `mcp/`, `cli/`,
`tools/` (the MCP/CLI-shared adapter; its delegation targets become
`memory::api` / `knowledge::api`), and `ui/` — the console-delivery adapter
(`control/static_assets.rs` moves here: embedded-bundle serving and the
base-stamping contract). `ui/` is deliberately **layer-free**: it is delivery
plumbing for `crates/ui`'s bundle, not a domain, and inventing
`domain/application/infra` for it would be the DDD theater this rework
exists to remove. `bin/` stays the composition root. Platform modules stay at
the root unchanged: `logging.rs`, `observability.rs`, `runner.rs`,
`eval_support.rs`, `service/{startup.rs, fs_watch/, fs_watch.rs}`
(composition glue and the filesystem-ingestion input adapter),
`service/mock_db.rs` (test support), `main.rs`, `lib.rs`.

`storage/` dissolves into per-context `infra` by store:
`{episode_store.rs, agent_memory.rs, procedures.rs}` → `memory::infra`;
`{fact_store.rs, entity_store.rs, triple_store.rs, claims.rs,
inbox_revision_store.rs}` → `knowledge::infra`;
`{app_store.rs, context_store.rs}` → `provisioning::infra`;
`{embedding_backfill_store.rs, embedding_state_store.rs, reembed_store.rs}` →
`embedding::infra`; `{client.rs, migrations.rs, close.rs, helpers.rs,
queries.rs, types.rs}` → `shared` kernel infra. Module roots
(`control.rs`, `http.rs`, `service.rs`, `storage.rs`, `models.rs`, `tools.rs`,
`mcp.rs`, and per-tree submodule roots) dissolve as their content moves.

**Census rule (completeness contract):** every top-level unit of
`crates/memory-mcp/src/` **and every file of the dissolving trees** (`control/`,
`service/`, `storage/`) has exactly one home — a context row, the
transport/platform residual list, or the `storage/` split above. Any move not
covered by census is a spec bug and must amend this section before it lands.

### Layout and dependency rules (the contract)

```
src/<context>/
  api.rs        # the ONLY cross-context entry point (pub(crate) facade)
  domain/       # pure: entities, value objects, policies. No async runtime,
                # no axum, no surrealdb, no other context's types except shared
  application/  # use cases / workflows; depends on own domain + own ports (traits)
  infra/        # port implementations: SurrealDB stores, KDF, clocks
```

| From ↓ / To → | own `domain` | own `application` | own `infra` | other ctx `api` | other ctx internals | `shared` | axum/surrealdb |
|---|---|---|---|---|---|---|---|
| `domain` | ✅ | ❌ | ❌ | ❌ | ❌ | ✅ | ❌ |
| `application` | ✅ | ✅ | via ports only | ✅ | ❌ | ✅ | ❌ |
| `infra` | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ |
| transports (`http`,`mcp`,`cli`,`tools`,`bin`) | ❌ | via `<ctx>::api` only | ❌ | ✅ | ❌ | ✅ | ✅ |

Enforcement: `crates/memory-mcp/tests/module_boundaries.rs` — source-pin tests
in the existing style (`admin_api.rs`'s constructor guard; `test_ui_bundle_pin.py`
for the Dockerfile). One assertion group per rule row: scan each context's
source files for forbidden `use` paths and forbidden crate deps per layer.

### Rename surface (the four declared behavior-visible changes)

1. **Package and outward asset names**: `crates/control-plane-ui/` →
   `crates/ui/`; package + `[[bin]]` name `ui`; `Dioxus.toml` name
   `memory-mcp-ui`. Emitted assets become `ui-dxh<hash>.js`,
   `ui_bg-dxh<hash>.wasm`. The `<title>` copy ("Memory MCP control plane")
   stays: "control plane" is the domain term for the subsystem (see
   CONTEXT.md), it was never the package name's problem.
2. **`MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI` removed** —
   `HttpConfig::enable_control_plane_ui` is gone with no successor flag; the
   console is served whenever the profile carries it.
3. **`MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE` removed** —
   `HttpConfig::enable_control_plane` is gone, retiring the data-plane-only
   runtime mode and its "control off" test matrix rows.
   `memory_mcp_http` (the `streamable-http` profile) always mounts data plane
   + control plane + console as one product and has **no runtime product-shape
   switches at all**.
4. **Names**: feature `control-plane-ui` → `ui` (internal name, implied by
   `streamable-http` as before); build-time `MEMORY_MCP_CONTROL_PLANE_UI_DIST`
   → `MEMORY_MCP_UI_DIST` (hard rename — build env is not deployed API);
   generated manifest `control_plane_assets.rs` → `ui_assets.rs`.

Compatibility notes: hashed asset URLs change with any bundle rebuild anyway;
the relocatable-bundle sentinel contract (`/__memory_mcp_base__`) is
content-driven and **not** affected by file names. Cookie contracts, OIDC
redirects, HTTP routes, the eight-tool MCP surface and the bi-temporal model
are untouched.

### No runtime product-shape knobs (to-be doctrine)

An earlier draft of this spec kept `MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE`,
arguing from the shipped implementation (default `false`, "control off" test
rows, store-level degradation). The creator's ruling overturns that: the
implementation history is not a design argument. The flag conflated three
concerns — what is compiled, what is mounted, what is externally reachable —
and the to-be decomposes them onto three clean axes:

1. **build profile** (`streamable-http` vs local) — which product is compiled;
   `streamable-http` is *data plane + control plane + console*, one whole
   product, always mounted;
2. **runtime configuration** — deployment *values* only (URLs, keys, auth
   method set, signup mode, quotas). Nothing at runtime selects product
   shape;
3. **reverse proxy** — which mounted routes are externally reachable (the
   `/metrics` precedent elevated to a principle: exposure is a proxy concern,
   never a config knob).

Consequences embraced as declared breaks: the data-plane-only runtime mode is
retired (its tests and config-validation branches are deleted, not adapted),
and an API-only *exposure* is achieved by proxy routing, not by mutilating the
product. This also resolves two documented roughnesses by deletion: the
compose-vs-code default divergence (`true` vs `false`) disappears with the
flag, and the oddity that provider settings were demanded even with the flag
`false` disappears with its validation branch.

## Non-goals (behavior freeze)

- MCP public surface stays exactly eight tools (`public_surface_snapshot`).
- HTTP routes, status codes, envelopes: unchanged.
- Bi-temporal validity (`t_ref`/`t_ingested`, invalidate-never-delete): unchanged.
- One Active Namespace per process / Tenant-never-by-request invariant: unchanged.
- Cookie contracts (`__Host-`/`__Secure-`, `Path` scoping): unchanged.
- `MEMORY_MCP_HTTP_SIGNUP_MODE`, `MEMORY_MCP_HTTP_AUTH_METHODS`, quotas and all
  other deployment *value* knobs: unchanged.
- Crate splitting into a workspace per context: out of scope (ADR-0058 records
  the revisit condition).
- ADR-0056's build optionality stays as-is: `MEMORY_MCP_UI_DIST` unset still
  yields a binary with an empty UI catalog ("unconditional UI" means the SaaS
  profile always *carries* the console capability and serves it whenever it is
  bundled — compile-without-`dx` is what makes the test matrix possible).
- Historical documents (ADRs 0001–0057, past specs and plans) are not rewritten.

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Big-bang diff is unreviewable | The plan lands as ordered phases; every phase leaves `master` green (full CI matrix), each phase is one reviewable commit series per context |
| DDD theater (folders without rules) | Guard tests land in the *first* extraction phase and only grow |
| Storage seams (`ServiceContext`, `DbClient`) resist clean layering | `shared` keeps the narrow ports; extraction order runs `ui`/`operations` first and the storage-heavy `knowledge`/`memory` last |
| Concurrent writers on this repo (observed twice) | Rename + scaffolding phases are conflict-hot; announce the branch, land fast |
| `dx` bundle layout assumptions keyed on the bin name | Re-verify with a probe bundle before/after (the relocatable-bundle spike pattern); `test_ui_bundle_pin.py` extended for the `ui` name |
