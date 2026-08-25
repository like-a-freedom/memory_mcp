//! Filesystem-ingestion pipeline owned by the stdio MCP server.
//!
//! Discovery sources (startup scan + OS watcher) funnel into one durable inbox
//! revision store; a sequential processor runs `ingest → extract` per revision
//! from the durable prepared-content snapshot.

use std::time::Duration;

/// Interval between stability samples of a candidate file.
pub(crate) const STABILITY_SAMPLE_INTERVAL: Duration = Duration::from_millis(500);
/// Consecutive matching samples required before a candidate is accepted.
pub(crate) const STABILITY_REQUIRED_MATCHES: u8 = 2;
/// Upper bound for waiting on a file to stabilize.
pub(crate) const STABILITY_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(feature = "fs-watch")]
pub(crate) mod candidate;
#[cfg(feature = "fs-watch")]
pub mod processor;
#[cfg(feature = "fs-watch")]
pub mod runtime;
#[cfg(feature = "fs-watch")]
pub mod telemetry;
