//! Database persistence mechanisms.
//!
//! The connection, the query seam and the retry classification are
//! technical: they know about SurrealDB and about transactions, and
//! nothing about facts, episodes or sessions. Which *table* a query
//! touches is an owner decision and lives in that owner's store.

pub mod db_errors;
pub mod transactions;
