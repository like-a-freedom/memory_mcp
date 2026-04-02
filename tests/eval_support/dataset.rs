use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct RetrievalEvalCase {
    pub id: String,
    pub description: String,
    pub episodes: Vec<EvalEpisode>,
    pub query: RetrievalQuery,
    pub expected: RetrievalExpectation,
}

#[derive(Debug, Deserialize, Clone)]
pub struct EvalEpisode {
    pub source_type: String,
    pub source_id: String,
    pub content: String,
    pub t_ref: String,
    pub scope: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RetrievalQuery {
    pub query: String,
    pub scope: String,
    pub budget: i32,
    pub as_of: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RetrievalExpectation {
    pub must_contain: Vec<String>,
    pub must_not_contain: Vec<String>,
    pub expect_empty: bool,
    pub min_recall_at_k: f64,
    /// Retrieval ability tier for this case.
    /// One of: "direct", "alias", "temporal", "graph", "reasoning"
    #[serde(default = "default_tier")]
    pub tier: String,
}

fn default_tier() -> String {
    "direct".to_string()
}

#[derive(Debug, Deserialize, Clone)]
pub struct ExtractionEvalCase {
    pub id: String,
    pub description: String,
    pub content: String,
    pub scope: String,
    pub t_ref: String,
    pub expected: ExtractionExpectation,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ExtractionExpectation {
    pub entities: Vec<ExpectedEntity>,
    pub fact_types: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ExpectedEntity {
    pub entity_type: String,
    pub canonical_name: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LatencyEvalCase {
    pub id: String,
    pub description: String,
    pub query: String,
    pub scope: String,
    pub episode_count: usize,
    pub warmup_iterations: usize,
    pub measured_iterations: usize,
}

pub fn parse_retrieval_cases(input: &str) -> Result<Vec<RetrievalEvalCase>, String> {
    let cases: Vec<RetrievalEvalCase> =
        serde_json::from_str(input).map_err(|err| err.to_string())?;
    validate_retrieval_cases(&cases)?;
    Ok(cases)
}

pub fn parse_extraction_cases(input: &str) -> Result<Vec<ExtractionEvalCase>, String> {
    let cases: Vec<ExtractionEvalCase> =
        serde_json::from_str(input).map_err(|err| err.to_string())?;
    // fact_types may be empty for entity-only eval cases
    Ok(cases)
}

pub fn parse_latency_cases(input: &str) -> Result<Vec<LatencyEvalCase>, String> {
    let cases: Vec<LatencyEvalCase> = serde_json::from_str(input).map_err(|err| err.to_string())?;
    if cases.iter().any(|case| case.measured_iterations == 0) {
        return Err("latency case must declare measured_iterations > 0".to_string());
    }
    Ok(cases)
}

const VALID_TIERS: &[&str] = &["direct", "alias", "temporal", "graph", "reasoning"];

fn validate_retrieval_cases(cases: &[RetrievalEvalCase]) -> Result<(), String> {
    let mut ids = std::collections::BTreeSet::new();
    for case in cases {
        if !ids.insert(case.id.clone()) {
            return Err(format!("duplicate retrieval case id: {}", case.id));
        }
        if case.expected.expect_empty && !case.expected.must_contain.is_empty() {
            return Err(format!(
                "retrieval case {} cannot set expect_empty=true and must_contain simultaneously",
                case.id
            ));
        }
        if case.query.budget <= 0 {
            return Err(format!("retrieval case {} must use budget > 0", case.id));
        }
        if !VALID_TIERS.contains(&case.expected.tier.as_str()) {
            return Err(format!(
                "retrieval case {} has invalid tier '{}', expected one of {:?}",
                case.id, case.expected.tier, VALID_TIERS
            ));
        }
    }
    Ok(())
}
