//! End-to-end quality checks using the production Anno extractor.

mod common;

use memory_mcp::models::{ExtractResult, IngestRequest};
use memory_mcp::service::MemoryService;
use memory_mcp::service::memory_container_shims::memory_capabilities_extract::ExtractCapability;
use memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability;
use memory_mcp::storage::DbClient;

async fn extract(service: &MemoryService, content: &str) -> ExtractResult {
    let id = IngestCapability::ingest_from_service(
        service,
        IngestRequest {
            source_context: None,
            source_type: "conversation".into(),
            source_id: "quality-corpus".into(),
            content: content.into(),
            t_ref: chrono::Utc::now(),
            t_ingested: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .expect("ingest");
    ExtractCapability::extract_from_service(service, &id, None, None)
        .await
        .expect("extract")
}

#[tokio::test]
async fn date_and_percentage_mentions_do_not_become_entities() {
    let tm = common::TestMemory::new(false).await;
    let result = extract(&tm.service, "Порог: 95% 2026-10-24. Тренировка выполнена.").await;
    assert!(
        result.entities.is_empty(),
        "scalar mentions are not entities: {:?}",
        result.entities
    );
    assert!(result.links.is_empty());
    let rows = tm
        .db_client
        .query(
            "SELECT episode_id FROM episode WHERE episode_id = $id",
            Some(serde_json::json!({"id": result.episode_id})),
            "org",
        )
        .await
        .expect("read source episode");
    assert!(
        !serde_json::from_value::<Vec<serde_json::Value>>(rows)
            .expect("rows")
            .is_empty()
    );
}

#[tokio::test]
async fn supported_multilingual_names_remain_linked() {
    let tm = common::TestMemory::new(false).await;
    let result = extract(&tm.service, "Иван Петров met Alice Smith.").await;
    let names: Vec<_> = result
        .entities
        .iter()
        .map(|entity| entity.canonical_name.as_str())
        .collect();
    assert!(names.contains(&"Иван Петров"), "{names:?}");
    assert!(names.contains(&"Alice Smith"), "{names:?}");
    assert_eq!(result.links.len(), result.entities.len());
}

#[tokio::test]
async fn grammatical_chinese_subject_is_linked_without_inventing_a_person_type() {
    let tm = common::TestMemory::new(false).await;
    let result = extract(&tm.service, "张三报告ARR为500万美元。").await;
    assert!(
        result
            .entities
            .iter()
            .any(|entity| entity.canonical_name == "张三" && entity.entity_type == "concept")
    );
    assert!(!result.facts.is_empty());
}

#[tokio::test]
async fn grammatical_subject_does_not_span_sentences() {
    let tm = common::TestMemory::new(false).await;
    let result = extract(
        &tm.service,
        "ARR grew to $3M. I will send the update by Friday.",
    )
    .await;
    assert!(
        !result
            .entities
            .iter()
            .any(|entity| entity.canonical_name.contains("$3M.")),
        "a preceding sentence cannot become a grammatical subject: {:?}",
        result.entities
    );
}

#[tokio::test]
async fn unresolved_first_person_promise_does_not_invent_an_identity() {
    let tm = common::TestMemory::new(false).await;
    let result = extract(&tm.service, "I will send the update by Friday.").await;
    assert!(
        !result
            .entities
            .iter()
            .any(|entity| entity.canonical_name == "I")
    );
}

#[tokio::test]
async fn compound_reporter_does_not_invent_a_group_identity() {
    let tm = common::TestMemory::new(false).await;
    let result = extract(&tm.service, "Alice Smith and Bob Jones reports ARR is $3M.").await;
    assert!(
        !result
            .entities
            .iter()
            .any(|entity| entity.canonical_name == "Alice Smith and Bob Jones")
    );
}

#[tokio::test]
async fn explicit_resolution_remains_available_for_scalar_names() {
    let tm = common::TestMemory::new(false).await;
    let id = common::resolve_entity(&tm.service, "concept", "95%")
        .await
        .expect("manual resolve");
    assert!(id.starts_with("entity:"));
}

#[tokio::test]
async fn genuine_conflicts_are_all_returned() {
    let tm = common::TestMemory::new(false).await;
    let subject = common::resolve_entity(&tm.service, "person", "Alice Smith")
        .await
        .expect("resolve subject");
    for value in 0..20 {
        common::seed_fact_with_links(
            &tm.service,
            "org",
            &format!("Alice Smith reports ARR is ${value}M."),
            "2026-10-01T00:00:00Z".parse().expect("timestamp"),
            vec![subject.clone()],
        )
        .await;
    }
    let result = extract(&tm.service, "Alice Smith reports ARR is $25M.").await;
    assert_eq!(
        result.warnings.len(),
        20,
        "every conflicting fact must remain visible"
    );
    let counterparts: std::collections::HashSet<_> = result
        .warnings
        .iter()
        .map(|w| &w.conflicting_fact_id)
        .collect();
    assert_eq!(counterparts.len(), 20);
    assert!(
        result
            .warnings
            .iter()
            .all(|w| !w.existing_content.is_empty() && !w.new_content.is_empty())
    );
}

/// Reproducible local workload; run separately with `--ignored --nocapture`.
#[tokio::test]
#[ignore = "manual performance profile"]
async fn profile_extract_quality_corpus() {
    for (label, count) in [("small", 5), ("medium", 20), ("large", 50)] {
        let tm = common::TestMemory::new(false).await;
        let mut content = String::from("Facts:\n");
        for index in 0..count {
            content.push_str(&format!(
                "- Historical record: {{\"date\":\"2026-10-{:02}\",\"duration\":{}}}\n",
                1 + index % 28,
                index + 1
            ));
        }
        let start = std::time::Instant::now();
        let result = extract(&tm.service, &content).await;
        let elapsed = start.elapsed();
        let bytes = serde_json::to_vec(&result).expect("serialize").len();
        eprintln!(
            "case={label} input_bytes={} elapsed_ms={} facts={} entities={} warnings={} output_bytes={bytes}",
            content.len(),
            elapsed.as_millis(),
            result.facts.len(),
            result.entities.len(),
            result.warnings.len()
        );
        if label == "large" {
            let start = std::time::Instant::now();
            let repeated = ExtractCapability::extract_from_service(
                &tm.service,
                &result.episode_id,
                None,
                None,
            )
            .await
            .expect("repeat");
            eprintln!(
                "case=repeat input_bytes={} elapsed_ms={} facts={} entities={} warnings={} output_bytes={}",
                content.len(),
                start.elapsed().as_millis(),
                repeated.facts.len(),
                repeated.entities.len(),
                repeated.warnings.len(),
                serde_json::to_vec(&repeated).expect("serialize").len()
            );
        }
    }
    let tm = common::TestMemory::new(false).await;
    let subject = common::resolve_entity(&tm.service, "person", "Alice Smith")
        .await
        .expect("resolve subject");
    for value in 0..20 {
        common::seed_fact_with_links(
            &tm.service,
            "org",
            &format!("Alice Smith reports ARR is ${value}M."),
            "2026-10-01T00:00:00Z".parse().expect("timestamp"),
            vec![subject.clone()],
        )
        .await;
    }
    let content = "Alice Smith reports ARR is $25M.";
    let start = std::time::Instant::now();
    let result = extract(&tm.service, content).await;
    eprintln!(
        "case=genuine_existing_20 input_bytes={} elapsed_ms={} facts={} entities={} warnings={} output_bytes={}",
        content.len(),
        start.elapsed().as_millis(),
        result.facts.len(),
        result.entities.len(),
        result.warnings.len(),
        serde_json::to_vec(&result).expect("serialize").len()
    );
}

#[tokio::test]
async fn reporter_is_not_implicitly_the_subject_of_another_persons_claim() {
    let tm = common::TestMemory::new(false).await;
    let alice = common::resolve_entity(&tm.service, "person", "Alice Smith")
        .await
        .expect("resolve Alice");
    common::seed_fact_with_links(
        &tm.service,
        "org",
        "Alice Smith reports ARR is $5M.",
        "2026-10-01T00:00:00Z".parse().expect("timestamp"),
        vec![alice.clone()],
    )
    .await;
    let result = extract(
        &tm.service,
        "Alice Smith says Bob Jones reports ARR is $10M.",
    )
    .await;
    let claims = tm
        .db_client
        .query(
            "SELECT subject FROM claim WHERE source_episode_id = $ep",
            Some(serde_json::json!({"ep": result.episode_id})),
            "org",
        )
        .await
        .expect("claims");
    let rows: Vec<serde_json::Value> = serde_json::from_value(claims).expect("claim rows");
    assert!(
        rows.iter()
            .all(|row| row["subject"]["subject_key"] != alice),
        "the reporter must not become the claim subject: {rows:?}"
    );
    assert!(
        result.warnings.is_empty(),
        "reporter and subject are not interchangeable: {:?}",
        result.warnings
    );
}

#[tokio::test]
async fn repeated_reporting_marker_does_not_bind_the_first_name() {
    let tm = common::TestMemory::new(false).await;
    let result = extract(
        &tm.service,
        "Alice Smith reports Bob Jones reports ARR is $10M.",
    )
    .await;
    let rows = tm
        .db_client
        .query(
            "SELECT source_fact_id FROM claim WHERE source_episode_id = $ep",
            Some(serde_json::json!({"ep": result.episode_id})),
            "org",
        )
        .await
        .expect("claims");
    assert!(
        serde_json::from_value::<Vec<serde_json::Value>>(rows)
            .expect("rows")
            .is_empty()
    );
    assert!(!result.facts.is_empty());
}

#[tokio::test]
#[ignore = "manual genuine-conflict scale profile"]
async fn profile_genuine_conflict_scale() {
    for count in [20, 50, 100] {
        let tm = common::TestMemory::new(false).await;
        let subject = common::resolve_entity(&tm.service, "person", "Alice Smith")
            .await
            .expect("subject");
        for value in 0..count {
            common::seed_fact_with_links(
                &tm.service,
                "org",
                &format!("Alice Smith reports ARR is ${value}M."),
                "2026-10-01T00:00:00Z".parse().expect("time"),
                vec![subject.clone()],
            )
            .await;
        }
        let start = std::time::Instant::now();
        let result = extract(&tm.service, "Alice Smith reports ARR is $1000M.").await;
        let elapsed = start.elapsed();
        assert_eq!(result.warnings.len(), count);
        eprintln!(
            "case=genuine_scale existing_facts={count} elapsed_ms={} warnings={} output_bytes={}",
            elapsed.as_millis(),
            result.warnings.len(),
            serde_json::to_vec(&result).expect("serialize").len()
        );
    }
}

#[tokio::test]
async fn equivalent_currency_scales_do_not_create_a_contradiction() {
    let tm = common::TestMemory::new(false).await;
    extract(&tm.service, "Alice Smith reports ARR is $1M.").await;
    let result = extract(&tm.service, "Alice Smith reports ARR is $1000K.").await;
    assert!(
        result.warnings.is_empty(),
        "equivalent quantities must retain one canonical value"
    );
    let rows = tm
        .db_client
        .query(
            "SELECT source_fact_id FROM claim WHERE source_episode_id = $ep AND schema_family = 'quantity'",
            Some(serde_json::json!({"ep": result.episode_id})),
            "org",
        )
        .await
        .expect("claims");
    assert!(
        !serde_json::from_value::<Vec<serde_json::Value>>(rows)
            .expect("rows")
            .is_empty()
    );
}

#[cfg(feature = "prometheus")]
#[tokio::test]
#[ignore = "manual existing-stage metric attribution"]
async fn profile_existing_stage_metrics() {
    let handle = metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
        .expect("recorder");
    fn snapshot(
        handle: &metrics_exporter_prometheus::PrometheusHandle,
    ) -> std::collections::BTreeMap<String, f64> {
        handle
            .render()
            .lines()
            .filter_map(|line| {
                let (key, value) = line.rsplit_once(' ')?;
                let name = key.split('{').next()?;
                if name.starts_with("memory_")
                    && (name.ends_with("_sum") || name.ends_with("_count"))
                {
                    Some((key.to_string(), value.parse().ok()?))
                } else {
                    None
                }
            })
            .collect()
    }
    for genuine in [false, true] {
        let tm = common::TestMemory::new(false).await;
        let content = if genuine {
            let subject = common::resolve_entity(&tm.service, "person", "Alice Smith")
                .await
                .expect("subject");
            for value in 0..100 {
                common::seed_fact_with_links(
                    &tm.service,
                    "org",
                    &format!("Alice Smith reports ARR is ${value}M."),
                    "2026-10-01T00:00:00Z".parse().expect("time"),
                    vec![subject.clone()],
                )
                .await;
            }
            "Alice Smith reports ARR is $1000M.".to_string()
        } else {
            let mut content = "Facts:\n".to_string();
            for index in 0..50 {
                content.push_str(&format!(
                    "- Historical record: {{\"date\":\"2026-10-{:02}\",\"duration\":{}}}\n",
                    1 + index % 28,
                    index + 1
                ));
            }
            content
        };
        let before = snapshot(&handle);
        let result = extract(&tm.service, &content).await;
        let delta: std::collections::BTreeMap<_, _> = snapshot(&handle)
            .into_iter()
            .map(|(key, value)| {
                let delta = value - before.get(&key).copied().unwrap_or_default();
                (key, delta)
            })
            .filter(|(_, delta)| *delta != 0.0)
            .collect();
        eprintln!(
            "stage_profile genuine={genuine} warnings={} delta={}",
            result.warnings.len(),
            serde_json::to_string(&delta).expect("metrics")
        );
    }
}

#[tokio::test]
async fn url_in_a_commitment_preserves_its_canonical_subject() {
    let tm = common::TestMemory::new(false).await;
    let result = extract(
        &tm.service,
        "Alice Smith will send Bob Jones https://example.com by Friday.",
    )
    .await;
    let alice = result
        .entities
        .iter()
        .find(|entity| entity.canonical_name == "Alice Smith")
        .expect("Alice");
    let rows = tm.db_client.query("SELECT subject FROM claim WHERE source_episode_id = $ep AND schema_family = 'commitment'", Some(serde_json::json!({"ep": result.episode_id})), "org").await.expect("claims");
    let rows: Vec<serde_json::Value> = serde_json::from_value(rows).expect("rows");
    assert!(!rows.is_empty());
    assert!(
        rows.iter()
            .all(|row| row["subject"]["subject_key"] == alice.entity_id)
    );
}
