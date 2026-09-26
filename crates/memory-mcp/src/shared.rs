//! The pure kernel: values and vocabulary every bounded context may
//! share.
//!
//! The spec allows `shared` to contain runtime- and
//! storage-independent error variants, validated identifiers and
//! temporal value semantics. It explicitly forbids the `models/`
//! tree, a service container, the registry, a database client, SQL,
//! config parsing, a global cache and a worker runtime — those stay
//! with their owner or in `platform`.
//!
//! A value belongs here only when both sides share its *meaning*,
//! not because the fields happen to look alike. That is why
//! `error.rs` moved here in full but `service/value_helpers.rs`
//! did not: reading a database row is persistence work, not a pure
//! value semantic.

pub mod error;
pub mod temporal;
