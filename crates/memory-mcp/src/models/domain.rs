use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::provenance::Provenance;

/// Standard fact type classification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FactType {
    Note,
    Decision,
    Metric,
    Promise,
    Experience,
}

impl FactType {
    /// All standard fact types.
    pub const ALL: &'static [Self] = &[
        Self::Note,
        Self::Decision,
        Self::Metric,
        Self::Promise,
        Self::Experience,
    ];

    /// Returns the string representation for database storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Decision => "decision",
            Self::Metric => "metric",
            Self::Promise => "promise",
            Self::Experience => "experience",
        }
    }
}

impl std::fmt::Display for FactType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// An episode represents a unit of ingested content.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Episode {
    pub episode_id: String,
    pub source_type: String,
    pub source_id: String,
    pub content: String,
    pub t_ref: DateTime<Utc>,
    pub t_ingested: DateTime<Utc>,
    pub scope: String,
    pub visibility_scope: String,
    pub policy_tags: Vec<String>,
    /// Stable source-lineage identifier (e.g. `fs:docs/spec.md`) for episodes
    /// ingested by the filesystem watcher. Optional and never set by the public
    /// `ingest` path; claim projection prefers it over the versioned
    /// `source_id` as the reconciliation lineage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_lineage: Option<String>,
}

impl Episode {}

/// An entity represents a canonical named thing.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Entity {
    pub entity_id: String,
    pub entity_type: String,
    pub canonical_name: String,
    pub aliases: Vec<String>,
}

impl Entity {}

/// A fact represents a piece of knowledge extracted from an episode.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Fact {
    pub fact_id: String,
    pub fact_type: String,
    pub content: String,
    pub quote: String,
    pub source_episode: String,
    pub t_valid: DateTime<Utc>,
    pub t_ingested: DateTime<Utc>,
    pub t_invalid: Option<DateTime<Utc>>,
    pub t_invalid_ingested: Option<DateTime<Utc>>,
    pub confidence: f64,
    #[serde(default)]
    pub index_keys: Vec<String>,
    #[serde(default)]
    pub access_count: i64,
    #[serde(default)]
    pub last_accessed: Option<DateTime<Utc>>,
    pub entity_links: Vec<String>,
    pub scope: String,
    pub policy_tags: Vec<String>,
    pub provenance: Provenance,
    /// Full-text search relevance score (only present for FTS results).
    pub ft_score: f64,
}

/// Half-life and scaling constants for fact confidence decay.
impl Fact {
    /// Half-life in days for metric and promise fact confidence decay.
    pub const METRIC_HALF_LIFE_DAYS: f64 = 365.0;

    /// Half-life in days for general fact confidence decay.
    pub const DEFAULT_HALF_LIFE_DAYS: f64 = 180.0;

    /// Scaling factor for confidence rounding.
    pub const CONFIDENCE_SCALE: f64 = 10000.0;

    /// Returns true if the fact is active (not invalidated) as of the given timestamp.
    #[must_use]
    pub fn is_active(&self, as_of: DateTime<Utc>) -> bool {
        self.t_invalid.is_none_or(|t| t > as_of)
    }

    /// Calculates confidence decayed by half-life based on fact age.
    #[must_use]
    pub fn decayed_confidence(&self, now: DateTime<Utc>) -> f64 {
        let half_life_days = if self.fact_type == FactType::Metric.as_str()
            || self.fact_type == FactType::Promise.as_str()
            || self.fact_type == FactType::Decision.as_str()
        {
            Self::METRIC_HALF_LIFE_DAYS
        } else {
            Self::DEFAULT_HALF_LIFE_DAYS
        };
        let delta_days = (now - self.t_valid).num_days().max(0) as f64;
        let decay = 0.5_f64.powf(delta_days / half_life_days);
        (self.confidence * decay * Self::CONFIDENCE_SCALE).round() / Self::CONFIDENCE_SCALE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn a_fact_of_type(fact_type: &str, t_valid: DateTime<Utc>) -> Fact {
        Fact {
            fact_id: "fact:1".to_string(),
            fact_type: fact_type.to_string(),
            content: "test".to_string(),
            quote: "test".to_string(),
            source_episode: "episode:1".to_string(),
            t_valid,
            t_ingested: Utc::now(),
            t_invalid: None,
            t_invalid_ingested: None,
            confidence: 1.0,
            index_keys: vec![],
            access_count: 0,
            last_accessed: None,
            entity_links: vec![],
            scope: "org".to_string(),
            policy_tags: vec![],
            provenance: crate::models::Provenance::manual(),
            ft_score: 0.0,
        }
    }

    /// These four cases moved here from `service::query`, which held a
    /// one-line wrapper over this method and nothing else. They always
    /// tested this function; deleting the wrapper did not delete the
    /// coverage, and leaving them behind a removed name would not have
    /// tested anything.
    #[test]
    fn decayed_confidence_metric_uses_longer_half_life() {
        let fact = a_fact_of_type("metric", Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap());
        let now = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let confidence = fact.decayed_confidence(now);
        assert!(confidence > 0.4 && confidence < 0.6);
    }

    #[test]
    fn decayed_confidence_decision_uses_longer_half_life() {
        let fact = a_fact_of_type(
            "decision",
            Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap(),
        );
        let now = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let confidence = fact.decayed_confidence(now);
        assert!(confidence > 0.4 && confidence < 0.6);
    }

    #[test]
    fn decayed_confidence_general_uses_shorter_half_life() {
        let fact = a_fact_of_type("note", Utc.with_ymd_and_hms(2023, 7, 1, 0, 0, 0).unwrap());
        let now = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let confidence = fact.decayed_confidence(now);
        assert!(confidence > 0.4 && confidence < 0.6);
    }

    #[test]
    fn decayed_confidence_fresh_fact_has_high_confidence() {
        let fact = a_fact_of_type("note", Utc::now());
        let confidence = fact.decayed_confidence(Utc::now());
        assert!(confidence > 0.99);
    }
}

/// Origin of an edge (relationship between entities or facts).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EdgeOrigin {
    #[default]
    Extracted,
    Inferred,
    Ambiguous,
}

/// The attributes of an edge that say *how we know it*, separated from the
/// identity fields and the bi-temporal ones.
///
/// `Edge::relate` used to hardcode these four — `Inferred`, `1.0`, `0.8`,
/// `Provenance::manual()` — so no caller could record that a relationship
/// was stated by an operator rather than inferred, or that confidence was
/// anything other than 0.8. A business decision was hiding in a fixture
/// helper.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EdgeAttributes {
    pub origin: EdgeOrigin,
    pub strength: f64,
    pub confidence: f64,
    pub provenance: Provenance,
}

impl EdgeAttributes {
    /// The attributes `MemoryService::relate` used to hardcode.
    ///
    /// Kept so a caller that genuinely has nothing to say can say so
    /// explicitly, rather than the helper deciding on its behalf.
    pub fn inferred() -> Self {
        Self {
            origin: EdgeOrigin::Inferred,
            strength: 1.0,
            confidence: 0.8,
            provenance: Provenance::manual(),
        }
    }
}

/// An edge represents a relationship between entities or facts.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Edge {
    #[serde(rename = "in")]
    pub in_id: String,
    pub relation: String,
    #[serde(rename = "out")]
    pub out_id: String,
    #[serde(default)]
    pub origin: EdgeOrigin,
    pub strength: f64,
    pub confidence: f64,
    pub provenance: Provenance,
    pub t_valid: DateTime<Utc>,
    pub t_ingested: DateTime<Utc>,
    pub t_invalid: Option<DateTime<Utc>>,
    pub t_invalid_ingested: Option<DateTime<Utc>>,
}

/// A community groups related entities.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Community {
    pub community_id: String,
    pub member_entities: Vec<String>,
    pub summary: String,
    pub updated_at: DateTime<Utc>,
}
