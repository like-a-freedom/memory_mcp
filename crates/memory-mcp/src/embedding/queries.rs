//! The temporal columns of the embedding context's own tables.
//!
//! `DbClient::create` and `build_upsert_query` take the field list from the
//! caller rather than looking it up, because which columns are datetimes is a
//! property of the table and the table belongs to a context. This is the
//! embedding context naming its two.

/// The temporal columns an embedding-state row carries.
pub const EMBEDDING_STATE_TEMPORAL_FIELDS: &[&str] = &["updated_at"];

/// The temporal columns an embedding-job row carries.
pub const EMBEDDING_JOB_TEMPORAL_FIELDS: &[&str] =
    &["requested_at", "started_at", "updated_at", "finished_at"];
