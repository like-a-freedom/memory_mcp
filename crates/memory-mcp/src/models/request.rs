use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::access::AccessPayload;
use super::provenance::ProvenanceSource;

/// Request to ingest a new episode into memory.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IngestRequest {
    pub source_type: String,
    pub source_id: String,
    pub content: String,
    pub t_ref: DateTime<Utc>,
    #[serde(default = "super::default_scope")]
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub t_ingested: Option<DateTime<Utc>>,
    pub visibility_scope: Option<String>,
    #[serde(default)]
    pub policy_tags: Vec<String>,
}

/// Request to explain context items with source citations.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExplainRequest {
    pub context_pack: Vec<ExplainItem>,
    /// Request compact (token-efficient) response. Defaults to true.
    #[serde(
        default = "crate::tools::parsers::default_compact",
        skip_serializing_if = "is_default_true"
    )]
    #[schemars(skip)]
    pub compact: bool,
}

/// A single item to explain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ExplainItem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact_id: Option<String>,
    pub content: String,
    /// Exact source text. Omitted under compact=true because it duplicates `content`.
    #[serde(
        default,
        skip_serializing_if = "crate::tools::compact::skip_if_compact"
    )]
    pub quote: String,
    pub source_episode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t_ref: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t_ingested: Option<DateTime<Utc>>,
    #[serde(default)]
    pub provenance: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citation_context: Option<String>,
    /// All provenance sources for this fact (direct + linked episodes).
    #[serde(default)]
    pub all_sources: Vec<ProvenanceSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_insights: Option<GraphInsights>,
    /// Age of the fact in days (computed from t_valid vs now).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact_age_days: Option<i64>,
    /// Confidence after applying time-based decay.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "super::rounding::round_2_opt"
    )]
    pub decayed_confidence: Option<f64>,
    /// How this fact entered the memory system (e.g. "manual", "extraction").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingestion_method: Option<String>,
}

impl Default for ExplainItem {
    fn default() -> Self {
        Self {
            fact_id: None,
            content: String::new(),
            quote: String::new(),
            source_episode: String::new(),
            scope: None,
            t_ref: None,
            t_ingested: None,
            provenance: serde_json::Value::Null,
            citation_context: None,
            all_sources: Vec::new(),
            graph_insights: None,
            fact_age_days: None,
            decayed_confidence: None,
            ingestion_method: None,
        }
    }
}

/// Ranked hub entities and cross-community paths relevant to an explained fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GraphInsights {
    #[serde(default)]
    pub hub_entities: Vec<GraphHubEntity>,
    #[serde(default)]
    pub surprising_connections: Vec<SurprisingConnection>,
}

/// A high-degree entity in the current graph neighborhood.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GraphHubEntity {
    pub entity_id: String,
    pub canonical_name: String,
    pub degree: usize,
}

/// A short cross-community path that may reveal a non-obvious connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct SurprisingConnection {
    pub source_entity_id: String,
    pub source_entity_name: String,
    pub target_entity_id: String,
    pub target_entity_name: String,
    pub hop_count: usize,
    #[serde(default)]
    pub path: Vec<String>,
}

/// Request to extract entities and facts from an episode.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExtractRequest {
    pub episode_id: String,
}

/// Entity candidate for resolution.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EntityCandidate {
    pub entity_type: String,
    pub canonical_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// Request to invalidate a fact.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InvalidateRequest {
    pub fact_id: String,
    pub reason: String,
    pub t_invalid: DateTime<Utc>,
}

/// Request to assemble context for a query.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AssembleContextRequest {
    pub query: String,
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default)]
    pub fact_types: Vec<String>,
    pub as_of: Option<DateTime<Utc>>,
    #[serde(default = "super::default_budget")]
    pub budget: i32,
    #[serde(default)]
    pub view_mode: Option<String>,
    #[serde(default)]
    pub window_start: Option<DateTime<Utc>>,
    #[serde(default)]
    pub window_end: Option<DateTime<Utc>>,
    #[serde(skip_serializing, default)]
    #[schemars(skip)]
    pub access: Option<AccessPayload>,
    /// Request compact (token-efficient) response. Defaults to true.
    #[serde(
        default = "crate::tools::parsers::default_compact",
        skip_serializing_if = "is_default_true"
    )]
    #[schemars(skip)]
    pub compact: bool,
}

// `skip_serializing_if` target for `compact` — skips when the value is the default `true`.
fn is_default_true(b: &bool) -> bool {
    *b
}

/// A compact extracted entity returned by the MCP `extract` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExtractedEntity {
    pub entity_id: String,
    #[serde(rename = "type")]
    pub entity_type: String,
    pub canonical_name: String,
}

/// A compact extracted fact returned by the MCP `extract` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExtractedFact {
    pub fact_id: String,
    #[serde(rename = "type")]
    pub fact_type: String,
}

/// A relationship link produced during extraction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExtractedLink {
    pub entity_id: String,
    pub episode_id: String,
}

/// Structured result returned by the MCP `extract` tool.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ExtractResult {
    pub episode_id: String,
    pub entities: Vec<ExtractedEntity>,
    pub facts: Vec<ExtractedFact>,
    pub links: Vec<ExtractedLink>,
    #[serde(default)]
    pub warnings: Vec<ContradictionWarning>,
    /// Optional reconciliation metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconciliation: Option<ReconciliationSummary>,
}

impl ExtractResult {
    /// Returns an empty extraction result for partial or no-input responses.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }
}

/// A non-blocking warning about a newly extracted fact that may contradict an active fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ContradictionWarning {
    pub fact_type: String,
    pub new_fact_id: String,
    pub conflicting_fact_id: String,
    pub existing_content: String,
    pub new_content: String,
    #[serde(default)]
    pub entity_ids: Vec<String>,
    pub reason: String,
}

/// Summary of claim reconciliation for an extract operation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ReconciliationSummary {
    pub status: ReconciliationStatus,
    pub claims_projected: usize,
    pub active_relations: usize,
    #[serde(default)]
    pub reason_codes: Vec<String>,
}

/// Status of claim reconciliation processing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationStatus {
    Complete,
    Pending,
    Partial,
    Failed,
    #[default]
    Unsupported,
}

/// Reconciliation metadata for a single context item.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ClaimReconciliationMetadata {
    #[serde(default)]
    pub claim_ids: Vec<String>,
    #[serde(default)]
    pub relations: Vec<ClaimRelationSummary>,
}

/// Summary of a single relation for public exposure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ClaimRelationSummary {
    pub relation_id: String,
    pub outcome: crate::models::claim::ClaimRelationOutcome,
    pub counterpart_fact_id: String,
    pub counterpart_source_episode_id: String,
    pub reason_code: String,
    pub evaluator_version: String,
}

/// A ranked context item returned by the MCP `assemble_context` tool.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct AssembledContextItem {
    pub fact_id: String,
    pub content: String,
    /// Exact source text. Omitted under compact=true because it duplicates `content`.
    #[serde(
        default,
        skip_serializing_if = "crate::tools::compact::skip_if_compact"
    )]
    pub quote: String,
    pub source_episode: String,
    #[serde(serialize_with = "super::rounding::round_2")]
    pub confidence: f64,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "super::rounding::round_2_opt"
    )]
    pub relevance: Option<f64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "super::rounding::round_2_opt"
    )]
    pub grounding: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_available: Option<bool>,
    pub provenance: serde_json::Value,
    /// Rationale for ranking. Under compact=true, serialized as `tier=<tier>` only.
    #[serde(serialize_with = "crate::tools::compact::serialize_rationale")]
    pub rationale: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieval_tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconciliation: Option<ClaimReconciliationMetadata>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    // --- AssembledContextItem serialization ---

    #[test]
    fn assembled_context_item_rounds_confidence_to_two_dp() {
        let item = AssembledContextItem {
            confidence: 0.8500000000000001,
            ..Default::default()
        };
        let val = serde_json::to_value(&item).unwrap();
        assert_eq!(val["confidence"], json!(0.85));
    }

    #[test]
    fn assembled_context_item_omits_relevance_when_none() {
        let item = AssembledContextItem {
            confidence: 0.9,
            relevance: None,
            ..Default::default()
        };
        let val = serde_json::to_value(&item).unwrap();
        assert!(!val.as_object().unwrap().contains_key("relevance"));
    }

    #[test]
    fn assembled_context_item_rounds_relevance_when_some() {
        let item = AssembledContextItem {
            confidence: 0.9,
            relevance: Some(0.3333333333333333),
            ..Default::default()
        };
        let val = serde_json::to_value(&item).unwrap();
        assert_eq!(val["relevance"], json!(0.33));
    }

    #[test]
    fn assembled_context_item_omits_grounding_when_none() {
        let item = AssembledContextItem {
            confidence: 0.9,
            grounding: None,
            ..Default::default()
        };
        let val = serde_json::to_value(&item).unwrap();
        assert!(!val.as_object().unwrap().contains_key("grounding"));
    }

    #[test]
    fn assembled_context_item_rounds_grounding_when_some() {
        let item = AssembledContextItem {
            confidence: 0.9,
            grounding: Some(0.999999999),
            ..Default::default()
        };
        let val = serde_json::to_value(&item).unwrap();
        assert_eq!(val["grounding"], json!(1.0));
    }

    #[test]
    fn assembled_context_item_rounds_all_f64_fields_together() {
        let item = AssembledContextItem {
            confidence: 1.23456789,
            relevance: Some(0.555555555),
            grounding: Some(0.0049999999),
            ..Default::default()
        };
        let val = serde_json::to_value(&item).unwrap();
        assert_eq!(val["confidence"], json!(1.23));
        assert_eq!(val["relevance"], json!(0.56));
        assert_eq!(val["grounding"], json!(0.0));
    }

    // --- Compact-mode serialization (guarded responses) ---

    #[test]
    fn assembled_context_item_compact_omits_quote() {
        let mut item = AssembledContextItem {
            fact_id: "fact:a".to_string(),
            content: "The system scales.".to_string(),
            quote: "The system scales.".to_string(),
            rationale: "tier=direct ...".to_string(),
            ..Default::default()
        };
        {
            let _guard = crate::tools::compact::set_compact(true);
            let val = serde_json::to_value(&item).unwrap();
            assert!(
                val.get("quote").is_none(),
                "compact serialization must omit quote"
            );
        }
        // Guard dropped — verbose path must show quote again:
        item.quote = "The system scales.".to_string();
        let val = serde_json::to_value(&item).unwrap();
        assert_eq!(val["quote"].as_str().unwrap(), "The system scales.");
    }

    #[test]
    fn assembled_context_item_compact_slims_rationale_to_tier() {
        let item = AssembledContextItem {
            rationale: "tier=direct fts=0.85 access_count=3 confidence=0.92 semantic=enabled"
                .to_string(),
            ..Default::default()
        };
        let _guard = crate::tools::compact::set_compact(true);
        let val = serde_json::to_value(&item).unwrap();
        assert_eq!(val["rationale"].as_str().unwrap(), "tier=direct");
    }

    #[test]
    fn assembled_context_item_verbose_keeps_full_rationale() {
        let full = "tier=direct fts=0.85 access_count=3 confidence=0.92".to_string();
        let item = AssembledContextItem {
            rationale: full.clone(),
            ..Default::default()
        };
        let _guard = crate::tools::compact::set_compact(false);
        let val = serde_json::to_value(&item).unwrap();
        assert_eq!(val["rationale"].as_str().unwrap(), full.as_str());
    }

    #[test]
    fn explain_item_compact_omits_quote() {
        let item = ExplainItem {
            content: "Budget $2M.".to_string(),
            quote: "Budget $2M.".to_string(),
            source_episode: "episode:budget".to_string(),
            ..Default::default()
        };
        let _guard = crate::tools::compact::set_compact(true);
        let val = serde_json::to_value(&item).unwrap();
        assert!(val.get("quote").is_none(), "compact must omit quote");
        assert_eq!(val["content"].as_str().unwrap(), "Budget $2M.");
    }

    #[test]
    fn explain_item_compact_keeps_citation_and_sources() {
        let item = ExplainItem {
            content: "Budget $2M.".to_string(),
            source_episode: "episode:budget".to_string(),
            citation_context: Some("Full budget breakdown ...".to_string()),
            ..Default::default()
        };
        let _guard = crate::tools::compact::set_compact(true);
        let val = serde_json::to_value(&item).unwrap();
        assert_eq!(
            val["citation_context"].as_str().unwrap(),
            "Full budget breakdown ..."
        );
    }

    // --- ExplainItem serialization ---

    #[test]
    fn explain_item_rounds_decayed_confidence_when_some() {
        let item = ExplainItem {
            content: "test".to_string(),
            quote: "test".to_string(),
            source_episode: "ep:1".to_string(),
            decayed_confidence: Some(0.123456789),
            ..Default::default()
        };
        let val = serde_json::to_value(&item).unwrap();
        assert_eq!(val["decayed_confidence"], json!(0.12));
    }

    #[test]
    fn explain_item_omits_decayed_confidence_when_none() {
        let item = ExplainItem {
            content: "test".to_string(),
            quote: "test".to_string(),
            source_episode: "ep:1".to_string(),
            decayed_confidence: None,
            ..Default::default()
        };
        let val = serde_json::to_value(&item).unwrap();
        assert!(!val.as_object().unwrap().contains_key("decayed_confidence"));
    }

    #[test]
    fn explain_item_rounds_decayed_confidence_full_precision() {
        let item = ExplainItem {
            content: "test".to_string(),
            quote: "test".to_string(),
            source_episode: "ep:1".to_string(),
            decayed_confidence: Some(0.8999999999999999),
            ..Default::default()
        };
        let val = serde_json::to_value(&item).unwrap();
        assert_eq!(
            val["decayed_confidence"].as_f64().unwrap(),
            0.9,
            "0.8999999999999999 should round to 0.9"
        );
    }
}
