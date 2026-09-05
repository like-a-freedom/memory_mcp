//! HTTP middleware.
//!
//! Each middleware is `axum::middleware::from_fn`-compatible. Layer
//! ordering matters: layers added LATER wrap layers added EARLIER on
//! the request path.
//!
//! The implementation is split by concern across `middleware/`:
//!
//! - `preflight` — modern MCP envelope validation (mirrored headers,
//!   JSON-RPC 2.0, version, method classification) and the
//!   `ValidatedMcpRequest` extension.
//! - `auth` — Bearer API-key, control-plane cookie, and operator
//!   allowlist authenticators, plus CSRF.
//! - `acquire_runtime` — tenant resolution, admission permits,
//!   runtime pool acquisition, and quota reserve.
//! - `deadline` — outer-most request deadline (returns 408 on timeout).
//! - `sse_headers` — inject `Cache-Control: no-cache` and
//!   `X-Accel-Buffering: no` on SSE responses.
//! - `host_origin` — host and origin allowlist enforcement with
//!   trusted-proxy CIDR awareness.
//!
//! This file is a thin façade: every public name is re-exported so
//! callers continue to use the `crate::http::middleware::X` paths.

mod acquire_runtime;
mod auth;
mod deadline;
mod host_origin;
mod preflight;
mod sse_headers;

// Public re-exports. Adding a new middleware should add it here and
// in the corresponding submodule, never in this file.
pub use acquire_runtime::acquire_runtime;
pub use auth::{
    authenticate, authenticate_control_plane_operator, authenticate_control_plane_session,
    require_control_plane_csrf,
};
pub use deadline::request_deadline;
pub use host_origin::host_origin;
pub use preflight::{prevalidate_mcp, reject_non_post_mcp};
pub use sse_headers::inject_sse_headers;

// `ValidatedMcpRequest` is internal, but it is attached to a
// `Request::extensions()` slot and read back by `acquire_runtime`
// and by the transport layer, so it is `pub(crate)` and re-exported
// here for the same path stability.
pub(crate) use preflight::ValidatedMcpRequest;
