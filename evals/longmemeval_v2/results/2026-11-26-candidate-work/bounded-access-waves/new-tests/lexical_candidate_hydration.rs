//! First-hit FTS candidates must be unique before full source hydration.
mod common;
use memory_mcp::{knowledge::KnowledgeStoreClient, storage::DbClient};
use serde_json::json;

#[tokio::test]
async fn lexical_candidate_union_keeps_first_hit_score_and_all_source_fields() {
    let (_, db) = common::make_service_with_client().await;
    for (id, content, key, valid) in [
        (
            "fact:hydration_a",
            "Falcon Falcon deployment",
            "hydrationalpha",
            "2026-01-01T00:00:00Z",
        ),
        (
            "fact:hydration_b",
            "Falcon deployment deployment",
            "hydrationbeta",
            "2026-01-01T00:00:00Z",
        ),
        (
            "fact:hydration_future",
            "Falcon deployment",
            "future",
            "2027-01-01T00:00:00Z",
        ),
    ] {
        db.create(id,json!({"fact_id":id,"fact_type":"note","content":content,"quote":format!("Literal quote for {id}"),"source_episode":format!("episode:{key}"),
            "confidence":0.9,"index_keys":[key],"t_valid":valid,"t_ingested":valid,"scope":"org","policy_tags":[],"entity_links":[],"provenance":{"source":"manual"}}), "org", &["t_valid","t_ingested"]).await.unwrap();
    }
    let terms = [
        "hydrationbeta",
        "hydrationalpha",
        "absent",
        "falcon",
        "hydrationalpha",
        "hydrationbeta",
        "absent",
        "falcon",
        "hydrationbeta",
    ]
    .map(str::to_owned);
    let store = KnowledgeStoreClient::new(db.clone(), "org");
    let mut expected = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for term in &terms {
        let rows = db.query("SELECT fact_id, fact_type, content, quote, source_episode, t_valid, t_ingested, t_invalid, t_invalid_ingested, confidence, index_keys, access_count, last_accessed, entity_links, scope, policy_tags, provenance, search::score(1) AS ft_score FROM fact WHERE t_valid <= type::datetime($cutoff) AND (t_ingested IS NONE OR t_ingested <= type::datetime($cutoff)) AND (t_invalid IS NONE OR t_invalid > type::datetime($cutoff) OR t_invalid_ingested > type::datetime($cutoff)) AND (content @1@ __TERM__ OR index_keys @1@ __TERM__) ORDER BY ft_score DESC, t_valid DESC, fact_id ASC LIMIT $limit".replace("__TERM__",&serde_json::to_string(term).unwrap()).as_str(), Some(json!({"cutoff":"2026-05-01T00:00:00Z","limit":1})), "org").await.unwrap();
        for row in rows.as_array().unwrap() {
            if seen.insert(row["fact_id"].clone()) {
                expected.push(row.clone());
            }
        }
    }
    assert_eq!(
        expected.len(),
        2,
        "different independently limited terms must select different first winners"
    );
    let actual = store
        .select_lexical_fact_candidates("2026-05-01T00:00:00Z", &terms, 1, &[])
        .await
        .unwrap();
    assert_eq!(
        actual, expected,
        "first occurrence owns rank/score; later term duplicates must not be hydrated again"
    );
}
