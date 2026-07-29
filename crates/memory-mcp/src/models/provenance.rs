use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Structured provenance tracking for facts and edges.
///
/// Replaces the previous `serde_json::Value` provenance with typed fields
/// for reliable querying, filtering, and audit tracing.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Provenance {
    /// ID of the episode from which this fact was derived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_episode_id: Option<String>,

    /// URL or identifier of an external source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,

    /// How this fact entered the memory system.
    ///
    /// Known values: `manual`, `agent_observation`, `extraction`, `reflection`.
    #[serde(default = "Provenance::default_ingestion_method")]
    pub ingestion_method: String,

    /// Agent or system that created this fact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,

    /// Confidence of the source (0.0–1.0), if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_confidence: Option<f64>,

    /// Free-text justification for reflection-derived facts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence_basis: Option<String>,

    /// Extraction strategy used (e.g. `gliner`, `structured_summary`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extraction_strategy: Option<String>,

    /// Source type from the originating episode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,

    /// Source ID from the originating episode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
}

impl Provenance {
    fn default_ingestion_method() -> String {
        "manual".to_string()
    }

    /// Provenance for a manually created fact.
    pub fn manual() -> Self {
        Self {
            ingestion_method: "manual".to_string(),
            ..Default::default()
        }
    }

    /// Provenance for a fact derived from an agent observation.
    pub fn agent_observation(episode_id: impl Into<String>) -> Self {
        Self {
            source_episode_id: Some(episode_id.into()),
            ingestion_method: "agent_observation".to_string(),
            ..Default::default()
        }
    }

    /// Provenance for a fact extracted from an episode.
    pub fn extraction(
        episode_id: impl Into<String>,
        source_type: impl Into<String>,
        source_id: impl Into<String>,
        strategy: impl Into<String>,
    ) -> Self {
        Self {
            source_episode_id: Some(episode_id.into()),
            ingestion_method: "extraction".to_string(),
            source_type: Some(source_type.into()),
            source_id: Some(source_id.into()),
            extraction_strategy: Some(strategy.into()),
            ..Default::default()
        }
    }

    /// Convert to a `serde_json::Value` for storage and query-time enrichment.
    ///
    /// This preserves backward compatibility with existing DB records that
    /// store provenance as a JSON object.
    pub fn to_json_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_default()
    }

    /// Parse from a `serde_json::Value`, falling back to `Self::manual()`
    /// for malformed or non-object values.
    pub fn from_json_value(value: &serde_json::Value) -> Self {
        serde_json::from_value(value.clone()).unwrap_or_default()
    }
}

impl Default for Provenance {
    fn default() -> Self {
        Self {
            source_episode_id: None,
            source_url: None,
            ingestion_method: Self::default_ingestion_method(),
            created_by: None,
            source_confidence: None,
            confidence_basis: None,
            extraction_strategy: None,
            source_type: None,
            source_id: None,
        }
    }
}

/// Query-time enrichment keys added to provenance by the retrieval pipeline.
///
/// These are NOT stored in the database — they are computed at query time
/// and merged into the provenance object returned to callers.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ProvenanceEnrichment {
    /// Query terms that matched this fact's content or index keys.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matched_query_terms: Vec<String>,

    /// Graph traversal trace showing how this fact was reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_trace: Option<serde_json::Value>,
}

/// A single provenance source for a fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ProvenanceSource {
    /// Source episode ID.
    pub episode_id: String,
    /// Source episode content (excerpt).
    pub episode_content: String,
    /// Source episode timestamp.
    pub episode_t_ref: String,
    /// Relationship to fact: "direct" (created fact) or "linked" (via entity).
    pub relationship: String,
    /// Entity link path (if relationship is "linked").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_path: Option<String>,
}
