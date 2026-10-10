//! Optional independent query/passage relevance evidence, applied before budget
//! selection and again to synthesized/episode items before caching the result.

use super::{RetrievalContext, types::RankedContextFact};
use crate::{error::MemoryError, models::AssembledContextItem};

pub(super) fn candidate_pool_limit(budget: usize, reranking_enabled: bool) -> usize {
    let scaled = budget.max(1).saturating_mul(4);
    if reranking_enabled {
        scaled.max(50)
    } else {
        scaled
    }
}

async fn scores(
    ctx: &RetrievalContext,
    query: &str,
    documents: &[String],
) -> Result<Option<Vec<f64>>, MemoryError> {
    if query.trim().is_empty() || documents.is_empty() {
        return Ok(None);
    }
    let provider = &ctx.embedding_provider;
    let scores = ctx.embedding_service.rerank(query, documents).await?;
    if let Some(ref values) = scores {
        if provider.reranking_signature().is_none()
            || !provider.reranking_admission_floor().is_finite()
            || values.len() != documents.len()
            || values.iter().any(|value| !value.is_finite())
        {
            return Err(MemoryError::Validation(
                "invalid reranking evidence identity, cardinality or finite score".into(),
            ));
        }
    } else if provider.reranking_signature().is_some() {
        return Err(MemoryError::Validation(
            "configured reranker returned no evidence".into(),
        ));
    }
    Ok(scores)
}

fn evidence(ctx: &RetrievalContext, score: f64) -> serde_json::Value {
    serde_json::json!({"model_signature": ctx.embedding_provider.reranking_signature(),
        "relevance_logit": score, "admission_floor": ctx.embedding_provider.reranking_admission_floor()})
}

fn document(
    content: &str,
    date: Option<chrono::DateTime<chrono::Utc>>,
    source: Option<&str>,
) -> String {
    let mut context = String::new();
    if let Some(date) = date {
        context.push_str(&format!(
            "Memory reference date: {} ({}).\n",
            date.format("%Y-%m-%d"),
            date.format("%d %B %Y")
        ));
    }
    if let Some(source) = source {
        context.push_str(&format!("Source identity: {source}\n"));
    }
    context.push_str(content);
    context
}

pub(super) async fn filter_candidates(
    ctx: &RetrievalContext,
    query: &str,
    candidates: Vec<RankedContextFact>,
) -> Result<Vec<RankedContextFact>, MemoryError> {
    let docs = candidates
        .iter()
        .map(|item| {
            document(
                &item.fact.content,
                Some(item.fact.t_valid),
                item.fact.provenance.source_id.as_deref().or(item
                    .fact
                    .provenance
                    .source_url
                    .as_deref()),
            )
        })
        .collect::<Vec<_>>();
    let Some(values) = scores(ctx, query, &docs).await? else {
        return Ok(candidates);
    };
    let mut kept = candidates
        .into_iter()
        .zip(values)
        .filter_map(|(mut item, score)| {
            let exact_reference_admission =
                item.evidence.admission_basis == Some(super::types::AdmissionBasis::Reference);
            let explicit_date_admitted = item.evidence.explicit_date_admitted;
            if score < ctx.embedding_provider.reranking_admission_floor()
                && !exact_reference_admission
                && !explicit_date_admitted
            {
                return None;
            }
            item.evidence.reranking = Some(evidence(ctx, score));
            Some((score, item))
        })
        .collect::<Vec<_>>();
    kept.sort_by(|(a, left), (b, right)| {
        b.total_cmp(a)
            .then_with(|| left.fact.fact_id.cmp(&right.fact.fact_id))
    });
    Ok(kept.into_iter().map(|(_, item)| item).collect())
}

pub(super) async fn filter_items(
    ctx: &RetrievalContext,
    query: &str,
    items: Vec<AssembledContextItem>,
    budget: usize,
    preserve_input_order: bool,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    if ctx.embedding_provider.reranking_signature().is_none() || query.trim().is_empty() {
        return Ok(items.into_iter().take(budget).collect());
    }
    let indexed = items.into_iter().enumerate().collect::<Vec<_>>();
    let (already, pending): (Vec<_>, Vec<_>) = indexed.into_iter().partition(|(_, item)| {
        item.provenance
            .get("reranking")
            .and_then(|e| e.get("model_signature"))
            .and_then(serde_json::Value::as_str)
            == ctx.embedding_provider.reranking_signature()
    });
    let mut scored = Vec::with_capacity(already.len() + pending.len());
    for (input_position, item) in already {
        let score = item
            .provenance
            .get("reranking")
            .and_then(|evidence| evidence.get("relevance_logit"))
            .and_then(serde_json::Value::as_f64)
            .filter(|score| score.is_finite())
            .ok_or_else(|| {
                MemoryError::Validation(
                    "cached reranking evidence has no finite relevance logit".into(),
                )
            })?;
        scored.push((score, input_position, item));
    }
    let docs = pending
        .iter()
        .map(|(_, item)| {
            document(
                &item.content,
                item.provenance
                    .get("reference_time")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                    .map(|date| date.with_timezone(&chrono::Utc)),
                item.provenance
                    .get("source_id")
                    .and_then(serde_json::Value::as_str),
            )
        })
        .collect::<Vec<_>>();
    if let Some(values) = scores(ctx, query, &docs).await? {
        for ((input_position, mut item), score) in pending.into_iter().zip(values) {
            let exact_reference_admission = item
                .provenance
                .get("retrieval_admission")
                .and_then(|admission| admission.get("basis"))
                .and_then(serde_json::Value::as_str)
                == Some("reference");
            if score < ctx.embedding_provider.reranking_admission_floor()
                && !exact_reference_admission
            {
                continue;
            }
            if !item.provenance.is_object() {
                item.provenance = serde_json::json!({});
            }
            item.provenance["reranking"] = evidence(ctx, score);
            scored.push((score, input_position, item));
        }
    }
    if preserve_input_order {
        scored.sort_by_key(|(_, input_position, _)| *input_position);
    } else {
        scored.sort_by(|(left_score, _, left_item), (right_score, _, right_item)| {
            right_score
                .total_cmp(left_score)
                .then_with(|| left_item.fact_id.cmp(&right_item.fact_id))
        });
    }
    Ok(scored
        .into_iter()
        .take(budget)
        .map(|(_, _, item)| item)
        .collect())
}
