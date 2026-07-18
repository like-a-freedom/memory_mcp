//! Claim reconciliation telemetry — bounded metrics and trace events.
//!
//! Exposes six Prometheus metric families and structurally redacted
//! trace diagnostics. Zero-config: no recorder opens a socket without
//! the `prometheus` feature and `MEMORY_PROMETHEUS_LISTEN_ADDR`.

#![allow(dead_code)]

use crate::models::claim::ClaimSchemaFamily;

/// Schema label for Prometheus metrics.
pub(crate) fn schema_label(family: ClaimSchemaFamily) -> &'static str {
    match family {
        ClaimSchemaFamily::Attribute => "attribute",
        ClaimSchemaFamily::Quantity => "quantity",
        ClaimSchemaFamily::Relation => "relation",
        ClaimSchemaFamily::Commitment => "commitment",
    }
}

/// Outcome label for Prometheus metrics.
pub(crate) fn outcome_label(outcome: &str) -> &'static str {
    match outcome {
        "duplicate" => "duplicate",
        "supersession" => "supersession",
        "correction" => "correction",
        "contradiction" => "contradiction",
        "temporal_ambiguity" => "temporal_ambiguity",
        _ => "other",
    }
}
