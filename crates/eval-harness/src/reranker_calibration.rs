//! Threshold fitting on independently reviewed development query/passage pairs.
use crate::error::EvalError;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct CalibrationPair {
    pub id: String,
    pub query: String,
    pub document: String,
    pub relevant: bool,
}

#[derive(Debug, Deserialize)]
pub struct CalibrationDataset {
    pub schema_version: String,
    pub split: String,
    pub source_sha256: String,
    pub label_trust: String,
    pub pairs: Vec<CalibrationPair>,
}

#[derive(Debug, Serialize)]
pub struct OperatingPoint {
    pub floor: f64,
    pub true_positive: usize,
    pub false_positive: usize,
    pub false_negative: usize,
    pub true_negative: usize,
    pub precision: f64,
    pub recall: f64,
    pub f1: f64,
}

fn evaluate_floor(scores: &[(f64, bool)], floor: f64) -> Result<OperatingPoint, EvalError> {
    if !floor.is_finite()
        || scores.iter().any(|(score, _)| !score.is_finite())
        || !scores.iter().any(|(_, relevant)| *relevant)
        || !scores.iter().any(|(_, relevant)| !relevant)
    {
        return Err(EvalError::InvalidInput(
            "operating point requires finite scores, a finite floor and both labels".into(),
        ));
    }
    let (mut tp, mut fp, mut fn_, mut tn) = (0, 0, 0, 0);
    for (score, relevant) in scores {
        match (*score >= floor, *relevant) {
            (true, true) => tp += 1,
            (true, false) => fp += 1,
            (false, true) => fn_ += 1,
            (false, false) => tn += 1,
        }
    }
    let precision = if tp + fp == 0 {
        0.0
    } else {
        tp as f64 / (tp + fp) as f64
    };
    let recall = tp as f64 / (tp + fn_) as f64;
    let f1 = 2.0 * tp as f64 / (2 * tp + fp + fn_) as f64;
    Ok(OperatingPoint {
        floor,
        true_positive: tp,
        false_positive: fp,
        false_negative: fn_,
        true_negative: tn,
        precision,
        recall,
        f1,
    })
}

pub fn fit_threshold(scores: &[(f64, bool)]) -> Result<OperatingPoint, EvalError> {
    if scores.iter().any(|(s, _)| !s.is_finite())
        || !scores.iter().any(|(_, label)| *label)
        || !scores.iter().any(|(_, label)| !label)
    {
        return Err(EvalError::InvalidInput(
            "calibration requires finite scores and both labels".into(),
        ));
    }
    let mut boundaries = scores.iter().map(|(s, _)| *s).collect::<Vec<_>>();
    boundaries.sort_by(f64::total_cmp);
    boundaries.dedup();
    let mut best: Option<OperatingPoint> = None;
    let mut floors = Vec::with_capacity(boundaries.len());
    floors.push(boundaries[0]);
    for interval in boundaries.windows(2) {
        // Half first to avoid overflow for finite scores of either sign.
        let middle = interval[0] / 2.0 + interval[1] / 2.0;
        floors.push(if middle > interval[0] && middle <= interval[1] {
            middle
        } else {
            interval[1]
        });
    }
    for floor in floors {
        let point = evaluate_floor(scores, floor)?;
        if best.as_ref().is_none_or(|old| {
            point
                .f1
                .total_cmp(&old.f1)
                .then_with(|| point.precision.total_cmp(&old.precision))
                .then_with(|| point.recall.total_cmp(&old.recall))
                .then_with(|| point.floor.total_cmp(&old.floor))
                .is_gt()
        }) {
            best = Some(point);
        }
    }
    best.ok_or_else(|| EvalError::InvalidInput("no calibration boundary".into()))
}

fn validate(dataset: &CalibrationDataset) -> Result<(), EvalError> {
    if dataset.schema_version != "memory-reranker-calibration/v1"
        || dataset.split != "development"
        || dataset.label_trust != "reviewed"
        || dataset.source_sha256.len() != 64
        || !dataset.source_sha256.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(EvalError::InvalidInput(
            "calibration requires reviewed development pairs and a source SHA-256".into(),
        ));
    }
    let mut ids = BTreeSet::new();
    if dataset.pairs.is_empty()
        || dataset.pairs.iter().any(|p| {
            p.id.trim().is_empty()
                || p.query.trim().is_empty()
                || p.document.trim().is_empty()
                || !ids.insert(&p.id)
        })
    {
        return Err(EvalError::InvalidInput(
            "empty or duplicate calibration pair".into(),
        ));
    }
    if !dataset.pairs.iter().any(|p| p.relevant) || !dataset.pairs.iter().any(|p| !p.relevant) {
        return Err(EvalError::InvalidInput(
            "calibration requires both relevant and irrelevant pairs".into(),
        ));
    }
    Ok(())
}

pub async fn calibrate_native(path: &Path) -> Result<serde_json::Value, EvalError> {
    let bytes = std::fs::read(path).map_err(|source| EvalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let dataset: CalibrationDataset = serde_json::from_slice(&bytes)
        .map_err(|e| EvalError::InvalidInput(format!("invalid calibration dataset: {e}")))?;
    validate(&dataset)?;
    let embedding = memory_mcp::config::EmbeddingConfig::from_env()
        .map_err(|_| EvalError::InvalidConfig("invalid embedding configuration".into()))?;
    let db = memory_mcp::config::SurrealConfig::from_env()
        .map_err(|_| EvalError::InvalidConfig("invalid provider data directory".into()))?;
    let resolved = memory_mcp::eval_support::retrieval::create_eval_embedding_provider(
        &embedding,
        &db.data_dir_or_default(),
    )
    .await
    .map_err(|_| EvalError::InvalidConfig("real provider initialization failed".into()))?;
    let signature = resolved
        .provider
        .reranking_signature()
        .ok_or_else(|| {
            EvalError::InvalidConfig("calibration requires a configured real reranker".into())
        })?
        .to_owned();
    let configured_floor = resolved.provider.reranking_admission_floor();
    let mut scored = Vec::new();
    let mut observations = Vec::new();
    for pair in dataset.pairs {
        let values = resolved
            .provider
            .rerank(&pair.query, &[pair.document])
            .await
            .map_err(|_| EvalError::Suite("native reranking inference failed".into()))?
            .ok_or_else(|| EvalError::Suite("native reranker returned no evidence".into()))?;
        if values.len() != 1 || !values[0].is_finite() {
            return Err(EvalError::Suite(
                "invalid calibration score cardinality or finite value".into(),
            ));
        }
        scored.push((values[0], pair.relevant));
        observations.push(
            serde_json::json!({"id":pair.id,"relevant":pair.relevant,"relevance_logit":values[0]}),
        );
    }
    let operating_point = fit_threshold(&scored)?;
    let configured_operating_point = evaluate_floor(&scored, configured_floor)?;
    Ok(
        serde_json::json!({"schema_version":"memory-reranker-calibration-result/v1",
        "dataset_sha256":hex::encode(sha2::Sha256::digest(&bytes)),
        "source_sha256":dataset.source_sha256,"reranking_signature":signature,
        "selection_rule":"maximum development F1 on interior score-gap midpoints; ties by precision, recall, then higher floor",
        "operating_point":operating_point,"configured_operating_point":configured_operating_point,
        "observations":observations}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn learns_a_nonzero_separating_boundary_from_development_scores() {
        let result =
            fit_threshold(&[(2.0, true), (3.0, true), (1.0, false), (-2.0, false)]).expect("fit");
        assert_eq!(result.floor, 1.5);
        assert_eq!(result.f1, 1.0);
        assert_eq!(result.false_positive, 0);
        assert_eq!(result.false_negative, 0);
    }
    #[test]
    fn negative_logits_can_be_relevant_on_independent_data() {
        let result = fit_threshold(&[(-3.0, true), (-2.0, true), (-4.0, false)]).expect("fit");
        assert_eq!(result.floor, -3.5);
        assert_eq!(result.recall, 1.0);
    }
    #[test]
    fn separating_boundary_has_an_interior_margin_for_repeat_inference() {
        let selected = fit_threshold(&[(-2.9, true), (-1.0, true), (-4.0, false)]).expect("fit");
        assert!(
            selected.floor > -4.0 && selected.floor < -2.9,
            "do not pin an operational boundary to a single floating-point positive: {}",
            selected.floor
        );
        assert!(
            -2.9001 >= selected.floor,
            "small replay drift must preserve independent positive admission"
        );
        assert!(
            -3.9999 < selected.floor,
            "small replay drift must preserve independent negative refusal"
        );
    }

    #[test]
    fn configured_floor_reports_inclusive_admission_counts() {
        let result = evaluate_floor(&[(0.0, true), (0.0, false), (-1.0, false)], 0.0)
            .expect("finite configured floor");
        assert_eq!(result.true_positive, 1);
        assert_eq!(result.false_positive, 1);
        assert_eq!(result.false_negative, 0);
        assert_eq!(result.true_negative, 1);
        assert_eq!(result.precision, 0.5);
        assert_eq!(result.recall, 1.0);
    }

    #[test]
    fn nonfinite_configured_floor_fails_closed() {
        assert!(evaluate_floor(&[(1.0, true), (-1.0, false)], f64::NAN).is_err());
    }

    #[test]
    fn nonfinite_scores_fail_closed() {
        assert!(fit_threshold(&[(f64::NAN, true), (0.0, false)]).is_err());
    }
}
