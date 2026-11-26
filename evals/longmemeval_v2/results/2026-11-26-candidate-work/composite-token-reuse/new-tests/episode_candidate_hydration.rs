//! Literal-substring episode candidates preserve per-term caps before hydration.
mod common;
use memory_mcp::{memory::episode_context_store::EpisodeContextStore, storage::DbClient};
use serde_json::json;

#[tokio::test]
async fn episode_membership_preserves_unicode_literal_and_blank_term_semantics() {
    let (_, db) = common::make_service_with_client().await;
    for (id, content) in [
        ("episode:literal_a", "Ёж"),
        ("episode:literal_b", "A.B"),
        ("episode:literal_c", "aXb"),
        ("episode:literal_d", "İ"),
    ] {
        db.create(id,json!({"episode_id":id,"source_type":"document","source_id":id,"content":content,"t_ref":"2026-01-01T00:00:00Z","t_ingested":"2026-01-01T00:00:00Z","scope":"org","policy_tags":[]}),"org", &["t_ref","t_ingested"]).await.unwrap();
    }
    let store = EpisodeContextStore::new(db.clone(), "org");
    let terms = ["ЁЖ", "a.b", "not-found", " \t", "İ"].map(str::to_owned);
    let rows = store
        .select_episode_candidates("2026-05-01T00:00:00Z", &terms, 2, &[])
        .await
        .unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| row["episode_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "episode:literal_a",
            "episode:literal_b",
            "episode:literal_d"
        ]
    );
    // Independent existing single-term owner reads establish the full-field
    // oracle, including blank-as-unfiltered, case folding and literal dots.
    let mut expected = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for term in &terms {
        for row in store
            .select_episodes_by_content("2026-05-01T00:00:00Z", Some(term), 2)
            .await
            .unwrap()
        {
            if seen.insert(row["episode_id"].clone()) {
                expected.push(row);
            }
        }
    }
    assert_eq!(rows, expected);
}

#[tokio::test]
async fn episode_term_candidates_return_each_payload_once_in_first_hit_order() {
    let (_, db) = common::make_service_with_client().await;
    for (id, content, time) in [
        (
            "episode:hydration_a",
            "alpha common",
            "2026-01-01T00:00:00Z",
        ),
        ("episode:hydration_b", "beta common", "2026-01-02T00:00:00Z"),
        (
            "episode:hydration_future",
            "alpha beta common",
            "2027-01-01T00:00:00Z",
        ),
        (
            "episode:hydration_c",
            "alpha common",
            "2026-01-01T00:00:00Z",
        ),
    ] {
        db.create(id,json!({"episode_id":id,"source_type":"chat","source_id":id,"content":content,"t_ref":time,"t_ingested":time,"scope":"org","policy_tags":[]}),"org", &["t_ref","t_ingested"]).await.unwrap();
    }
    let terms = [
        "alpha", "beta", "common", "alpha", "absent", "beta", "common", "alpha", "common",
    ]
    .map(str::to_owned);
    let store = EpisodeContextStore::new(db.clone(), "org");
    let rows = store
        .select_episode_candidates("2026-05-01T00:00:00Z", &terms, 1, &[])
        .await
        .unwrap();
    assert_eq!(
        rows.len(),
        2,
        "full source payload must be returned only on its first term hit; cap must not refill"
    );
    let mut expected = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for term in &terms {
        let records = db.query("SELECT * FROM episode WHERE t_ref <= type::datetime($cutoff) AND (t_ingested IS NONE OR t_ingested <= type::datetime($cutoff)) AND string::contains(string::lowercase(content), $query) ORDER BY t_ref DESC, episode_id ASC LIMIT $limit",Some(json!({"cutoff":"2026-05-01T00:00:00Z","query":term,"limit":1})),"org").await.unwrap();
        for row in records.as_array().unwrap() {
            if seen.insert(row["episode_id"].clone()) {
                expected.push(row.clone());
            }
        }
    }
    assert_eq!(rows, expected);
    assert_eq!(rows[0]["episode_id"], "episode:hydration_a");
    assert_eq!(rows[1]["episode_id"], "episode:hydration_b");
}

#[tokio::test]
async fn episode_candidate_later_batch_keeps_new_winner_without_refilling_seen_caps() {
    let (_, db) = common::make_service_with_client().await;
    for (id, content, valid, ingested) in [
        (
            "episode:seen",
            "alpha",
            "2026-01-02T00:00:00Z",
            "2026-01-02T00:00:00Z",
        ),
        (
            "episode:refill",
            "alpha",
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:00:00Z",
        ),
        (
            "episode:new",
            "gamma",
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:00:00Z",
        ),
        (
            "episode:future_ingested",
            "gamma",
            "2026-04-01T00:00:00Z",
            "2027-01-01T00:00:00Z",
        ),
    ] {
        db.create(id,json!({"episode_id":id,"source_type":"chat","source_id":id,"content":content,"t_ref":valid,"t_ingested":ingested,"scope":"org","policy_tags":[]}),"org", &["t_ref","t_ingested"]).await.unwrap();
    }
    let store = EpisodeContextStore::new(db, "org");
    let terms = [
        "ALPHA", "alpha", "alpha", "alpha", "alpha", "alpha", "alpha", "alpha", "gamma",
    ]
    .map(str::to_owned);
    let rows = store
        .select_episode_candidates("2026-05-01T00:00:00Z", &terms, 1, &["episode:seen".into()])
        .await
        .unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| row["episode_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["episode:new"]
    );
}
