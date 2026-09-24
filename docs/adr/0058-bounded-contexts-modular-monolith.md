# ADR-0058: Bounded contexts and clean architecture inside the modular monolith

**Date:** 2026-09-23
**Status:** Accepted
**Supersedes:** the `Module seams` map in `CONTEXT.md` (that map stays
authoritative for the pre-0058 layout until each context lands)
**Spec:** `docs/superpowers/specs/2026-09-23-ddd-modular-monolith.md`

## Context

`CONTEXT.md` defines crisp aggregates — Account, External Identity, Tenant,
Tenant Namespace, Tenant Registry, Account API Key, Authenticated Principal —
but the code concentrates them in a `control/` module of 20+ peers, and the
memory domain (`service/`, `storage/`) sits outside any shared structural
contract. Cross-cutting calls between accounts, tenancy, provisioning and
delivery compile freely; nothing enforces the isolation rules the glossary
already states (e.g. "never accepted as a data-plane request argument").

A request to rename the `control-plane-ui` package surfaced the naming symptom
of the structural problem: the package named a layer, not a thing.

## Decision

1. **One modular monolith, explicit bounded contexts** at the top level of
   `crates/memory-mcp/src/`: `identity`, `tenancy`, `provisioning`,
   `operations`, `memory`, `knowledge`, `embedding` (platform
   context: embedding generation/caching/recovery, model artifacts and
   runtimes, re-embedding), plus a `shared` kernel. Transports (`http`, `mcp`,
   `cli`, `tools`, `ui`, `bin`) remain outer adapters; `ui` (console-bundle
   delivery, from `static_assets.rs`) is deliberately layer-free. There is no
   `delivery` context: cookie/CSRF hardening belongs to the Control Plane
   Session aggregate (`identity::infra`) and console delivery is plumbing, not
   a domain.
2. **Clean architecture inside each context**: `domain` (pure) /
   `application` (use cases over ports) / `infra` (port implementations).
   Dependency rule: inward only; `domain` depends on nothing outside `shared`.
3. **Boundaries are compile-adjacent and test-enforced**: cross-context calls
   go through the target's `api` facade (`pub(crate)`); `tests/module_boundaries.rs`
   source-pins every dependency-rule violation class.
4. **No per-context crates.** One deployable, one Active-Namespace invariant
   (ADR-0038); compiler walls are not worth the workspace churn yet. Revisit
   only when a context needs independent release or ownership boundaries.
5. **The operator console package is renamed to `ui`** (bin `ui`; outward
   assets `ui-dxh*.js`, `ui_bg-*.wasm`). "Control plane" remains the domain
   term for the subsystem and is not a package name.
6. **The UI ships unconditionally with `streamable-http`**: the console is
   part of the product whole. Both runtime switches —
   `MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI` / `enable_control_plane_ui` and
   `MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE` / `enable_control_plane` — are
   deleted (decision 8).
7. **Build-time `MEMORY_MCP_CONTROL_PLANE_UI_DIST` becomes
   `MEMORY_MCP_UI_DIST`** (hard rename; build environment is not deployed API).
8. **No runtime product-shape switches.**
   `MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI` and
   `MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE` are both removed, retiring the
   data-plane-only runtime mode. Product shape is the build profile alone
   (`streamable-http` = data plane + control plane + console as one whole;
   local = standalone stdio memory); runtime config carries deployment values
   only; route exposure is reverse-proxy policy (the `/metrics` precedent
   elevated to a principle). The implementation's shipped default posture is
   deliberately overturned: implementation history is not a design argument.

## Consequences

- Positive: the module map finally speaks the glossary's language; isolation
  rules are mechanically enforced; the ugly outward asset names disappear; two
  configuration knobs less and one dead runtime mode gone.
- Negative: a large, conflict-hot restructuring (mitigated by ordered phases,
  each leaving `master` green); four breaking surface changes for operators
  (renamed package and outward asset names, two removed runtime switches with
  the data-plane-only mode retired, renamed build env) documented in the
  spec's compatibility notes.
- The eight-tool MCP surface, HTTP routes, bi-temporal model and cookie
  contracts are frozen (spec non-goals).
