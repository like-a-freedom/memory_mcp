# ADR-0058: Bounded contexts and clean architecture inside the modular monolith

**Date:** 2026-09-23; clarified after architecture review 2026-09-24
**Status:** Accepted design direction; implementation tracked by the plan
**Supersedes:** Module organization after implementation, not current runtime behavior
**Spec:** [design contract](../superpowers/specs/2026-09-23-ddd-modular-monolith.md)
**Plan:** [execution and evidence](../superpowers/plans/2026-09-23-ddd-modular-monolith.md)

## Context

`control/`, `service/` and `storage/` mix business rules, orchestration,
persistence and protocol concerns. A shared glossary does not by itself define
aggregate transaction boundaries. Renaming the console package is useful but
does not establish architectural isolation. The accepted initiative addresses
both the package surface and ownership across the full application.

## Decision

1. Keep one production library crate with seven responsibility modules:
   `identity`, `tenancy`, `provisioning`, `operations`, `memory`, `knowledge`,
   `embedding`. Distinguish business models from orchestration and technical
   capabilities: embedding is a platform capability, not a rich business
   bounded context. Verify transaction/ownership assumptions before extraction.
2. Use context-local domain policies, application use cases/ports and infra
   implementations. Create layers when behavior needs them; no empty-layer
   ceremony. A small **pure** `shared` kernel is separate from technical
   `platform` persistence/runtime and `bootstrap` composition. Domain and
   application must not acquire infrastructure through a shared re-export.
3. Cross-module behavior goes through published API data contracts and the
   acyclic allowlist in the spec. Keep context internals private; combine Rust
   privacy with source guards and negative fixtures for bypasses/cycles.
   Bootstrap has explicit wiring privileges and public library entrypoints for
   separate binary crates. Tests/evaluation use deliberate feature-gated
   surfaces. Facade-only rules do not prohibit constructing adapters at startup.
4. Preserve existing atomic transactions, audit and durable change events.
   Ordinary canonical writes have one owner. Existing cross-owner registry
   transactions use narrow owner-defined atomic ports implemented by an
   explicit control persistence integration seam; do not split them into
   separately committed facade calls. No universal registry, event bus or
   distributed transaction framework is introduced.
5. Transports (`http`, `mcp`, `cli`, `tools`, `ui`) remain outer adapters.
   `ui` serves the bundled console without domain layers. HTTP cookie/CSRF
   mechanics stay in HTTP; identity owns session and authentication policies.
6. Rename `control-plane-ui` package/bin to `ui`, emitted assets accordingly,
   internal feature to `ui`, build variable to `MEMORY_MCP_UI_DIST` and generated
   asset manifest to `ui_assets.rs`. Control plane remains a domain subsystem
   name. Historical ADRs 0001–0057 and old plans/specs are not rewritten.
7. Remove `MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI` and
   `MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE` and their fields, retiring the
   data-plane-only runtime mode. SaaS `streamable-http` carries the data plane,
   control plane and console capability. Developer/test builds may have an
   empty UI catalog per ADR-0056; release artifacts must embed a real bundle.
8. Runtime deployment values and auth-method configuration remain. Proxy
   routing controls exposure; application authorization/CSRF protections still
   apply. Flag removal is a product choice, not a consequence of DDD or
   Twelve-Factor. Validate upgrades with now-active authentication startup and
   readiness requirements.
9. Apply Twelve-Factor operational checks to SaaS with explicit local embedded/
   stdio exceptions. Keep the local process's immutable Active Namespace and
   each SaaS Tenant Runtime's immutable Tenant Namespace distinct. Neither
   accepts storage selection in data-plane arguments.

## Alternatives and consequences

A package rename alone fixes artifact names but leaves ownership ambiguous.
Top-level horizontal layers preserve the existing cross-domain coupling.
Per-context crates could strengthen compiler dependency walls even within one
monolith, but are deferred because module privacy plus tested guards are less
migration work now. Namespace semantics do not determine crate topology.
Reconsider crates if guards repeatedly miss violations, independent ownership
needs stronger walls, or build isolation demonstrably benefits; independent
service deployment is not a prerequisite.

The benefit is explicit policy/transaction ownership and testable dependency
rules. The cost is a broad migration with significant registry, retrieval and
public Rust API seams. One initiative lands as buildable commits with exact
expiring legacy bridges; it is released as a coherent whole. Phase 0 inventory
and transaction evidence are gates, not claims already satisfied by this ADR.

There are four declared surface-change categories: package/assets rename,
UI runtime switch removal, control-plane runtime switch removal, and related
internal/build names. All other HTTP/MCP, cookie, temporal, namespace and
persisted-data semantics are frozen. Detailed ownership, upgrade prerequisites,
verification and limitations are maintained in the spec/plan rather than
repeated as competing contracts here.
