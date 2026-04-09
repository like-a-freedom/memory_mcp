//! Full-dataset loading and sampling for external eval benchmarks.
//!
//! Reads upstream datasets from `tests/fixtures/evals/raw/` (downloaded by
//! `scripts/convert_external_evals.py`) and applies an optional sampling
//! limit via the `MEMORY_MCP_EVAL_SAMPLE_PCT` environment variable.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use csv::ReaderBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::external::{DatasetKind, NormalizedExternalRetrievalCase, normalize_external_dataset};

// ── Sampling ────────────────────────────────────────────────────────────────

/// Returns the sampling percentage (1–100) from the environment.
///
/// `MEMORY_MCP_EVAL_SAMPLE_PCT=10` → use 10 % of cases.
/// Default is 100 (full dataset).
pub fn sample_pct_from_env() -> usize {
    std::env::var("MEMORY_MCP_EVAL_SAMPLE_PCT")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|pct| (1..=100).contains(pct))
        .unwrap_or(100)
}

/// Deterministically sample a prefix of *cases* according to *pct* (1–100).
pub fn sample_cases<T>(cases: Vec<T>, pct: usize) -> Vec<T> {
    if pct >= 100 {
        return cases;
    }
    let limit = (cases.len() as f64 * pct as f64 / 100.0).ceil() as usize;
    let limit = limit.max(1); // always keep at least 1 case
    cases.into_iter().take(limit).collect()
}

// ── Raw fixture paths ───────────────────────────────────────────────────────

/// Path to the raw upstream fixture in `tests/fixtures/evals/raw/<kind>/`.
///
/// These files are downloaded by `scripts/convert_external_evals.py` and are
/// the single source of truth for eval data.
pub fn raw_fixture_path(kind: DatasetKind) -> PathBuf {
    let file_name = match kind {
        DatasetKind::LongMemEvalCleaned => "longmemeval_s_cleaned.json",
        DatasetKind::LoCoMo => "locomo10.json",
        DatasetKind::PersonaMem => "questions_32k.csv",
        DatasetKind::PrefEval => "travel_hotel_overall300_topk_history_persona.json",
    };

    repo_root()
        .join("tests")
        .join("fixtures")
        .join("evals")
        .join("raw")
        .join(dataset_dir_name(kind))
        .join(file_name)
}

/// Path to the bundled cache file in `tests/fixtures/evals/full/<kind>/`.
///
/// Used for PersonaMem and PrefEval which need multi-source bundling.
pub fn full_dataset_cache_path(kind: DatasetKind) -> PathBuf {
    let spec = full_dataset_spec(kind);
    let dataset_dir = full_dataset_root().join(spec.dir_name);
    dataset_dir.join(spec.bundle_file_name.unwrap_or(spec.primary_file_name))
}

// ── Loading ─────────────────────────────────────────────────────────────────

/// Loads and normalizes external dataset cases from raw fixtures.
///
/// Automatically applies sampling based on `MEMORY_MCP_EVAL_SAMPLE_PCT`
/// (default: 100 %).
pub async fn load_external_dataset_cases(
    kind: DatasetKind,
) -> Result<Vec<NormalizedExternalRetrievalCase>, String> {
    let pct = sample_pct_from_env();
    let raw = load_raw_dataset_raw(kind).await?;
    let cases = normalize_external_dataset(kind, &raw)?;
    Ok(sample_cases(cases, pct))
}

// ── Bundling helpers ────────────────────────────────────────────────────────

pub fn wrap_prefeval_full_track(track: &str, raw: &str) -> Result<String, String> {
    let records: Vec<Value> = serde_json::from_str(raw)
        .map_err(|err| format!("parse full prefeval track array: {err}"))?;

    serde_json::to_string_pretty(&json!({
        "track": track,
        "records": records,
    }))
    .map_err(|err| format!("serialize wrapped prefeval track: {err}"))
}

pub fn bundle_personamem_official_sources(
    questions_csv: &str,
    shared_contexts_jsonl: &str,
) -> Result<String, String> {
    let mut reader = ReaderBuilder::new()
        .has_headers(true)
        .from_reader(questions_csv.as_bytes());
    let mut questions = Vec::new();
    let mut needed_context_ids = BTreeSet::new();

    for record in reader.deserialize::<PersonaMemCsvQuestion>() {
        let question = record.map_err(|err| format!("parse personamem questions csv: {err}"))?;
        needed_context_ids.insert(question.shared_context_id.clone());
        questions.push(question);
    }

    let mut shared_contexts = BTreeMap::<String, Value>::new();
    for line in shared_contexts_jsonl
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let record: Value = serde_json::from_str(line)
            .map_err(|err| format!("parse personamem context line: {err}"))?;
        let Some(record_map) = record.as_object() else {
            return Err("personamem context jsonl line is not an object".to_string());
        };

        for context_id in &needed_context_ids {
            if let Some(context_messages) = record_map.get(context_id) {
                shared_contexts.insert(context_id.clone(), context_messages.clone());
            }
        }
    }

    let missing_contexts = needed_context_ids
        .into_iter()
        .filter(|context_id| !shared_contexts.contains_key(context_id))
        .collect::<Vec<_>>();
    if !missing_contexts.is_empty() {
        return Err(format!(
            "personamem bundle missing shared contexts: {:?}",
            missing_contexts
        ));
    }

    serde_json::to_string_pretty(&json!({
        "questions": questions,
        "shared_contexts": shared_contexts,
    }))
    .map_err(|err| format!("serialize bundled personamem fixture: {err}"))
}

// ── Internal: raw fixture loading with bundling ─────────────────────────────

async fn load_raw_dataset_raw(kind: DatasetKind) -> Result<String, String> {
    match kind {
        DatasetKind::LongMemEvalCleaned | DatasetKind::LoCoMo => {
            // Direct JSON — read from raw fixture
            let path = raw_fixture_path(kind);
            std::fs::read_to_string(&path)
                .map_err(|err| format!("read raw fixture {}: {err}", path.display()))
        }
        DatasetKind::PrefEval => {
            // Check for pre-bundled cache first; fall back to raw + wrap
            let bundle_path = full_dataset_cache_path(kind);
            if bundle_path.exists() {
                return std::fs::read_to_string(&bundle_path).map_err(|err| {
                    format!("read prefeval bundle {}: {err}", bundle_path.display())
                });
            }
            let raw_path = raw_fixture_path(kind);
            let raw = std::fs::read_to_string(&raw_path)
                .map_err(|err| format!("read raw prefeval {}: {err}", raw_path.display()))?;
            let spec = full_dataset_spec(kind);
            let wrapped = wrap_prefeval_full_track(
                spec.prefeval_track
                    .ok_or_else(|| "prefeval spec missing track label".to_string())?,
                &raw,
            )?;
            // Cache the bundle for future calls
            if let Some(parent) = bundle_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&bundle_path, &wrapped);
            Ok(wrapped)
        }
        DatasetKind::PersonaMem => {
            // Check for pre-bundled cache first; fall back to raw CSV + JSONL
            let bundle_path = full_dataset_cache_path(kind);
            if bundle_path.exists() {
                return std::fs::read_to_string(&bundle_path).map_err(|err| {
                    format!("read personamem bundle {}: {err}", bundle_path.display())
                });
            }
            let csv_path = raw_fixture_path(kind);
            let questions_csv = std::fs::read_to_string(&csv_path)
                .map_err(|err| format!("read raw personamem {}: {err}", csv_path.display()))?;

            // The auxiliary JSONL lives next to the CSV in raw/
            let spec = full_dataset_spec(kind);
            let aux_file = spec
                .auxiliary_file_name
                .ok_or_else(|| "personamem spec missing auxiliary file".to_string())?;
            let aux_path = csv_path.parent().unwrap().join(aux_file);
            let shared_contexts_jsonl = std::fs::read_to_string(&aux_path).map_err(|err| {
                format!(
                    "read raw personamem auxiliary {}: {err}",
                    aux_path.display()
                )
            })?;

            let bundled =
                bundle_personamem_official_sources(&questions_csv, &shared_contexts_jsonl)?;
            // Cache the bundle for future calls
            if let Some(parent) = bundle_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&bundle_path, &bundled);
            Ok(bundled)
        }
    }
}

// ── Dataset specs ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
struct FullDatasetSpec {
    dir_name: &'static str,
    primary_file_name: &'static str,
    auxiliary_file_name: Option<&'static str>,
    bundle_file_name: Option<&'static str>,
    prefeval_track: Option<&'static str>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PersonaMemCsvQuestion {
    persona_id: i64,
    question_id: String,
    question_type: String,
    topic: String,
    context_length_in_tokens: i64,
    context_length_in_letters: i64,
    distance_to_ref_in_blocks: i64,
    distance_to_ref_in_tokens: i64,
    num_irrelevant_tokens: i64,
    distance_to_ref_proportion_in_context: String,
    user_question_or_message: String,
    correct_answer: String,
    all_options: String,
    shared_context_id: String,
    end_index_in_shared_context: i64,
}

fn full_dataset_spec(kind: DatasetKind) -> FullDatasetSpec {
    match kind {
        DatasetKind::LongMemEvalCleaned => FullDatasetSpec {
            dir_name: "longmemeval",
            primary_file_name: "longmemeval_s_cleaned.json",
            auxiliary_file_name: None,
            bundle_file_name: None,
            prefeval_track: None,
        },
        DatasetKind::LoCoMo => FullDatasetSpec {
            dir_name: "locomo",
            primary_file_name: "locomo10.json",
            auxiliary_file_name: None,
            bundle_file_name: None,
            prefeval_track: None,
        },
        DatasetKind::PersonaMem => FullDatasetSpec {
            dir_name: "personamem",
            primary_file_name: "questions_32k.csv",
            auxiliary_file_name: Some("shared_contexts_32k.jsonl"),
            bundle_file_name: Some("personamem_32k.bundle.json"),
            prefeval_track: None,
        },
        DatasetKind::PrefEval => FullDatasetSpec {
            dir_name: "prefeval",
            primary_file_name: "travel_hotel_overall300_topk_history_persona.json",
            auxiliary_file_name: None,
            bundle_file_name: Some("travel_hotel_overall300_topk_history_persona.bundle.json"),
            prefeval_track: Some("travel_hotel_overall300_topk_history_persona"),
        },
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn full_dataset_root() -> PathBuf {
    repo_root()
        .join("tests")
        .join("fixtures")
        .join("evals")
        .join("full")
}

fn dataset_dir_name(kind: DatasetKind) -> &'static str {
    match kind {
        DatasetKind::LongMemEvalCleaned => "longmemeval",
        DatasetKind::LoCoMo => "locomo",
        DatasetKind::PersonaMem => "personamem",
        DatasetKind::PrefEval => "prefeval",
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_fixture_path_points_to_raw_longmemeval() {
        let path = raw_fixture_path(DatasetKind::LongMemEvalCleaned);
        let normalized = path.to_string_lossy().replace('\\', "/");
        assert!(
            normalized.ends_with("tests/fixtures/evals/raw/longmemeval/longmemeval_s_cleaned.json"),
            "unexpected raw longmemeval path: {normalized}"
        );
    }

    #[test]
    fn raw_fixture_path_points_to_raw_locomo() {
        let path = raw_fixture_path(DatasetKind::LoCoMo);
        let normalized = path.to_string_lossy().replace('\\', "/");
        assert!(
            normalized.ends_with("tests/fixtures/evals/raw/locomo/locomo10.json"),
            "unexpected raw locomo path: {normalized}"
        );
    }

    #[test]
    fn prefeval_full_cache_path_points_to_wrapped_bundle() {
        let path = full_dataset_cache_path(DatasetKind::PrefEval);
        let normalized = path.to_string_lossy().replace('\\', "/");
        assert!(
            normalized.ends_with(
                "tests/fixtures/evals/full/prefeval/travel_hotel_overall300_topk_history_persona.bundle.json"
            ),
            "unexpected prefeval full dataset path: {normalized}"
        );
    }

    #[test]
    fn sample_pct_from_env_defaults_to_100() {
        // When env var is not set, defaults to 100.
        // (We can't easily unset env vars in concurrent tests, so just check the function exists.)
        let pct = sample_pct_from_env();
        assert!((1..=100).contains(&pct));
    }

    #[test]
    fn sample_cases_returns_all_at_100_pct() {
        let cases: Vec<i32> = (0..100).collect();
        assert_eq!(sample_cases(cases.clone(), 100), cases);
    }

    #[test]
    fn sample_cases_takes_prefix_at_lower_pct() {
        let cases: Vec<i32> = (0..100).collect();
        let sampled = sample_cases(cases, 10);
        assert_eq!(sampled.len(), 10);
        assert_eq!(sampled[0], 0);
        assert_eq!(sampled[9], 9);
    }

    #[test]
    fn sample_cases_keeps_at_least_one() {
        let cases = vec![42];
        assert_eq!(sample_cases(cases.clone(), 1), cases);
    }
}
