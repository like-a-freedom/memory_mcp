//! HTTP SaaS profile (ADR-0052). Gated on `streamable-http` in lib.rs:
//! `#[cfg(feature = "streamable-http")] pub mod http;`

pub mod config;
// Later tasks append, each together with the file it creates:
//   pub mod shutdown; pub mod health; pub mod transport; pub mod router;
//   pub mod server; pub mod middleware; pub mod metrics; pub mod validation;
//   pub mod principal; pub mod registry; pub mod runtime;
//   #[cfg(feature = "control-plane")] pub mod control;
