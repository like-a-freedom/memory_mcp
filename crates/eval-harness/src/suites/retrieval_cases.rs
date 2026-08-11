//! Shared case layer for the retrieval and response-size suites.
//!
//! Both suites score the same seeded retrieval fixture
//! (`tests/fixtures/retrieval_cases.json`). The corpus shape, loader, and
//! as-of derivation live here once so the two suites cannot drift: a schema
//! change to the fixture is a one-module change, and cross-suite drift
//! becomes a compile error instead of silent divergence.

use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

use crate::error::EvalError;

#[derive(Debug, Deserialize)]
pub(super) struct RetrievalEvalCase {
    pub(super) id: String,
    #[allow(dead_code)]
    pub(super) description: String,
    pub(super) query: String,
    pub(super) scope: String,
    #[serde(default)]
    pub(super) project: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub(super) tags: Vec<String>,
    #[serde(default = "default_budget")]
    pub(super) budget: i32,
    pub(super) facts: Vec<SeedFact>,
    #[serde(default)]
    pub(super) entities: Vec<SeedEntity>,
    #[serde(default)]
    pub(super) communities: Vec<SeedCommunity>,
    #[serde(default)]
    pub(super) edges: Vec<SeedEdge>,
    pub(super) expected: RetrievalExpectation,
}

#[derive(Debug, Deserialize)]
pub(super) struct SeedFact {
    pub(super) content: String,
    pub(super) t_valid: String,
    #[serde(default)]
    pub(super) project: Option<String>,
    #[serde(default)]
    pub(super) source_id: Option<String>,
    #[serde(default)]
    pub(super) entity_links: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SeedEntity {
    pub(super) entity_id: String,
    pub(super) entity_type: String,
    pub(super) canonical_name: String,
    #[serde(default)]
    pub(super) aliases: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SeedCommunity {
    pub(super) community_id: String,
    pub(super) member_entities: Vec<String>,
    pub(super) summary: String,
    pub(super) updated_at: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct SeedEdge {
    pub(super) from_id: String,
    pub(super) relation: String,
    pub(super) to_id: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct RetrievalExpectation {
    #[allow(dead_code)]
    pub(super) tier: String,
    pub(super) must_contain: Vec<String>,
    #[serde(default)]
    pub(super) must_not_contain: Vec<String>,
    #[serde(default = "default_min_recall_at_k")]
    pub(super) min_recall_at_k: f64,
}

fn default_budget() -> i32 {
    5
}

fn default_min_recall_at_k() -> f64 {
    1.0
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/retrieval_cases.json")
}

pub(super) fn load_cases() -> Result<Vec<RetrievalEvalCase>, EvalError> {
    let raw = std::fs::read_to_string(fixture_path()).map_err(|source| EvalError::Io {
        path: fixture_path(),
        source,
    })?;
    serde_json::from_str(&raw).map_err(EvalError::Artifact)
}

pub(super) fn case_as_of(case: &RetrievalEvalCase) -> DateTime<Utc> {
    let latest = case
        .facts
        .iter()
        .filter_map(|f| f.t_valid.parse::<DateTime<Utc>>().ok())
        .chain(
            case.communities
                .iter()
                .filter_map(|c| c.updated_at.parse::<DateTime<Utc>>().ok()),
        )
        .max()
        .unwrap_or_else(Utc::now);

    std::cmp::max(Utc::now(), latest) + Duration::seconds(1)
}
