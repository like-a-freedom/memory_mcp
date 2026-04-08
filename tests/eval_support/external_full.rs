#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use csv::ReaderBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::external::{DatasetKind, NormalizedExternalRetrievalCase, normalize_external_dataset};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalDatasetFlavor {
    Sample,
    Full,
}

#[derive(Debug, Clone, Copy)]
struct FullDatasetSpec {
    dir_name: &'static str,
    primary_file_name: &'static str,
    primary_url: &'static str,
    auxiliary_file_name: Option<&'static str>,
    auxiliary_url: Option<&'static str>,
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

pub fn sample_fixture_path(kind: DatasetKind) -> PathBuf {
    let file_name = match kind {
        DatasetKind::LongMemEvalCleaned => "sample_longmemeval_s_cleaned.json",
        DatasetKind::LoCoMo => "sample_locomo10.json",
        DatasetKind::PersonaMem => "sample_personamem_32k.json",
        DatasetKind::PrefEval => "sample_travel_hotel_implicit_persona.json",
    };

    repo_root()
        .join("tests")
        .join("fixtures")
        .join("evals")
        .join("raw")
        .join(dataset_dir_name(kind))
        .join(file_name)
}

pub fn full_dataset_cache_path(kind: DatasetKind) -> PathBuf {
    let spec = full_dataset_spec(kind);
    let dataset_dir = full_dataset_root().join(spec.dir_name);

    dataset_dir.join(spec.bundle_file_name.unwrap_or(spec.primary_file_name))
}

pub async fn load_external_dataset_cases(
    kind: DatasetKind,
    flavor: ExternalDatasetFlavor,
) -> Result<Vec<NormalizedExternalRetrievalCase>, String> {
    let raw = match flavor {
        ExternalDatasetFlavor::Sample => std::fs::read_to_string(sample_fixture_path(kind))
            .map_err(|err| format!("read sample fixture for {:?}: {err}", kind))?,
        ExternalDatasetFlavor::Full => load_full_dataset_raw(kind).await?,
    };

    normalize_external_dataset(kind, &raw)
}

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

async fn load_full_dataset_raw(kind: DatasetKind) -> Result<String, String> {
    let cache_path = ensure_full_dataset_cached(kind).await?;
    std::fs::read_to_string(&cache_path)
        .map_err(|err| format!("read full dataset cache {}: {err}", cache_path.display()))
}

async fn ensure_full_dataset_cached(kind: DatasetKind) -> Result<PathBuf, String> {
    let spec = full_dataset_spec(kind);
    let dataset_dir = full_dataset_root().join(spec.dir_name);
    std::fs::create_dir_all(&dataset_dir)
        .map_err(|err| format!("create full dataset dir {}: {err}", dataset_dir.display()))?;

    let primary_path = dataset_dir.join(spec.primary_file_name);
    ensure_text_download(spec.primary_url, &primary_path).await?;

    let auxiliary_path = match (spec.auxiliary_file_name, spec.auxiliary_url) {
        (Some(file_name), Some(url)) => {
            let path = dataset_dir.join(file_name);
            ensure_text_download(url, &path).await?;
            Some(path)
        }
        _ => None,
    };

    match kind {
        DatasetKind::LongMemEvalCleaned | DatasetKind::LoCoMo => Ok(primary_path),
        DatasetKind::PrefEval => {
            let bundle_path = dataset_dir.join(
                spec.bundle_file_name
                    .ok_or_else(|| "prefeval full dataset spec missing bundle file".to_string())?,
            );
            if !bundle_path.exists() {
                let raw = std::fs::read_to_string(&primary_path).map_err(|err| {
                    format!(
                        "read prefeval primary dataset {}: {err}",
                        primary_path.display()
                    )
                })?;
                let wrapped = wrap_prefeval_full_track(
                    spec.prefeval_track.ok_or_else(|| {
                        "prefeval full dataset spec missing track label".to_string()
                    })?,
                    &raw,
                )?;
                std::fs::write(&bundle_path, wrapped).map_err(|err| {
                    format!("write prefeval bundle {}: {err}", bundle_path.display())
                })?;
            }
            Ok(bundle_path)
        }
        DatasetKind::PersonaMem => {
            let bundle_path =
                dataset_dir.join(spec.bundle_file_name.ok_or_else(|| {
                    "personamem full dataset spec missing bundle file".to_string()
                })?);
            if !bundle_path.exists() {
                let questions_csv = std::fs::read_to_string(&primary_path).map_err(|err| {
                    format!(
                        "read personamem questions {}: {err}",
                        primary_path.display()
                    )
                })?;
                let auxiliary_path = auxiliary_path.ok_or_else(|| {
                    "personamem full dataset spec missing auxiliary path".to_string()
                })?;
                let shared_contexts_jsonl =
                    std::fs::read_to_string(&auxiliary_path).map_err(|err| {
                        format!(
                            "read personamem shared contexts {}: {err}",
                            auxiliary_path.display()
                        )
                    })?;
                let bundled =
                    bundle_personamem_official_sources(&questions_csv, &shared_contexts_jsonl)?;
                std::fs::write(&bundle_path, bundled).map_err(|err| {
                    format!("write personamem bundle {}: {err}", bundle_path.display())
                })?;
            }
            Ok(bundle_path)
        }
    }
}

async fn ensure_text_download(url: &str, path: &PathBuf) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }

    let response = reqwest::get(url)
        .await
        .map_err(|err| format!("fetch {url}: {err}"))?
        .error_for_status()
        .map_err(|err| format!("fetch {url}: {err}"))?;
    let body = response
        .text()
        .await
        .map_err(|err| format!("read {url}: {err}"))?;

    std::fs::write(path, body)
        .map_err(|err| format!("write cached dataset {}: {err}", path.display()))
}

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

fn full_dataset_spec(kind: DatasetKind) -> FullDatasetSpec {
    match kind {
        DatasetKind::LongMemEvalCleaned => FullDatasetSpec {
            dir_name: "longmemeval",
            primary_file_name: "longmemeval_s_cleaned.json",
            primary_url: "https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/main/longmemeval_s_cleaned.json",
            auxiliary_file_name: None,
            auxiliary_url: None,
            bundle_file_name: None,
            prefeval_track: None,
        },
        DatasetKind::LoCoMo => FullDatasetSpec {
            dir_name: "locomo",
            primary_file_name: "locomo10.json",
            primary_url: "https://raw.githubusercontent.com/snap-research/locomo/main/data/locomo10.json",
            auxiliary_file_name: None,
            auxiliary_url: None,
            bundle_file_name: None,
            prefeval_track: None,
        },
        DatasetKind::PersonaMem => FullDatasetSpec {
            dir_name: "personamem",
            primary_file_name: "questions_32k.csv",
            primary_url: "https://huggingface.co/datasets/bowen-upenn/PersonaMem/resolve/main/questions_32k.csv",
            auxiliary_file_name: Some("shared_contexts_32k.jsonl"),
            auxiliary_url: Some(
                "https://huggingface.co/datasets/bowen-upenn/PersonaMem/resolve/main/shared_contexts_32k.jsonl",
            ),
            bundle_file_name: Some("personamem_32k.bundle.json"),
            prefeval_track: None,
        },
        DatasetKind::PrefEval => FullDatasetSpec {
            dir_name: "prefeval",
            primary_file_name: "travel_hotel_overall300_topk_history_persona.json",
            primary_url: "https://raw.githubusercontent.com/amazon-science/PrefEval/main/benchmark_dataset/rag_retrieval/simcse_implicit_persona/travel_hotel_overall300_topk_history_persona.json",
            auxiliary_file_name: None,
            auxiliary_url: None,
            bundle_file_name: Some("travel_hotel_overall300_topk_history_persona.bundle.json"),
            prefeval_track: Some("travel_hotel_overall300_topk_history_persona"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_fixture_path_points_to_existing_trimmed_fixture() {
        let path = sample_fixture_path(DatasetKind::LoCoMo);

        assert!(
            path.exists(),
            "expected sample fixture path to exist: {}",
            path.display()
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
}
