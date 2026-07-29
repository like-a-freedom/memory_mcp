use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct ClaimCase {
    #[allow(dead_code)]
    id: String,
    split: CorpusSplit,
    #[serde(default)]
    #[allow(dead_code)]
    setup: Vec<SourceSample>,
    #[allow(dead_code)]
    source: SourceSample,
    expected: ExpectedCase,
    #[serde(default)]
    coverage: Vec<String>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CorpusSplit {
    Development,
    Test,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SourceSample {
    source_type: String,
    source_id: String,
    content: String,
    scope: String,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    policy_tags: Vec<String>,
    t_ref: String,
}

#[derive(Debug, Deserialize)]
struct ExpectedCase {
    #[serde(default)]
    claims: Vec<ExpectedClaim>,
    #[serde(default)]
    #[allow(dead_code)]
    relations: Vec<ExpectedRelation>,
    #[serde(default)]
    #[allow(dead_code)]
    skip_reason_codes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ExpectedClaim {
    schema: String,
    #[allow(dead_code)]
    subject: String,
    #[serde(default)]
    #[allow(dead_code)]
    comparison_key: BTreeMap<String, String>,
    #[allow(dead_code)]
    value: serde_json::Value,
    #[serde(default)]
    #[allow(dead_code)]
    qualifiers: BTreeMap<String, String>,
    #[allow(dead_code)]
    cardinality: String,
    #[serde(default)]
    #[allow(dead_code)]
    valid_from: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    valid_to: Option<String>,
    #[allow(dead_code)]
    source_span: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ExpectedRelation {
    setup_source_id: String,
    source_id: String,
    outcome: String,
    reason_code: String,
    #[serde(default)]
    predecessor_source_id: Option<String>,
    #[serde(default)]
    successor_source_id: Option<String>,
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("claim_reconciliation_cases.json")
}

fn load_cases() -> Vec<ClaimCase> {
    let raw = std::fs::read_to_string(fixture_path()).expect("read claim reconciliation fixture");
    serde_json::from_str(&raw).expect("parse claim reconciliation fixture")
}

#[test]
fn claim_fixture_covers_every_schema_outcome_and_isolation_boundary() {
    let cases = load_cases();

    let mut schemas_seen = BTreeSet::new();
    let mut outcomes_seen = BTreeSet::new();
    let mut has_duplicate = false;
    let mut has_coexistence = false;
    let mut has_not_comparable = false;
    let mut has_not_same_slot = false;
    let mut has_dev_split = false;
    let mut has_test_split = false;
    let mut all_coverage = BTreeSet::new();

    for case in &cases {
        for claim in &case.expected.claims {
            schemas_seen.insert(claim.schema.clone());
        }

        for relation in &case.expected.relations {
            outcomes_seen.insert(relation.outcome.clone());
            match relation.outcome.as_str() {
                "duplicate" => has_duplicate = true,
                "coexistence" => has_coexistence = true,
                _ => {}
            }
        }

        for code in &case.expected.skip_reason_codes {
            match code.as_str() {
                "not_comparable" => has_not_comparable = true,
                "not_same_slot" => has_not_same_slot = true,
                _ => {}
            }
        }

        for tag in &case.coverage {
            all_coverage.insert(tag.clone());
        }

        match case.split {
            CorpusSplit::Development => has_dev_split = true,
            CorpusSplit::Test => has_test_split = true,
        }
    }

    let required_schemas: BTreeSet<String> = [
        "attribute/v1".to_string(),
        "quantity/v1".to_string(),
        "relation/v1".to_string(),
        "commitment/v1".to_string(),
    ]
    .into_iter()
    .collect();
    assert!(
        schemas_seen.is_superset(&required_schemas),
        "missing schemas: {:?} (have {:?})",
        required_schemas
            .difference(&schemas_seen)
            .collect::<Vec<_>>(),
        schemas_seen
    );

    let required_outcomes: BTreeSet<String> = [
        "duplicate".to_string(),
        "contradiction".to_string(),
        "coexistence".to_string(),
        "supersession".to_string(),
        "correction".to_string(),
    ]
    .into_iter()
    .collect();
    assert!(
        outcomes_seen.is_superset(&required_outcomes),
        "missing outcomes: {:?} (have {:?})",
        required_outcomes
            .difference(&outcomes_seen)
            .collect::<Vec<_>>(),
        outcomes_seen
    );

    assert!(has_duplicate, "missing duplicate case");
    assert!(has_coexistence, "missing coexistence case");
    assert!(has_not_comparable, "missing not_comparable skip case");
    assert!(has_not_same_slot, "missing not_same_slot skip case");

    assert!(has_dev_split, "missing development split");
    assert!(has_test_split, "missing test split");

    let required_coverage: BTreeSet<String> = [
        "alias".to_string(),
        "unit_conversion".to_string(),
        "unknown_unit".to_string(),
        "missing_time".to_string(),
        "overlapping_interval".to_string(),
        "disjoint_interval".to_string(),
        "correction".to_string(),
        "supersession".to_string(),
        "cross_scope".to_string(),
        "cross_project".to_string(),
        "cross_policy".to_string(),
        "unresolved_subject".to_string(),
        "qualifier_mismatch".to_string(),
        "set_valued".to_string(),
        "domain_finance".to_string(),
        "domain_staffing".to_string(),
        "domain_delivery".to_string(),
        "domain_compliance".to_string(),
        "domain_incidents".to_string(),
        "domain_decisions".to_string(),
        "domain_preferences".to_string(),
        "domain_configuration".to_string(),
        "domain_commitments".to_string(),
        "domain_relations".to_string(),
        "structured_source".to_string(),
        "kv_source".to_string(),
        "free_sentence_source".to_string(),
    ]
    .into_iter()
    .collect();

    let missing_coverage: Vec<_> = required_coverage
        .difference(&all_coverage)
        .cloned()
        .collect();
    assert!(
        missing_coverage.is_empty(),
        "missing coverage tags: {:?} (have {:?})",
        missing_coverage,
        all_coverage
    );
}
