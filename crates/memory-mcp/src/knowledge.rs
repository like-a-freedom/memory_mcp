//! Knowledge bounded context.
//!
//! Owns entities, aliases, facts, claims, triples,
//! communities, extraction and reconciliation, and knowledge
//! queries. It distinguishes contradiction, correction,
//! supersession and retraction, and keeps the single
//! bi-temporal close/write implementation.
//!
//! Knowledge does not own episode lifecycle or model runtime
//! management. Reads leave through owner-named scopes rather
//! than caller-supplied tables.

pub mod api;
