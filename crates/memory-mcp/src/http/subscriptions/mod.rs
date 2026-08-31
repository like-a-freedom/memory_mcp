//! Durable subscriptions and transactional outbox (spec §11).
//!
//! The outbox ensures every canonical mutation atomically
//! increments a tenant-local sequence and emits a
//! `TenantChangeEvent`. Subscriptions/listen reads from
//! this log. The cross-replica wake uses SurrealDB LIVE
//! queries with outbox-based polling fallback.

pub mod outbox;
