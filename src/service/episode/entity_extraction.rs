use std::collections::{HashMap, HashSet};
use std::time::Instant;

use serde_json::{Value, json};

use crate::logging::LogLevel;
use crate::models::{EntityCandidate, ExtractedEntity};
use crate::service::MemoryService;
use crate::service::error::MemoryError;
use crate::service::normalize_text;
use crate::service::{log_args_with_duration, log_event};

fn ner_provider_uses_blocking_pool(provider: &str) -> bool {
    matches!(provider, "anno" | "gliner")
}

/// Extract entities from content.
///
/// # Arguments
///
/// * `service` - The memory service containing the entity extractor.
/// * `content` - The text content to extract entities from.
/// * `zero_shot_labels` - Optional custom entity labels for GLiNER extraction.
///   When provided, these labels override the default NER configuration.
pub async fn extract_entities(
    service: &MemoryService,
    content: &str,
    zero_shot_labels: Option<&[String]>,
) -> Result<Vec<ExtractedEntity>, MemoryError> {
    let timer = Instant::now();
    let provider = service.entity_extractor.provider_name();
    let content_chars = content.chars().count();

    let extraction_result = if ner_provider_uses_blocking_pool(provider) {
        let extractor = service.entity_extractor.clone();
        let content_owned = content.to_string();
        let zero_shot_labels = zero_shot_labels.map(<[String]>::to_vec);
        let handle = tokio::runtime::Handle::current();

        tokio::task::spawn_blocking(move || {
            handle.block_on(async move {
                match zero_shot_labels {
                    Some(labels) => {
                        extractor
                            .extract_candidates_with_labels(&content_owned, &labels)
                            .await
                    }
                    None => extractor.extract_candidates(&content_owned).await,
                }
            })
        })
        .await
        .map_err(|err| MemoryError::Storage(format!("entity extraction task panicked: {err}")))?
    } else {
        match zero_shot_labels {
            Some(labels) => {
                service
                    .entity_extractor
                    .extract_candidates_with_labels(content, labels)
                    .await
            }
            None => service.entity_extractor.extract_candidates(content).await,
        }
    };

    let candidates = match extraction_result {
        Ok(candidates) => candidates,
        Err(err) => {
            let label_count = zero_shot_labels.map(|labels| labels.len());
            log_ner_error(service, provider, content_chars, label_count, &err, timer);
            return Err(err);
        }
    };

    service.logger.log(
        log_event(
            "ner.extract.done",
            log_args_with_duration(json!({"content_chars": content_chars}), timer.elapsed()),
            build_ner_log_result(
                provider,
                candidates.len(),
                zero_shot_labels.map(|labels| labels.len()),
                None,
            ),
            None,
            None,
            None,
        ),
        LogLevel::Info,
    );

    let candidates = dedupe_entity_candidates(candidates);
    let mut entities = Vec::with_capacity(candidates.len());
    let mut seen_entity_ids = HashSet::new();

    for candidate in candidates {
        let entity_type = candidate.entity_type.clone();
        let canonical_name = candidate.canonical_name.clone();

        let entity_id = service
            .resolve(candidate.clone(), None)
            .await
            .inspect_err(|err| {
                service.logger.log(
                    log_event(
                        "ner.resolve.error",
                        json!({
                            "entity_type": &entity_type,
                            "canonical_name": &canonical_name,
                            "error": err.to_string(),
                        }),
                        json!({"provider": provider}),
                        None,
                        None,
                        None,
                    ),
                    LogLevel::Warn,
                );
            })?;

        if seen_entity_ids.insert(entity_id.clone()) {
            entities.push(ExtractedEntity {
                entity_id,
                entity_type,
                canonical_name,
            });
        }
    }

    Ok(entities)
}

pub(super) fn dedupe_entity_candidates(candidates: Vec<EntityCandidate>) -> Vec<EntityCandidate> {
    #[derive(Debug, Default)]
    struct CandidateGroup {
        canonical_name: String,
        type_counts: HashMap<String, usize>,
        type_first_seen: HashMap<String, usize>,
        type_display_names: HashMap<String, String>,
        aliases: HashMap<String, String>,
    }

    let mut order = Vec::new();
    let mut groups = HashMap::<String, CandidateGroup>::new();

    for (index, candidate) in candidates.into_iter().enumerate() {
        let canonical_name = candidate.canonical_name.trim();
        let entity_type = candidate.entity_type.trim();
        if canonical_name.is_empty() || entity_type.is_empty() {
            continue;
        }

        let name_key = normalize_text(canonical_name);
        if name_key.is_empty() {
            continue;
        }

        let group = groups.entry(name_key.clone()).or_insert_with(|| {
            order.push(name_key.clone());
            CandidateGroup {
                canonical_name: canonical_name.to_string(),
                ..CandidateGroup::default()
            }
        });

        if canonical_name.len() > group.canonical_name.len() {
            group.canonical_name = canonical_name.to_string();
        }

        let entity_type_key = normalize_text(entity_type);
        *group
            .type_counts
            .entry(entity_type_key.clone())
            .or_default() += 1;
        group
            .type_first_seen
            .entry(entity_type_key.clone())
            .or_insert(index);
        group
            .type_display_names
            .entry(entity_type_key)
            .or_insert_with(|| entity_type.to_string());

        for alias in candidate.aliases {
            let alias = alias.trim();
            if alias.is_empty() {
                continue;
            }

            let alias_key = normalize_text(alias);
            if alias_key.is_empty() || alias_key == name_key {
                continue;
            }

            group
                .aliases
                .entry(alias_key)
                .or_insert_with(|| alias.to_string());
        }
    }

    let mut deduped = Vec::with_capacity(order.len());

    for key in order {
        let Some(mut group) = groups.remove(&key) else {
            continue;
        };

        let mut entity_types = group
            .type_counts
            .into_iter()
            .collect::<Vec<(String, usize)>>();
        entity_types.sort_by(|(left_key, left_count), (right_key, right_count)| {
            right_count
                .cmp(left_count)
                .then_with(|| {
                    group
                        .type_first_seen
                        .get(left_key)
                        .copied()
                        .unwrap_or(usize::MAX)
                        .cmp(
                            &group
                                .type_first_seen
                                .get(right_key)
                                .copied()
                                .unwrap_or(usize::MAX),
                        )
                })
                .then_with(|| left_key.cmp(right_key))
        });

        let Some((entity_type_key, _)) = entity_types.into_iter().next() else {
            continue;
        };

        let entity_type = group
            .type_display_names
            .remove(&entity_type_key)
            .unwrap_or(entity_type_key);

        let mut aliases = group.aliases.into_values().collect::<Vec<_>>();
        aliases.sort();

        deduped.push(EntityCandidate {
            entity_type,
            canonical_name: group.canonical_name,
            aliases,
        });
    }

    deduped
}

pub(super) fn build_ner_log_result(
    provider: &str,
    entity_count: usize,
    zero_shot_label_count: Option<usize>,
    error: Option<&str>,
) -> Value {
    let mut result = serde_json::Map::new();
    result.insert("provider".to_string(), json!(provider));
    result.insert("entity_count".to_string(), json!(entity_count));
    if let Some(zero_shot_label_count) = zero_shot_label_count {
        result.insert(
            "zero_shot_label_count".to_string(),
            json!(zero_shot_label_count),
        );
    }
    if let Some(error) = error {
        result.insert("error".to_string(), json!(error));
    }
    Value::Object(result)
}

fn log_ner_error(
    service: &MemoryService,
    provider: &str,
    content_chars: usize,
    zero_shot_label_count: Option<usize>,
    err: &MemoryError,
    timer: Instant,
) {
    service.logger.log(
        log_event(
            "ner.extract.error",
            log_args_with_duration(json!({"content_chars": content_chars}), timer.elapsed()),
            build_ner_log_result(provider, 0, zero_shot_label_count, Some(&err.to_string())),
            None,
            None,
            None,
        ),
        LogLevel::Warn,
    );
}
