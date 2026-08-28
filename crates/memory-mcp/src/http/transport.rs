//! HTTP SaaS transport (ADR-0052). Phase 3+ implementation lives here.
//!
//! # rmcp 3.1.2 API surface (verified 2026-08-28)
//!
//! The following types and methods are confirmed against the installed
//! `rmcp 3.1.2` source. Line numbers are stable while `Cargo.lock`
//! resolves the `rmcp` dep to `3.1.2` (currently pinned via the
//! workspace entry in `Cargo.toml`).
//!
//! - `rmcp::transport::streamable_http_server::StreamableHttpServerConfig`
//!   (`src/transport/streamable_http_server/tower.rs:60`).
//! - `rmcp::transport::streamable_http_server::StreamableHttpService<S, M>`
//!   (`src/transport/streamable_http_server/tower.rs:999`).
//! - `rmcp::transport::streamable_http_server::session::never::NeverSessionManager`
//!   (`src/transport/streamable_http_server/session/never.rs:19`).
//!
//! `StreamableHttpServerConfig` builder methods
//! (`src/transport/streamable_http_server/tower.rs`):
//!
//! - `with_allowed_hosts(impl IntoIterator<Item = String>)`            line 182
//! - `with_allowed_origins(impl IntoIterator<Item = String>)`          line 194
//! - `with_sse_keep_alive(Option<Duration>)`                           line 206
//! - `with_sse_retry(Option<Duration>)`                                line 211
//! - `with_legacy_session_mode(bool)`                                  line 216
//! - `with_json_response(bool)`                                        line 221
//! - `with_cancellation_token(CancellationToken)`                      line 226
//! - `with_max_request_body_bytes(usize)`                              line 232
//! - `with_stateless_protocol_metadata_required(bool)`                line 241
//!
//! `ServerHandler::supported_protocol_versions` has a default impl that
//! returns `Cow::Borrowed(ProtocolVersion::KNOWN_VERSIONS)`. The HTTP
//! profile overrides it to advertise only `V_2026_07_28`.
//!
//! `rmcp::model::ProtocolVersion` is a newtype struct with associated
//! constants `V_2024_11_05`, `V_2025_03_26`, `V_2025_06_18`,
//! `V_2025_11_25`, `V_2026_07_28`. `LATEST == V_2025_11_25`. The HTTP
//! profile pins both `supported_protocol_versions` and the `get_info()`
//! `protocol_version` fallback to `V_2026_07_28`.
