mod common;

use common::storage_release_gates as gates;
use serde_json::{Value, json};
use sha2::Digest;

fn comparable_pair_set(
    baseline_model: &str,
    candidate_model: &str,
    baseline_dimension: usize,
    candidate_dimension: usize,
) -> Value {
    let case = json!({
        "id": "http-remote384-retrieval-misses-ann-warm",
        "composition": "http",
        "scenario": "retrieval_misses",
        "workload": "ann",
        "phase": "steady",
        "cache_mode": "warm",
        "embedding_mode": "remote",
        "embedding_dimension": 384,
        "concurrency": 1
    });
    let identity = |model: &str, dimension: usize, git_commit: &str| {
        json!({
            "host_fingerprint": "linux-amd64-host-1",
            "os": "linux",
            "kernel": "6.12.0",
            "arch": "x86_64",
            "runner_platform": "linux/amd64",
            "emulated": false,
            "libc": "glibc-2.41",
            "ram_bytes": 3_221_225_472u64,
            "swap_bytes": 0u64,
            "cpu_quota_millis": 4000u64,
            "provider": "openai-compatible",
            "model": model,
            "model_revision": "614241f622f53c4eeff9890bdc4f31cfecc418b3",
            "git_commit": git_commit,
            "provider_config_hash": "pinned-provider-config",
            "embedding_mode": "remote",
            "embedding_dimension": dimension,
            "cache_mode": "warm",
            "runtime_base_image_digest": "sha256:pinned-runtime-image",
        })
    };
    let measurement = |revision: &str, latency_ms: f64, request_count: usize| {
        json!({
            "revision": revision,
            "warmup_count": 30,
            "request_samples_ms": vec![latency_ms; request_count],
            "successful_requests": request_count,
            "error_count": 0,
            "wall_time_ms": request_count as f64 * latency_ms,
            "app_cpu_ms": request_count as f64 * 8.0,
            "db_cpu_ms": request_count as f64 * 3.0,
            "ann_queries": request_count,
            "embedding_provider_requests": request_count,
            "case": case.clone(),
        })
    };
    let aa_pairs = (0..5)
        .map(|index| {
            json!({
                "baseline": measurement("source-parent", 10.0, 1000),
                "candidate": measurement("source-parent", 10.0, 1000),
                "block_id": format!("aa-{index}"),
            })
        })
        .collect::<Vec<_>>();
    let ab_pairs = ["AB", "BA", "AB", "BA", "AB"]
        .into_iter()
        .enumerate()
        .map(|(index, order)| {
            json!({
                "baseline": measurement("source-parent", 10.0, 1000),
                "candidate": measurement("candidate", 10.0, 1000),
                "block_id": format!("ab-{index}"),
                "order": order,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "schema_version": "memory-mcp-storage-performance/v1",
        "baseline": identity(baseline_model, baseline_dimension, "source-parent"),
        "candidate": identity(candidate_model, candidate_dimension, "candidate"),
        "case": case,
        "expected_case_ids": ["http-remote384-retrieval-misses-ann-warm"],
        "executed_case_ids": ["http-remote384-retrieval-misses-ann-warm"],
        "aa_pairs": aa_pairs,
        "ab_pairs": ab_pairs,
    })
}

fn disabled_pair_set(workload: &str, mode: &str, dimension: usize) -> Value {
    let mut pair_set = comparable_pair_set("disabled", "disabled", dimension, dimension);
    let case_id = format!("http-{mode}{dimension}-retrieval-misses-warm");
    let case = json!({
        "id": case_id.clone(),
        "composition": "http",
        "scenario": "retrieval_misses",
        "workload": workload,
        "phase": "steady",
        "cache_mode": "warm",
        "embedding_mode": mode,
        "embedding_dimension": dimension,
        "concurrency": 1
    });
    pair_set["case"] = case.clone();
    pair_set["expected_case_ids"] = json!([case_id.clone()]);
    pair_set["executed_case_ids"] = json!([case_id]);
    for side in ["baseline", "candidate"] {
        pair_set[side]["provider"] = json!("disabled");
        pair_set[side]["model"] = json!("disabled");
        pair_set[side]["model_revision"] = json!("disabled");
        pair_set[side]["embedding_mode"] = json!(mode);
    }
    for pair_key in ["aa_pairs", "ab_pairs"] {
        for pair in pair_set[pair_key]
            .as_array_mut()
            .expect("literal paired measurements")
        {
            for side in ["baseline", "candidate"] {
                pair[side]["case"] = case.clone();
                pair[side]["ann_queries"] = json!(0);
                pair[side]["embedding_provider_requests"] = json!(0);
            }
        }
    }
    pair_set
}

fn write_pair_set(directory: &std::path::Path, value: &Value) {
    std::fs::write(
        directory.join("pair-set.json"),
        serde_json::to_vec(value).expect("serialize pair set"),
    )
    .expect("write pair set");
}

fn comparable_action_pair_set(scenario: &str, fact_count: u64) -> Value {
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    let composition = match scenario {
        "startup" => "stdio",
        "backfill" => "http",
        _ => "cli",
    };
    let case_id = format!("{composition}-remote384-{scenario}-action");
    let action_case = json!({
        "id": case_id,
        "composition": composition,
        "scenario": scenario,
        "workload": scenario,
        "phase": "action",
        "cache_mode": "warm",
        "embedding_mode": "remote",
        "embedding_dimension": 384,
        "concurrency": 1
    });
    pair_set["case"] = action_case.clone();
    pair_set["expected_case_ids"] = json!([case_id]);
    pair_set["executed_case_ids"] = json!([case_id]);

    for pair_key in ["aa_pairs", "ab_pairs"] {
        for pair in pair_set[pair_key]
            .as_array_mut()
            .expect("literal paired measurements")
        {
            for side in ["baseline", "candidate"] {
                pair[side] = json!({
                "revision": if side == "baseline" || pair_key == "aa_pairs" { "source-parent" } else { "candidate" },
                        "case": action_case.clone(),
                        "action": {
                            "kind": scenario,
                            "completed": true,
                            "durable_state_verified": true,
                            "elapsed_ms": 1000.0,
                            "requested_facts": fact_count,
                            "completed_facts": fact_count,
                            "persisted_facts": fact_count,
                            "failed_facts": 0
                        },
                        "app_cpu_ms": 800.0,
                        "db_cpu_ms": 300.0,
                        "ann_queries": 0,
                        "embedding_provider_requests": fact_count
                    });
            }
        }
    }
    pair_set
}

fn release_profile_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals/profiles/release.json")
}

fn retrieval_quality_corpus_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/corpora/retrieval-quality-v1/retrieval-quality.json")
}

fn release_quality_artifact(git_commit: &str) -> Value {
    let profile_path = release_profile_path();
    let profile_bytes = std::fs::read(&profile_path).expect("pinned release profile");
    let profile: Value = serde_json::from_slice(&profile_bytes).expect("release profile JSON");
    let corpus_path = retrieval_quality_corpus_path();
    let corpus_bytes = std::fs::read(&corpus_path).expect("pinned retrieval-quality corpus");
    let corpus: Value = serde_json::from_slice(&corpus_bytes).expect("retrieval corpus JSON");
    let cases = corpus["cases"].as_array().expect("corpus cases");
    let metric_names = profile["gates"]
        .as_array()
        .expect("release gates")
        .iter()
        .filter(|gate| gate["target"]["suite_id"] == "retrieval-quality")
        .map(|gate| gate["target"]["metric"].as_str().expect("metric name"))
        .collect::<Vec<_>>();
    let mut case_keys = cases
        .iter()
        .map(|case| json!({"suite_id":"retrieval-quality", "case_id":case["id"]}))
        .collect::<Vec<_>>();
    let mut outcomes = cases
        .iter()
        .map(|case| {
            let mut metrics = serde_json::Map::new();
            for metric in &metric_names {
                metrics.insert((*metric).into(), json!(1.0));
            }
            if case["required_evidence_groups"]
                .as_array()
                .is_some_and(|groups| !groups.is_empty())
            {
                metrics.insert("required_group_recall".into(), json!(1.0));
            }
            json!({
                "case_key": {"suite_id":"retrieval-quality", "case_id":case["id"]},
                "mode": "end_to_end",
                "split": case["split"],
                "label_trust": case["label_trust"],
                "status": "passed",
                "metrics": metrics,
                "evidence": {},
                "failures": [],
                "duration_ms": 1,
                "attempts": 1,
            })
        })
        .collect::<Vec<_>>();
    let summary_metrics = metric_names
        .iter()
        .map(|metric| ((*metric).to_string(), json!(1.0)))
        .collect::<serde_json::Map<_, _>>();
    let mut suite_summaries = vec![json!({
        "suite_id": "retrieval-quality",
        "mode": "end_to_end",
        "total": cases.len(),
        "passed": cases.len(),
        "quality_failed": 0,
        "invalid": 0,
        "metrics": summary_metrics,
    })];
    let native = &corpus["native_preparation"];
    let model = native["model"].clone();
    let provider = native["provider_label"].clone();
    let dimension = native["embedding_dimension"].as_u64().expect("dimension");
    let mut suite_fingerprint = json!({
        "corpus_id": corpus["corpus_id"],
        "corpus_revision": corpus["revision"],
        "corpus_manifest_sha256": hex::encode(sha2::Sha256::digest(&corpus_bytes)),
        "source_bundle_sha256": corpus["source_sha256"],
        "prepared_provider": provider,
        "prepared_model": model,
        "prepared_embedding_signature": native["embedding_signature"],
        "prepared_embedding_dimension": dimension.to_string(),
        "prepared_similarity_threshold": native["similarity_threshold"].to_string(),
        "prepared_index_signature": native["index_signature"],
        "prepared_extractor_fingerprint": native["extractor_fingerprint"],
        "prepared_claim_rollout_stage": native["claim_rollout_stage"],
        "provider": native["provider_label"],
        "provider_status": "ready",
        "model": native["model"],
        "embedding_dimension": dimension.to_string(),
        "embedding_signature": native["embedding_signature"],
        "index_signature": native["index_signature"],
        "extractor_fingerprint": native["extractor_fingerprint"],
        "claim_rollout_stage": native["claim_rollout_stage"],
    });
    if let Some(signature) = native["reranking_signature"].as_str() {
        suite_fingerprint["reranking_signature"] = json!(signature);
    }
    let profile_digest = hex::encode(sha2::Sha256::digest(&profile_bytes));
    let mut suite_fingerprints = json!({ "retrieval-quality": suite_fingerprint });
    for suite in profile["suites"].as_array().expect("release suites") {
        let suite_id = suite["id"].as_str().expect("suite ID");
        if suite_id == "retrieval-quality" {
            continue;
        }
        let exact_cases = suite["expected_coverage"]["exact_cases"]
            .as_u64()
            .expect("suite coverage") as usize;
        let case_ids = (0..exact_cases)
            .map(|index| format!("{suite_id}-fixture-{index:03}"))
            .collect::<Vec<_>>();
        let case_metrics = profile["gates"]
            .as_array()
            .expect("release gates")
            .iter()
            .filter(|gate| gate["target"]["suite_id"] == suite_id)
            .map(|gate| {
                (
                    gate["target"]["metric"]
                        .as_str()
                        .expect("gate metric")
                        .to_owned(),
                    json!(1.0),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        let case_split = json!("test");
        let case_label_trust = json!("reviewed");
        case_keys.extend(case_ids.iter().map(|case_id| {
            json!({
                "suite_id": suite_id,
                "case_id": case_id
            })
        }));
        outcomes.extend(case_ids.iter().map(|case_id| {
            json!({
                "case_key": {"suite_id":suite_id, "case_id":case_id},
                "mode": "end_to_end",
                "split": case_split,
                "label_trust": case_label_trust,
                "status": "passed",
                "metrics": case_metrics,
                "evidence": {},
                "failures": [],
                "duration_ms": 1,
                "attempts": 1,
            })
        }));
        suite_summaries.push(json!({
            "suite_id": suite_id,
            "mode": "end_to_end",
            "total": exact_cases,
            "passed": exact_cases,
            "quality_failed": 0,
            "invalid": 0,
            "metrics": case_metrics,
        }));
        suite_fingerprints[suite_id] = json!({
            "suite_id": suite_id,
            "profile_digest": profile_digest,
        });
    }
    json!({
        "schema_version": "memory-mcp-eval/v2",
        "run_id": "storage-quality-fixture",
        "profile": "release",
        "started_at": "2026-10-13T00:00:00Z",
        "duration_ms": 1,
        "expected_case_ids": [],
        "expected_cases": case_keys,
        "outcomes": outcomes,
        "suite_summaries": suite_summaries,
        "gates": [],
        "fingerprint": {
            "rust_version": "rustc 1.99.0",
            "os_arch": "linux/x86_64",
            "package_version": env!("CARGO_PKG_VERSION"),
            "build_profile": "release",
            "enabled_features": ["fs-watch", "mcp-apps", "streamable-http", "eval-support"],
            "provider": provider,
            "model": model,
            "device": "cpu",
            "configuration_hash": "pinned-equivalent-configuration",
            "git_commit": git_commit,
            "evaluator_versions": {"eval-harness": env!("CARGO_PKG_VERSION")},
            "suite_fingerprints": suite_fingerprints,
            "profile_digest": profile_digest,
        },
        "budget_status": "passed",
        "verdict": "passed",
        "issues": [],
    })
}

fn write_quality_artifact(
    directory: &std::path::Path,
    name: &str,
    artifact: &Value,
) -> std::path::PathBuf {
    let path = directory.join(name);
    std::fs::write(
        &path,
        serde_json::to_vec(artifact).expect("serialize quality artifact"),
    )
    .expect("write quality artifact");
    path
}

#[test]
fn missing_baseline_is_invalid() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    std::fs::write(
        directory.path().join("pair-set.json"),
        r#"{"schema_version":"memory-mcp-storage-performance/v1","baseline":null,"candidate":{"revision":"candidate"}}"#,
    )
    .expect("literal pair-set fixture");

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["missing_baseline".into()]),
        "missing baseline evidence must fail closed with its named cause"
    );
}

#[test]
fn wrong_model_is_invalid() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    write_pair_set(
        directory.path(),
        &comparable_pair_set("intfloat/multilingual-e5-small", "other/model", 384, 384),
    );

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["model_mismatch".into()]),
        "different embedding models cannot form a causal A/B comparison"
    );
}

#[test]
fn wrong_dimension_is_invalid() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    write_pair_set(
        directory.path(),
        &comparable_pair_set(
            "intfloat/multilingual-e5-small",
            "intfloat/multilingual-e5-small",
            384,
            768,
        ),
    );

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["embedding_dimension_mismatch".into()]),
        "different resolved dimensions cannot form a causal A/B comparison"
    );
}

#[test]
fn action_pair_set_needs_completion_and_durable_facts_without_request_samples() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    write_pair_set(
        directory.path(),
        &comparable_action_pair_set("reembed", 205),
    );

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Passed,
        "an action comparison uses completed durable action evidence without synthetic request samples"
    );
}

#[test]
fn action_pair_set_accepts_complete_backfill_without_request_samples() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    write_pair_set(
        directory.path(),
        &comparable_action_pair_set("backfill", 205),
    );

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(verdict, gates::ReleaseGateVerdict::Passed);
}

#[test]
fn startup_action_pair_set_accepts_durable_readiness_without_fake_requests() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    write_pair_set(directory.path(), &comparable_action_pair_set("startup", 0));

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(verdict, gates::ReleaseGateVerdict::Passed);
}

#[test]
fn action_pair_set_rejects_missing_durable_verification() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_action_pair_set("reembed", 205);
    pair_set["ab_pairs"][0]["candidate"]["action"]["durable_state_verified"] = json!(false);
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert!(
        matches!(verdict, gates::ReleaseGateVerdict::Invalid(_)),
        "an action without read-after-write durability evidence cannot pass"
    );
}

#[test]
fn action_pair_set_rejects_unpersisted_facts() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_action_pair_set("reembed", 205);
    pair_set["ab_pairs"][0]["candidate"]["action"]["persisted_facts"] = json!(204);
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert!(
        matches!(verdict, gates::ReleaseGateVerdict::Invalid(_)),
        "reported action completion must match durable fact count"
    );
}

#[test]
fn action_pair_set_rejects_different_fact_work_between_sides() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_action_pair_set("reembed", 205);
    pair_set["ab_pairs"][0]["candidate"]["action"]["requested_facts"] = json!(206);
    pair_set["ab_pairs"][0]["candidate"]["action"]["completed_facts"] = json!(206);
    pair_set["ab_pairs"][0]["candidate"]["action"]["persisted_facts"] = json!(206);
    pair_set["ab_pairs"][0]["candidate"]["embedding_provider_requests"] = json!(206);
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["ab_action_work_mismatch".into()]),
        "a faster run that processed a different number of facts is not comparable"
    );
}

#[test]
fn different_host_is_invalid() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    pair_set["candidate"]["host_fingerprint"] = json!("linux-amd64-host-2");
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["host_mismatch".into()]),
        "paired runs on different physical hosts cannot support causal comparison"
    );
}

#[test]
fn different_cache_mode_is_invalid() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    pair_set["candidate"]["cache_mode"] = json!("cold");
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["cache_mode_mismatch".into()]),
        "warm and cold cache runs cannot be combined into one paired comparison"
    );
}

#[test]
fn different_provider_is_invalid() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    pair_set["candidate"]["provider"] = json!("local-candle");
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["provider_mismatch".into()]),
        "different embedding providers cannot form a causal A/B comparison"
    );
}

#[test]
fn different_model_revision_is_invalid() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    pair_set["candidate"]["model_revision"] = json!("different-immutable-model-revision");
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["model_revision_mismatch".into()]),
        "two different model revisions cannot form a causal A/B comparison"
    );
}

#[test]
fn different_memory_budget_is_invalid() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    pair_set["candidate"]["ram_bytes"] = json!(2_147_483_648u64);
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["ram_budget_mismatch".into()]),
        "runs under different RAM limits cannot form a causal A/B comparison"
    );
}

#[test]
fn increased_error_rate_is_rejected_without_dropping_failed_attempts() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    for pair in pair_set["ab_pairs"]
        .as_array_mut()
        .expect("literal A/B pairs")
    {
        pair["candidate"]["successful_requests"] = json!(999);
        pair["candidate"]["error_count"] = json!(1);
        pair["candidate"]["wall_time_ms"] = json!(9990.0);
        pair["candidate"]["app_cpu_ms"] = json!(7992.0);
    }
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Regressed(vec!["error_rate_regression".into()]),
        "failed requests stay in the denominator and a higher error rate rejects the run"
    );
}

#[test]
fn paired_run_order_must_be_balanced_and_pinned() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    pair_set["ab_pairs"][0]["order"] = json!("BA");
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["invalid_ab_order_schedule".into()]),
        "the release gate requires the preregistered AB/BA/AB/BA/AB sequence"
    );
}

#[test]
fn insufficient_pairs_is_invalid() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    pair_set["ab_pairs"]
        .as_array_mut()
        .expect("literal A/B pairs")
        .truncate(4);
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["insufficient_ab_pairs".into()]),
        "the release comparison requires five complete paired blocks"
    );
}

#[test]
fn insufficient_requests_is_invalid() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    pair_set["ab_pairs"][0]["baseline"]["request_samples_ms"]
        .as_array_mut()
        .expect("literal request samples")
        .truncate(999);
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["insufficient_request_samples".into()]),
        "each measured side must contain at least one thousand raw requests"
    );
}

#[test]
fn more_than_one_thousand_request_samples_is_invalid() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    let run = &mut pair_set["ab_pairs"][0]["baseline"];
    run["request_samples_ms"]
        .as_array_mut()
        .expect("literal request samples")
        .push(json!(10.0));
    run["successful_requests"] = json!(1001);
    run["wall_time_ms"] = json!(10010.0);
    run["app_cpu_ms"] = json!(8008.0);
    run["db_cpu_ms"] = json!(3003.0);
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["request_sample_count_mismatch".into()]),
        "the steady-state protocol is exactly one thousand measured requests"
    );
}

#[test]
fn more_than_thirty_warmup_requests_is_invalid() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    pair_set["ab_pairs"][0]["baseline"]["warmup_count"] = json!(31);
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["warmup_count_mismatch".into()]),
        "the steady-state protocol is exactly thirty warmup requests"
    );
}

#[test]
fn latency_point_regression_is_rejected() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    for pair in pair_set["ab_pairs"]
        .as_array_mut()
        .expect("literal A/B pairs")
    {
        let mut samples = vec![10.0; 949];
        samples.extend(vec![10.2; 51]);
        pair["candidate"]["request_samples_ms"] = json!(samples);
    }
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Regressed(vec!["latency_p95_ratio".into()]),
        "a paired latency ratio above one must fail the point-estimate gate"
    );
}

#[test]
fn latency_p50_point_regression_is_rejected() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    for pair in pair_set["ab_pairs"]
        .as_array_mut()
        .expect("literal A/B pairs")
    {
        let mut samples = vec![10.0; 499];
        samples.extend(vec![10.2; 501]);
        pair["candidate"]["request_samples_ms"] = json!(samples);
    }
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Regressed(vec!["latency_p50_ratio".into()]),
        "p50 latency is a required independent point-estimate gate"
    );
}

#[test]
fn latency_p99_point_regression_is_rejected_even_when_p95_is_unchanged() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    for pair in pair_set["ab_pairs"]
        .as_array_mut()
        .expect("literal A/B pairs")
    {
        let mut samples = vec![10.0; 1000];
        samples[980..].fill(20.0);
        pair["candidate"]["request_samples_ms"] = json!(samples);
    }
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Regressed(vec!["latency_p99_ratio".into()]),
        "p99 latency is a required independent point-estimate gate"
    );
}

#[test]
fn paired_latency_point_estimate_uses_median_not_mean() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    for (pair, latency_ms) in pair_set["ab_pairs"]
        .as_array_mut()
        .expect("literal A/B pairs")
        .iter_mut()
        .zip([9.9, 9.9, 9.9, 9.9, 11.0])
    {
        pair["candidate"]["request_samples_ms"] = json!(vec![latency_ms; 1000]);
    }
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec![
            "latency_p50_uncertainty_wide".into(),
            "latency_p95_uncertainty_wide".into(),
            "latency_p99_uncertainty_wide".into()
        ]),
        "one outlier pair widens uncertainty but must not turn the median point estimate into a regression"
    );
}

#[test]
fn database_cpu_point_regression_is_rejected_separately_from_app_cpu() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    for pair in pair_set["ab_pairs"]
        .as_array_mut()
        .expect("literal A/B pairs")
    {
        pair["candidate"]["db_cpu_ms"] = json!(3030.0);
    }
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Regressed(vec!["db_cpu_ms_per_attempt_ratio".into()]),
        "database CPU must not be treated as free when storage work shifts out of the app"
    );
}

#[test]
fn missing_database_cpu_evidence_is_invalid() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    pair_set["aa_pairs"][0]["baseline"]["db_cpu_ms"] = Value::Null;
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["invalid_aa_measurement".into()]),
        "missing database CPU counters cannot be treated as zero-cost work"
    );
}

#[test]
fn equal_nonzero_baseline_and_candidate_error_rates_are_invalid() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    for pair in pair_set["ab_pairs"]
        .as_array_mut()
        .expect("literal A/B pairs")
    {
        for side in ["baseline", "candidate"] {
            pair[side]["successful_requests"] = json!(999);
            pair[side]["error_count"] = json!(1);
        }
    }
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["baseline_error_rate_nonzero".into()]),
        "a nonzero source error rate cannot establish a zero-error performance baseline"
    );
}

#[test]
fn throughput_point_regression_is_rejected() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    for pair in pair_set["ab_pairs"]
        .as_array_mut()
        .expect("literal A/B pairs")
    {
        pair["candidate"]["wall_time_ms"] = json!(10_101.010101);
    }
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Regressed(vec!["throughput_ratio".into()]),
        "a paired throughput ratio below one must fail the point-estimate gate"
    );
}

#[test]
fn cpu_point_regression_is_rejected() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    for pair in pair_set["ab_pairs"]
        .as_array_mut()
        .expect("literal A/B pairs")
    {
        pair["candidate"]["app_cpu_ms"] = json!(8080.0);
    }
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Regressed(vec!["app_cpu_ms_per_attempt_ratio".into()]),
        "a paired application CPU ratio above one must fail the point-estimate gate"
    );
}

#[test]
fn wide_uncertainty_is_inconclusive() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    let candidate_latencies = [9.0, 11.0, 10.0, 10.0, 10.0];
    for (pair, latency_ms) in pair_set["ab_pairs"]
        .as_array_mut()
        .expect("literal A/B pairs")
        .iter_mut()
        .zip(candidate_latencies)
    {
        pair["candidate"]["request_samples_ms"] = json!(vec![latency_ms; 1000]);
    }
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec![
            "latency_p50_uncertainty_wide".into(),
            "latency_p95_uncertainty_wide".into(),
            "latency_p99_uncertainty_wide".into()
        ]),
        "a point estimate at parity is inconclusive when its block bootstrap interval is wide"
    );
}

#[test]
fn a_a_noise_above_three_percent_is_invalid() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    pair_set["aa_pairs"][0]["candidate"]["request_samples_ms"] = json!(vec![10.4; 1000]);
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["aa_noise_over_3_percent".into()]),
        "A/A calibration noise above three percent invalidates the comparison"
    );
}

#[test]
fn perfect_quality_cannot_hide_missing_cases() {
    let directory = tempfile::tempdir().expect("quality artifact directory");
    let baseline = release_quality_artifact(&"a".repeat(40));
    let mut candidate = release_quality_artifact(&"b".repeat(40));
    candidate["outcomes"]
        .as_array_mut()
        .expect("candidate quality outcomes")
        .pop();
    let baseline_path = write_quality_artifact(directory.path(), "baseline.json", &baseline);
    let candidate_path = write_quality_artifact(directory.path(), "candidate.json", &candidate);

    let verdict =
        gates::compare_quality_artifacts(&baseline_path, &candidate_path, &release_profile_path())
            .expect("readable evaluation artifacts produce a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["missing_candidate_case_coverage".into()]),
        "perfect aggregate values must not hide a missing candidate case"
    );
}

#[test]
fn quality_comparison_requires_coverage_for_every_release_profile_suite() {
    let directory = tempfile::tempdir().expect("quality artifact directory");
    let baseline = release_quality_artifact(&"a".repeat(40));
    let mut candidate = release_quality_artifact(&"b".repeat(40));
    candidate["suite_summaries"]
        .as_array_mut()
        .expect("suite summaries")
        .retain(|summary| summary["suite_id"] != "extraction");
    let baseline_path = write_quality_artifact(directory.path(), "baseline.json", &baseline);
    let candidate_path = write_quality_artifact(directory.path(), "candidate.json", &candidate);

    let verdict =
        gates::compare_quality_artifacts(&baseline_path, &candidate_path, &release_profile_path())
            .expect("readable evaluation artifacts produce a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec![
            "candidate_suite_coverage_mismatch:extraction".into()
        ]),
        "retrieval-quality coverage cannot hide a missing release profile suite"
    );
}

#[test]
fn identical_quality_metrics_pass_for_comparable_complete_artifacts() {
    let directory = tempfile::tempdir().expect("quality artifact directory");
    let baseline = release_quality_artifact(&"a".repeat(40));
    let candidate = release_quality_artifact(&"b".repeat(40));
    let baseline_path = write_quality_artifact(directory.path(), "baseline.json", &baseline);
    let candidate_path = write_quality_artifact(directory.path(), "candidate.json", &candidate);

    let verdict =
        gates::compare_quality_artifacts(&baseline_path, &candidate_path, &release_profile_path())
            .expect("readable evaluation artifacts produce a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Passed,
        "a complete, matching native release comparison with no regression passes"
    );
}

#[test]
fn non_retrieval_quality_regression_within_floor_and_budget_is_rejected() {
    let directory = tempfile::tempdir().expect("quality artifact directory");
    let mut baseline = release_quality_artifact(&"a".repeat(40));
    let mut candidate = release_quality_artifact(&"b".repeat(40));
    for (artifact, value) in [(&mut baseline, 0.9), (&mut candidate, 0.85)] {
        let summary = artifact["suite_summaries"]
            .as_array_mut()
            .expect("suite summaries")
            .iter_mut()
            .find(|summary| summary["suite_id"] == "extraction")
            .expect("extraction summary");
        summary["metrics"]["entity_f1"] = json!(value);
        for outcome in artifact["outcomes"]
            .as_array_mut()
            .expect("case outcomes")
            .iter_mut()
            .filter(|outcome| outcome["case_key"]["suite_id"] == "extraction")
        {
            outcome["metrics"]["entity_f1"] = json!(value);
        }
    }
    let baseline_path = write_quality_artifact(directory.path(), "baseline.json", &baseline);
    let candidate_path = write_quality_artifact(directory.path(), "candidate.json", &candidate);

    let verdict =
        gates::compare_quality_artifacts(&baseline_path, &candidate_path, &release_profile_path())
            .expect("readable evaluation artifacts produce a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Regressed(vec![
            "quality_metric_regression:extraction:entity_f1".into()
        ]),
        "zero-regression quality comparison must include profile suites beyond retrieval-quality"
    );
}

#[test]
fn one_lost_required_fact_rejects_quality() {
    let directory = tempfile::tempdir().expect("quality artifact directory");
    let baseline = release_quality_artifact(&"a".repeat(40));
    let mut candidate = release_quality_artifact(&"b".repeat(40));
    let required_outcome = candidate["outcomes"]
        .as_array_mut()
        .expect("candidate quality outcomes")
        .iter_mut()
        .find(|outcome| outcome["metrics"].get("required_group_recall").is_some())
        .expect("corpus has at least one required-evidence case");
    let required_case_id = required_outcome["case_key"]["case_id"]
        .as_str()
        .expect("required-evidence case ID")
        .to_owned();
    required_outcome["metrics"]["required_group_recall"] = json!(0.0);
    let baseline_path = write_quality_artifact(directory.path(), "baseline.json", &baseline);
    let candidate_path = write_quality_artifact(directory.path(), "candidate.json", &candidate);

    let verdict =
        gates::compare_quality_artifacts(&baseline_path, &candidate_path, &release_profile_path())
            .expect("readable evaluation artifacts produce a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Regressed(vec![format!(
            "required_fact_lost:{required_case_id}"
        )]),
        "losing a required-evidence group in one case must reject the candidate"
    );
}

#[test]
fn quality_floor_pass_does_not_allow_baseline_regression() {
    let directory = tempfile::tempdir().expect("quality artifact directory");
    let mut profile: Value = serde_json::from_slice(
        &std::fs::read(release_profile_path()).expect("pinned release profile"),
    )
    .expect("release profile JSON");
    profile["suites"]
        .as_array_mut()
        .expect("release suites")
        .iter_mut()
        .find(|suite| suite["id"] == "retrieval-quality")
        .expect("retrieval-quality suite")["corpus_root"] = json!(
        retrieval_quality_corpus_path()
            .parent()
            .expect("corpus directory")
            .to_string_lossy()
    );
    profile["gates"]
        .as_array_mut()
        .expect("release gates")
        .iter_mut()
        .find(|gate| {
            gate["target"]["suite_id"] == "retrieval-quality"
                && gate["target"]["metric"] == "precision_macro"
        })
        .expect("precision release gate")["hard_floor"] = json!(0.9);
    let profile_bytes = serde_json::to_vec(&profile).expect("serialize modified profile");
    let profile_path = directory.path().join("release.json");
    std::fs::write(&profile_path, &profile_bytes).expect("write modified release profile");

    let profile_digest = hex::encode(sha2::Sha256::digest(&profile_bytes));
    let mut baseline = release_quality_artifact(&"a".repeat(40));
    let mut candidate = release_quality_artifact(&"b".repeat(40));
    baseline["fingerprint"]["profile_digest"] = json!(&profile_digest);
    candidate["fingerprint"]["profile_digest"] = json!(&profile_digest);
    baseline["suite_summaries"][0]["metrics"]["precision_macro"] = json!(0.95);
    candidate["suite_summaries"][0]["metrics"]["precision_macro"] = json!(0.94);
    let baseline_path = write_quality_artifact(directory.path(), "baseline.json", &baseline);
    let candidate_path = write_quality_artifact(directory.path(), "candidate.json", &candidate);

    let verdict = gates::compare_quality_artifacts(&baseline_path, &candidate_path, &profile_path)
        .expect("readable evaluation artifacts produce a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Regressed(vec![
            "quality_metric_regression:precision_macro".into()
        ]),
        "meeting a relaxed absolute floor does not permit regression from the source parent"
    );
}

#[test]
fn complete_comparable_performance_pair_set_passes() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    write_pair_set(
        directory.path(),
        &comparable_pair_set(
            "intfloat/multilingual-e5-small",
            "intfloat/multilingual-e5-small",
            384,
            384,
        ),
    );

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Passed,
        "a complete preregistered A/A and A/B set with identical results should pass"
    );
}

#[test]
fn zero_success_baseline_is_invalid() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    for pair in pair_set["ab_pairs"]
        .as_array_mut()
        .expect("literal A/B pairs")
    {
        pair["baseline"]["successful_requests"] = json!(0);
        pair["baseline"]["error_count"] = json!(1000);
    }
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["no_successful_requests".into()]),
        "a zero-throughput baseline cannot yield an infinite candidate improvement"
    );
}

#[test]
fn non_linux_host_is_invalid() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    pair_set["baseline"]["os"] = json!("darwin");
    pair_set["candidate"]["os"] = json!("darwin");
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["native_linux_amd64_required".into()]),
        "matching x86_64 labels do not prove measurements ran on native Linux"
    );
}

#[test]
fn extra_performance_pairs_are_invalid() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    let extra_pair = pair_set["ab_pairs"][0].clone();
    pair_set["ab_pairs"]
        .as_array_mut()
        .expect("literal A/B pairs")
        .push(extra_pair);
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["ab_pair_count_mismatch".into()]),
        "unregistered extra blocks cannot silently change the preregistered sample size"
    );
}

#[test]
fn duplicate_performance_pair_ids_are_invalid() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    pair_set["ab_pairs"][1]["block_id"] = pair_set["ab_pairs"][0]["block_id"].clone();
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["duplicate_performance_block_id".into()]),
        "each measured pair must identify a distinct captured block"
    );
}

#[test]
fn measured_revision_must_match_declared_candidate_commit() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set(
        "intfloat/multilingual-e5-small",
        "intfloat/multilingual-e5-small",
        384,
        384,
    );
    pair_set["ab_pairs"][0]["candidate"]["revision"] = json!("unrelated-build");
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["candidate_run_revision_mismatch".into()]),
        "a run must be attributable to the candidate commit declared by its immutable manifest"
    );
}

#[test]
fn baseline_quality_outcomes_must_all_pass() {
    let directory = tempfile::tempdir().expect("quality artifact directory");
    let mut baseline = release_quality_artifact(&"a".repeat(40));
    let candidate = release_quality_artifact(&"b".repeat(40));
    baseline["outcomes"][0]["status"] = json!("quality_failed");
    baseline["verdict"] = json!("quality_failed");
    let baseline_path = write_quality_artifact(directory.path(), "baseline.json", &baseline);
    let candidate_path = write_quality_artifact(directory.path(), "candidate.json", &candidate);

    let verdict =
        gates::compare_quality_artifacts(&baseline_path, &candidate_path, &release_profile_path())
            .expect("readable evaluation artifacts produce a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["baseline_case_not_passed".into()]),
        "a passing summary cannot make a release artifact with a failed case a valid baseline"
    );
}

#[test]
fn failed_release_budget_cannot_be_hidden_by_passing_case_summary() {
    let directory = tempfile::tempdir().expect("quality artifact directory");
    let baseline = release_quality_artifact(&"a".repeat(40));
    let mut candidate = release_quality_artifact(&"b".repeat(40));
    candidate["budget_status"] = json!("failed");
    candidate["verdict"] = json!("quality_failed");
    let baseline_path = write_quality_artifact(directory.path(), "baseline.json", &baseline);
    let candidate_path = write_quality_artifact(directory.path(), "candidate.json", &candidate);

    let verdict =
        gates::compare_quality_artifacts(&baseline_path, &candidate_path, &release_profile_path())
            .expect("readable evaluation artifacts produce a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["candidate_run_not_passed".into()]),
        "a failed release budget must invalidate the candidate even when case summaries look perfect"
    );
}

#[test]
fn corpus_case_split_and_label_trust_must_match_quality_outcomes() {
    let directory = tempfile::tempdir().expect("quality artifact directory");
    let baseline = release_quality_artifact(&"a".repeat(40));
    let mut candidate = release_quality_artifact(&"b".repeat(40));
    let corpus: Value = serde_json::from_slice(
        &std::fs::read(retrieval_quality_corpus_path()).expect("retrieval-quality corpus"),
    )
    .expect("retrieval-quality corpus JSON");
    let first_id = candidate["outcomes"][0]["case_key"]["case_id"]
        .as_str()
        .expect("case ID")
        .to_owned();
    let source_case = corpus["cases"]
        .as_array()
        .expect("corpus cases")
        .iter()
        .find(|case| case["id"] == first_id)
        .expect("case exists in the pinned corpus");
    candidate["outcomes"][0]["split"] = json!(if source_case["split"] == "test" {
        "development"
    } else {
        "test"
    });
    let baseline_path = write_quality_artifact(directory.path(), "baseline.json", &baseline);
    let candidate_path = write_quality_artifact(directory.path(), "candidate.json", &candidate);

    let verdict =
        gates::compare_quality_artifacts(&baseline_path, &candidate_path, &release_profile_path())
            .expect("readable evaluation artifacts produce a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec![format!(
            "candidate_case_metadata_mismatch:{first_id}"
        )]),
        "case metadata must be anchored to the current pinned corpus rather than artifact claims"
    );
}

#[test]
#[ignore = "requires immutable native Linux amd64 performance A/A and A/B artifacts"]
fn performance_pair_set_meets_release_contract() {
    let pair_set = std::env::var_os("STORAGE_RELEASE_PAIR_SET")
        .map(std::path::PathBuf::from)
        .expect("STORAGE_RELEASE_PAIR_SET must point to the captured pair-set directory");

    let verdict = gates::compare_performance_pair_set(&pair_set)
        .expect("captured performance evidence should be readable");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Passed,
        "native release performance evidence must show no latency, throughput, CPU, or error regression"
    );
}

#[test]
fn different_measurement_case_is_invalid() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set("model", "model", 384, 384);
    pair_set["case"] = json!({
        "id": "http-remote384-retrieval-misses-ann-warm",
        "composition": "http",
        "scenario": "retrieval_misses",
        "workload": "ann",
        "phase": "steady",
        "cache_mode": "warm",
        "embedding_mode": "remote",
        "embedding_dimension": 384,
        "concurrency": 1
    });
    pair_set["ab_pairs"][0]["candidate"]["case"] = json!({
        "id": "http-remote768-retrieval-misses-ann-warm",
        "composition": "http",
        "scenario": "retrieval_misses",
        "workload": "ann",
        "phase": "steady",
        "cache_mode": "warm",
        "embedding_mode": "remote",
        "embedding_dimension": 768,
        "concurrency": 1
    });
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["performance_case_mismatch".into()]),
        "every measured block must match its declared mode, workload and phase"
    );
}

#[test]
fn performance_case_dimension_must_match_the_paired_embedding_mode() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set("model", "model", 384, 384);
    pair_set["case"] = json!({
        "id": "http-remote768-retrieval-misses-ann-warm",
        "composition": "http",
        "scenario": "retrieval_misses",
        "workload": "ann",
        "phase": "steady",
        "cache_mode": "warm",
        "embedding_mode": "remote",
        "embedding_dimension": 768,
        "concurrency": 1
    });
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["performance_case_mismatch".into()]),
        "the benchmark case width must equal both baseline and candidate dimensions"
    );
}

#[test]
fn performance_case_inventory_must_be_complete() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = comparable_pair_set("model", "model", 384, 384);
    pair_set["case"] = json!({
        "id": "http-remote384-retrieval-misses-ann-warm",
        "composition": "http",
        "scenario": "retrieval_misses",
        "workload": "ann",
        "phase": "steady",
        "cache_mode": "warm",
        "embedding_mode": "remote",
        "embedding_dimension": 384,
        "concurrency": 1
    });
    pair_set["expected_case_ids"] = json!(["http-remote384-retrieval-misses-ann-warm"]);
    pair_set["executed_case_ids"] = json!([]);
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["performance_case_inventory_mismatch".into()]),
        "a performance pair set must report the case it actually measured"
    );
}

#[test]
fn disabled_embedding_mode_passes_with_nonsemantic_retrieval() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    write_pair_set(
        directory.path(),
        &disabled_pair_set("lexical_fallback", "disabled_empty", 384),
    );

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Passed,
        "disabled embedding mode must retain a comparable nonsemantic workload"
    );
}

#[test]
fn disabled_stored_embedding_mode_passes_without_semantic_reads() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    write_pair_set(
        directory.path(),
        &disabled_pair_set("lexical_fallback", "disabled_stored", 3072),
    );

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Passed,
        "legacy stored vectors must not prevent a comparable nonsemantic disabled run"
    );
}

#[test]
fn zero_configured_dimension_is_invalid_even_when_embeddings_are_disabled() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    write_pair_set(
        directory.path(),
        &disabled_pair_set("lexical_fallback", "disabled_empty", 0),
    );

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["invalid_embedding_dimension".into()]),
        "disabled mode still requires a valid positive schema dimension"
    );
}

#[test]
fn disabled_embedding_mode_rejects_an_ann_workload() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    write_pair_set(
        directory.path(),
        &disabled_pair_set("ann", "disabled_empty", 384),
    );

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["disabled_mode_has_ann_workload".into()]),
        "a disabled embedding profile cannot claim semantic ANN work"
    );
}

#[test]
fn disabled_embedding_mode_rejects_observed_embedding_activity() {
    let directory = tempfile::tempdir().expect("pair-set directory");
    let mut pair_set = disabled_pair_set("lexical_fallback", "disabled_empty", 384);
    pair_set["ab_pairs"][0]["candidate"]["ann_queries"] = json!(1);
    pair_set["ab_pairs"][0]["candidate"]["embedding_provider_requests"] = json!(1);
    write_pair_set(directory.path(), &pair_set);

    let verdict = gates::compare_performance_pair_set(directory.path())
        .expect("a readable pair set produces a gate verdict");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Invalid(vec!["disabled_mode_has_embedding_activity".into()]),
        "disabled mode must make zero embedding-provider and ANN calls during measurement"
    );
}

#[test]
#[ignore = "requires immutable parent and candidate native release evaluation artifacts"]
fn native_quality_has_zero_regression() {
    let baseline = std::env::var_os("STORAGE_RELEASE_BASELINE_QUALITY")
        .map(std::path::PathBuf::from)
        .expect("STORAGE_RELEASE_BASELINE_QUALITY must point to the immutable parent artifact");
    let candidate = std::env::var_os("STORAGE_RELEASE_CANDIDATE_QUALITY")
        .map(std::path::PathBuf::from)
        .expect("STORAGE_RELEASE_CANDIDATE_QUALITY must point to the immutable candidate artifact");

    let verdict = gates::compare_quality_artifacts(&baseline, &candidate, &release_profile_path())
        .expect("captured native quality evidence should be readable");

    assert_eq!(
        verdict,
        gates::ReleaseGateVerdict::Passed,
        "native release quality evidence must retain every pinned case and quality metric"
    );
}
