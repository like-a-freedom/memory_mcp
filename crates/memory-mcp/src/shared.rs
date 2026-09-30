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
pub mod ids;
pub mod observability;
pub mod search;
pub mod search_lexical;
pub mod temporal;
pub mod triple_extractor;
pub mod validation;

// No root re-exports: every call site names the module it needs
// (`shared::ids::deterministic_fact_id`, `shared::search::normalize_text`,
// `shared::validation::validate_fact_input`). A facade here would be a
// second path to the same item with no owner and no user.
