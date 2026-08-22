# ADR-0045: Layer boundaries — neutral error home, no service→MCP imports

## Status

Accepted — 2026-08-21, audit remediation wave 2.

## Decision summary

`MemoryError` and `is_transient_db_error` move from `service/error.rs` to a
neutral top-level `crate::error` module. `service` re-exports them for
compatibility, so `crate::service::MemoryError` keeps working. Storage,
config, models, tools, CLI, and service all depend on the neutral module;
only the MCP layer imports `rmcp`.

App-session state (`SessionManager`, `AppSessionState`) moves to
`service/apps/session.rs` with `MemoryError` results. Protocol shaping
(`invalid_params`, `missing_app_field`, `internal_error`,
`open_app_result`, `app_command_result_from_details`,
`enrich_session_payload`) stays in the MCP layer. The service layer must
not import anything from `crate::mcp`; the MCP layer is a thin adapter.

## Context

The 2026-08-21 architecture audit found two dependency inversions:

1. **Storage → service.** Sixteen storage modules imported
   `crate::service::MemoryError`. The error type describes memory-domain
   failures (storage, validation, config, conflict), not service-layer
   policy; its home in `service/` forced lower layers to reach upward.
2. **Service → MCP.** `SessionManager` lived in `mcp/session.rs` but was
   consumed by `service/apps/session_lifecycle.rs` and
   `service/apps/dispatch.rs`, which also called `crate::mcp::session`
   helpers and returned `rmcp::ErrorData`. The service layer — supposed
   to be protocol-neutral — depended on the protocol adapter it should be
   serving.

## Consequences

- Dependency arrows point inward: `mcp → service → {storage, models} → error`.
- `ErrorData` remains in `dispatch.rs` executor signatures for now; the
  session-state seam (the inversion with runtime state) is fixed first.
  A later pass may introduce a service-owned command error type if the
  apps surface grows beyond MCP.
- No public API change: `memory_mcp::MemoryError` still resolves via the
  re-export chain.
