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
}

/// An entity represents a canonical named thing.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Entity {
    pub entity_id: String,
    pub entity_type: String,
    pub canonical_name: String,
    pub aliases: Vec<String>,
}

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

/// Origin of an edge (relationship between entities or facts).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EdgeOrigin {
    #[default]
    Extracted,
    Inferred,
    Ambiguous,
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
