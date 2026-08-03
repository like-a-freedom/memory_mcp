//! Episode operations - extraction and record parsing.

mod communities;
mod edges;
mod entity_extraction;
mod fact_extraction;
mod record_parsing;
mod statement_detection;
mod summary_parser;
pub(crate) mod triples;

pub(crate) use communities::build_community_summary;
pub(crate) use edges::store_edge;
pub(crate) use fact_extraction::{build_extract_log_result, extract_from_episode};
pub(crate) use record_parsing::fact_from_value_or_wrapper;
pub(crate) use record_parsing::unwrap_record_string;
pub use record_parsing::{episode_from_record, fact_from_record};

#[cfg(test)]
mod tests {
    use super::communities::{collect_connected_entity_component, find_overlapping_communities};
    use super::entity_extraction::{
        build_ner_log_result, dedupe_entity_candidates, extract_entities,
    };
    use super::fact_extraction::{
        build_extract_log_result_with_metadata, should_extract_note_fact,
    };
    use super::statement_detection::{
        is_document_action_item, is_experience_statement, is_promise_statement,
        is_summary_like_note_candidate,
    };
    use super::summary_parser::{
        sanitized_content_for_entity_extraction, structured_summary_fact_candidates,
    };
    use super::{episode_from_record, fact_from_record, unwrap_record_string};
    use crate::models::EntityCandidate;
    use crate::models::Episode;
    use crate::models::ExtractedFact;
    use crate::models::FactType;
    use crate::service::EntityExtractor;
    use crate::service::error::MemoryError;
    use crate::storage::{DbClient, SurrealDbClient};
    use chrono::Utc;
    use serde_json::{Value, json};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn episode_from_record_parses_full_record() {
        let mut record = serde_json::Map::new();
        record.insert("episode_id".to_string(), json!("episode:test123"));
        record.insert("source_type".to_string(), json!("email"));
        record.insert("source_id".to_string(), json!("msg-123"));
        record.insert("content".to_string(), json!("Test content"));
        record.insert("t_ref".to_string(), json!("2024-01-15T10:30:00Z"));
        record.insert("t_ingested".to_string(), json!("2024-01-15T10:31:00Z"));
        record.insert("scope".to_string(), json!("org"));
        record.insert("visibility_scope".to_string(), json!("org"));
        record.insert("policy_tags".to_string(), json!(["tag1", "tag2"]));

        let episode = episode_from_record(&record).unwrap();
        assert_eq!(episode.episode_id, "episode:test123");
        assert_eq!(episode.source_type, "email");
        assert_eq!(episode.source_id, "msg-123");
        assert_eq!(episode.content, "Test content");
        assert_eq!(episode.scope, "org");
        assert_eq!(episode.visibility_scope, "org");
        assert_eq!(episode.policy_tags, vec!["tag1", "tag2"]);
    }

    #[test]
    fn episode_from_record_returns_none_for_missing_required_field() {
        let mut record = serde_json::Map::new();
        record.insert("episode_id".to_string(), json!("episode:test123"));

        assert!(episode_from_record(&record).is_none());
    }

    #[test]
    fn episode_from_record_handles_wrapped_string_values() {
        let mut record = serde_json::Map::new();
        record.insert(
            "episode_id".to_string(),
            json!({"String": "episode:test123"}),
        );
        record.insert("source_type".to_string(), json!({"String": "email"}));
        record.insert("source_id".to_string(), json!({"String": "msg-123"}));
        record.insert("content".to_string(), json!({"String": "Test"}));
        record.insert(
            "t_ref".to_string(),
            json!({"String": "2024-01-15T10:30:00Z"}),
        );
        record.insert(
            "t_ingested".to_string(),
            json!({"String": "2024-01-15T10:31:00Z"}),
        );
        record.insert("scope".to_string(), json!({"String": "org"}));
        record.insert(
            "policy_tags".to_string(),
            json!({"Array": [{"String": "tag1"}]}),
        );

        let episode = episode_from_record(&record).unwrap();
        assert_eq!(episode.episode_id, "episode:test123");
        assert_eq!(episode.policy_tags, vec!["tag1"]);
    }

    #[test]
    fn episode_from_record_uses_defaults_for_optional_fields() {
        let mut record = serde_json::Map::new();
        record.insert("episode_id".to_string(), json!("episode:test123"));
        record.insert("source_type".to_string(), json!("email"));
        record.insert("source_id".to_string(), json!("msg-123"));
        record.insert("content".to_string(), json!("Test"));
        record.insert("t_ref".to_string(), json!("2024-01-15T10:30:00Z"));
        record.insert("t_ingested".to_string(), json!("2024-01-15T10:31:00Z"));
        record.insert("scope".to_string(), json!("org"));

        let episode = episode_from_record(&record).unwrap();
        assert_eq!(episode.visibility_scope, "");
        assert!(episode.policy_tags.is_empty());
    }

    #[test]
    fn fact_from_record_parses_full_record() {
        let record = json!({
            "fact_id": "fact:test123",
            "fact_type": "note",
            "content": "Test fact",
            "quote": "Test quote",
            "source_episode": "episode:abc",
            "t_valid": "2024-01-15T10:30:00Z",
            "t_ingested": "2024-01-15T10:31:00Z",
            "t_invalid": "2024-01-16T10:30:00Z",
            "confidence": 0.95,
            "entity_links": ["entity:1", "entity:2"],
            "scope": "org",
            "policy_tags": ["tag1"],
            "provenance": {"source": "test"}
        });

        let fact = fact_from_record(&record).unwrap();
        assert_eq!(fact.fact_id, "fact:test123");
        assert_eq!(fact.fact_type, "note");
        assert_eq!(fact.content, "Test fact");
        assert_eq!(fact.quote, "Test quote");
        assert_eq!(fact.source_episode, "episode:abc");
        assert!((fact.confidence - 0.95).abs() < f64::EPSILON);
        assert_eq!(fact.entity_links, vec!["entity:1", "entity:2"]);
        assert_eq!(fact.scope, "org");
        assert_eq!(fact.policy_tags, vec!["tag1"]);
    }

    #[test]
    fn fact_from_record_handles_optional_fields() {
        let record = json!({
            "fact_id": "fact:test123",
            "fact_type": "note",
            "content": "Test",
            "quote": "Quote",
            "source_episode": "episode:abc",
            "t_valid": "2024-01-15T10:30:00Z",
            "scope": "org"
        });

        let fact = fact_from_record(&record).unwrap();
        assert!(fact.t_invalid.is_none());
        assert!(fact.t_invalid_ingested.is_none());
        assert!(fact.entity_links.is_empty());
        assert!(fact.policy_tags.is_empty());
        assert!((fact.confidence).abs() < f64::EPSILON);
    }

    #[test]
    fn fact_from_record_returns_none_for_invalid_record() {
        let record = json!({"invalid": "data"});
        assert!(fact_from_record(&record).is_none());
    }

    #[test]
    fn unwrap_record_string_handles_record_id() {
        let value = json!({"RecordId": {"table": "entity", "key": "alice"}});
        assert_eq!(
            unwrap_record_string(&value),
            Some("entity:alice".to_string())
        );
    }

    #[test]
    fn is_promise_statement_detects_promise_patterns() {
        assert!(is_promise_statement("i will finish this task"));
        assert!(is_promise_statement("i'll deliver the report tomorrow"));
        assert!(is_promise_statement("will complete the project"));
        assert!(is_promise_statement("going to implement the feature"));
        assert!(is_promise_statement("I will do this tomorrow"));
    }

    #[test]
    fn is_promise_statement_rejects_non_promise_patterns() {
        assert!(!is_promise_statement("this is just a note"));
        assert!(!is_promise_statement("meeting scheduled for tomorrow"));
        assert!(!is_promise_statement("review the document"));
        assert!(!is_promise_statement("the task is complete"));
    }

    #[test]
    fn is_promise_statement_detects_lowercase_variations() {
        assert!(is_promise_statement("i will finish this"));
        assert!(is_promise_statement("i'll deliver"));
        assert!(is_promise_statement("will complete the task"));
    }

    #[test]
    fn is_experience_statement_detects_preference_patterns() {
        assert!(is_experience_statement(
            "Alice Smith prefers weekly launch updates over ad-hoc pings."
        ));
        assert!(is_experience_statement("I enjoy quiet deep-work mornings."));
        assert!(is_experience_statement(
            "I tend to avoid high-rise buildings for accommodations."
        ));
        assert!(is_experience_statement(
            "I have a strong aversion to beachfront resorts."
        ));
        assert!(is_experience_statement(
            "I do not enjoy casinos or gaming environments."
        ));
    }

    #[test]
    fn is_experience_statement_rejects_non_preference_patterns() {
        assert!(!is_experience_statement("Atlas budget is $2M."));
        assert!(!is_experience_statement("I will send the deck tomorrow."));
    }

    #[test]
    fn is_document_action_item_detects_email_style_bullets() {
        assert!(is_document_action_item(
            "Subject: Atlas follow-up\n\nAction items:\n- Alice Smith: send revised deck by Friday\n- Bob Jones: review launch checklist by Monday"
        ));
    }

    #[test]
    fn is_document_action_item_rejects_plain_notes() {
        assert!(!is_document_action_item(
            "Meeting notes: Alice shared the deck."
        ));
        assert!(!is_document_action_item(
            "Action items: this section is empty for now"
        ));
    }

    #[test]
    fn is_summary_like_note_candidate_detects_dense_summary_content() {
        assert!(is_summary_like_note_candidate(
            "July 2025 planning summary: platform integrations ready, stakeholder approvals pending, response workflow scoped."
        ));
    }

    #[test]
    fn is_summary_like_note_candidate_rejects_short_content() {
        assert!(!is_summary_like_note_candidate("Short note only"));
    }

    #[test]
    fn should_extract_note_fact_requires_supported_source_type_and_no_existing_facts() {
        let episode = Episode {
            episode_id: "episode:test".to_string(),
            source_type: "requirement".to_string(),
            source_id: "summary-1".to_string(),
            content: "July 2025 planning summary: platform integrations ready, stakeholder approvals pending, response workflow scoped.".to_string(),
            t_ref: Utc::now(),
            t_ingested: Utc::now(),
            scope: "org".to_string(),
            visibility_scope: String::new(),
            policy_tags: Vec::new(),
        };

        assert!(should_extract_note_fact(&episode, &[]));
        assert!(!should_extract_note_fact(
            &Episode {
                source_type: "meeting".to_string(),
                ..episode.clone()
            },
            &[]
        ));
        assert!(should_extract_note_fact(
            &Episode {
                source_type: "meeting_summary".to_string(),
                ..episode.clone()
            },
            &[]
        ));
        assert!(!should_extract_note_fact(
            &episode,
            &[ExtractedFact {
                fact_id: "fact:test".to_string(),
                fact_type: "promise".to_string(),
            }]
        ));
    }

    #[test]
    fn structured_summary_fact_candidates_extract_labeled_and_heading_scoped_lines() {
        let candidates = structured_summary_fact_candidates(
            "Project decision summary:\n\n- Decision: Approve the cross-platform activation policy.\n- Decision: Keep legacy on-premise licenses separate.\n\nDocumentation facts:\n- Fact: Docs team needs final terminology for supported languages.",
        );

        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].fact_type, FactType::Decision.as_str());
        assert_eq!(
            candidates[0].content,
            "Approve the cross-platform activation policy."
        );
        assert_eq!(candidates[1].fact_type, FactType::Decision.as_str());
        assert_eq!(
            candidates[1].content,
            "Keep legacy on-premise licenses separate."
        );
        assert_eq!(candidates[2].fact_type, FactType::Note.as_str());
        assert_eq!(
            candidates[2].content,
            "Docs team needs final terminology for supported languages."
        );
    }

    #[test]
    fn structured_summary_fact_candidates_extract_markdown_headings_without_colons() {
        let candidates = structured_summary_fact_candidates(
            "# September 2025 program summary\n\n## Decisions Made\n1. Regional launch in South market approved for September 30.\n2. Response logging rollout approved for September 30.\n\n## Pending Items\n1. Complete global launch follow-up.\n2. Continue platform 1.5 development.",
        );

        assert_eq!(candidates.len(), 4);
        assert_eq!(candidates[0].fact_type, FactType::Decision.as_str());
        assert_eq!(
            candidates[0].content,
            "Regional launch in South market approved for September 30."
        );
        assert_eq!(candidates[1].fact_type, FactType::Decision.as_str());
        assert_eq!(
            candidates[1].content,
            "Response logging rollout approved for September 30."
        );
        assert_eq!(candidates[2].fact_type, FactType::Note.as_str());
        assert_eq!(candidates[2].content, "Complete global launch follow-up.");
        assert_eq!(candidates[3].fact_type, FactType::Note.as_str());
        assert_eq!(candidates[3].content, "Continue platform 1.5 development.");
    }

    #[test]
    fn structured_summary_fact_candidates_extract_thematic_heading_lines_with_heading_context() {
        let candidates = structured_summary_fact_candidates(
            "# Monthly coordination summary\n\n## Release Activities\n- Finalize phased rollout checklist.\n- Publish support handoff notes.\n\n## Capacity Planning\n- Prepare archive review for next quarter.",
        );

        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].fact_type, FactType::Note.as_str());
        assert_eq!(
            candidates[0].content,
            "Release Activities: Finalize phased rollout checklist."
        );
        assert_eq!(candidates[0].quote, "Finalize phased rollout checklist.");
        assert_eq!(
            candidates[1].content,
            "Release Activities: Publish support handoff notes."
        );
        assert_eq!(
            candidates[2].content,
            "Capacity Planning: Prepare archive review for next quarter."
        );
    }

    #[test]
    fn sanitized_content_for_entity_extraction_strips_structural_labels() {
        let sanitized = sanitized_content_for_entity_extraction(
            "Architecture decisions:\n- Decision: Platform becomes the umbrella product name.\n- Fact: Legacy bridge remains active during rollout.",
        );

        assert!(!sanitized.contains("Decision:"));
        assert!(!sanitized.contains("Fact:"));
        assert!(!sanitized.contains("Architecture decisions:"));
        assert!(sanitized.contains("Platform becomes the umbrella product name."));
        assert!(sanitized.contains("Legacy bridge remains active during rollout."));
    }

    #[test]
    fn sanitized_content_for_entity_extraction_strips_thematic_section_headings() {
        let sanitized = sanitized_content_for_entity_extraction(
            "Release Activities:\n- Finalize phased rollout checklist.\n- Publish support handoff notes.",
        );

        assert!(!sanitized.contains("Release Activities:"));
        assert!(sanitized.contains("Finalize phased rollout checklist."));
        assert!(sanitized.contains("Publish support handoff notes."));
    }

    #[test]
    fn dedupe_entity_candidates_merges_duplicate_names_and_aliases() {
        use crate::models::EntityCandidate;
        use std::collections::BTreeSet;

        let candidates = dedupe_entity_candidates(vec![
            EntityCandidate {
                entity_type: "person".to_string(),
                canonical_name: "Avery Stone".to_string(),
                aliases: vec!["A. Stone".to_string()],
            },
            EntityCandidate {
                entity_type: "company".to_string(),
                canonical_name: "Avery Stone".to_string(),
                aliases: vec!["Stone Group".to_string()],
            },
            EntityCandidate {
                entity_type: "person".to_string(),
                canonical_name: "Avery Stone".to_string(),
                aliases: vec!["Avery S.".to_string()],
            },
            EntityCandidate {
                entity_type: "organization".to_string(),
                canonical_name: "Operations Forum".to_string(),
                aliases: vec![],
            },
        ]);

        assert_eq!(candidates.len(), 2);

        let avery = candidates
            .iter()
            .find(|candidate| candidate.canonical_name == "Avery Stone")
            .expect("deduped person candidate");
        assert_eq!(avery.entity_type, "person");
        assert_eq!(
            avery.aliases.iter().cloned().collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "A. Stone".to_string(),
                "Avery S.".to_string(),
                "Stone Group".to_string(),
            ])
        );
    }

    #[test]
    fn build_ner_log_result_includes_provider_entity_count_and_zero_shot_count() {
        let result = build_ner_log_result("gliner", 3, Some(6), None);

        assert_eq!(
            result.get("provider").and_then(Value::as_str),
            Some("gliner")
        );
        assert_eq!(result.get("entity_count").and_then(Value::as_u64), Some(3));
        assert_eq!(
            result.get("zero_shot_label_count").and_then(Value::as_u64),
            Some(6)
        );
        assert!(
            result.get("error").is_none(),
            "error field should be omitted when not provided"
        );
    }

    #[test]
    fn build_ner_log_result_omits_zero_shot_label_count_when_none() {
        let result = build_ner_log_result("regex", 0, None, None);

        assert_eq!(
            result.get("provider").and_then(Value::as_str),
            Some("regex")
        );
        assert_eq!(result.get("entity_count").and_then(Value::as_u64), Some(0));
        assert!(
            result.get("zero_shot_label_count").is_none(),
            "zero_shot_label_count should be omitted when None"
        );
    }

    #[test]
    fn build_ner_log_result_includes_error_when_provided() {
        let result = build_ner_log_result("gliner", 0, Some(3), Some("tokenization failed"));

        assert_eq!(
            result.get("error").and_then(Value::as_str),
            Some("tokenization failed")
        );
        assert_eq!(result.get("entity_count").and_then(Value::as_u64), Some(0));
    }

    #[test]
    fn build_extract_log_result_includes_episode_metadata_and_note_fallback_usage() {
        let episode = Episode {
            episode_id: "episode:test".to_string(),
            source_type: "requirement".to_string(),
            source_id: "summary-1".to_string(),
            content: "July 2025 planning summary: platform integrations ready.".to_string(),
            t_ref: Utc::now(),
            t_ingested: Utc::now(),
            scope: "org".to_string(),
            visibility_scope: String::new(),
            policy_tags: Vec::new(),
        };

        let result = build_extract_log_result_with_metadata(
            Some(&episode),
            2,
            &[ExtractedFact {
                fact_id: "fact:test".to_string(),
                fact_type: "note".to_string(),
            }],
            3,
            1,
            true,
            0,
        );

        assert_eq!(result.get("entities").and_then(Value::as_u64), Some(2));
        assert_eq!(result.get("facts").and_then(Value::as_u64), Some(1));
        assert_eq!(result.get("links").and_then(Value::as_u64), Some(3));
        assert_eq!(result.get("warnings").and_then(Value::as_u64), Some(1));
        assert_eq!(
            result.get("source_type").and_then(Value::as_str),
            Some("requirement")
        );
        assert_eq!(
            result.get("content_chars").and_then(Value::as_u64),
            Some(56)
        );
        assert_eq!(
            result.get("note_fallback_used").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            result
                .get("structured_line_fact_count")
                .and_then(Value::as_u64),
            Some(0)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn extract_entities_does_not_block_runtime_for_local_gliner_provider() {
        struct BlockingGlinerExtractor;

        #[async_trait::async_trait]
        impl EntityExtractor for BlockingGlinerExtractor {
            fn provider_name(&self) -> &'static str {
                "gliner"
            }

            async fn extract_candidates(
                &self,
                _content: &str,
            ) -> Result<Vec<EntityCandidate>, MemoryError> {
                std::thread::sleep(Duration::from_millis(250));
                Ok(Vec::new())
            }
        }

        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory("episode-test", "org", "warn")
                .await
                .expect("connect in memory"),
        );
        db_client
            .apply_migrations("org")
            .await
            .expect("apply migrations");

        let mut service = crate::service::MemoryService::new(
            db_client,
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("create service");
        service.entity_extractor = Arc::new(BlockingGlinerExtractor);

        let ticker = tokio::spawn(async move {
            let start = Instant::now();
            tokio::time::sleep(Duration::from_millis(50)).await;
            start.elapsed()
        });
        tokio::task::yield_now().await;

        let _ = extract_entities(&service.build_context(), "Atlas project status", None)
            .await
            .expect("extract entities");
        let tick_elapsed = ticker.await.expect("join ticker");

        assert!(
            tick_elapsed < Duration::from_millis(150),
            "local gliner extraction blocked the runtime for {:?}",
            tick_elapsed
        );
    }

    #[tokio::test]
    async fn collect_connected_entity_component_uses_neighbor_queries_instead_of_edge_scan() {
        use crate::storage::{DbClient, GraphDirection};
        use std::sync::Arc;

        struct NeighborOnlyDbClient;

        #[async_trait::async_trait]
        impl DbClient for NeighborOnlyDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            #[allow(clippy::too_many_arguments)]
            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
                _fact_types: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                panic!("community traversal should not scan the full edge table")
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                node_id: &str,
                _cutoff: &str,
                direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                let mk = |from_id: &str, relation: &str, to_id: &str| {
                    json!({
                        "edge_id": format!("edge:{from_id}:{relation}:{to_id}"),
                        "in": from_id,
                        "relation": relation,
                        "out": to_id,
                        "t_valid": "2024-01-01T00:00:00Z",
                        "t_ingested": "2024-01-01T00:00:00Z"
                    })
                };

                Ok(match (node_id, direction) {
                    ("entity:alice", GraphDirection::Outgoing) => {
                        vec![mk("entity:alice", "mentioned_in", "episode:shared")]
                    }
                    ("episode:shared", GraphDirection::Incoming) => vec![
                        mk("entity:alice", "mentioned_in", "episode:shared"),
                        mk("entity:bob", "mentioned_in", "episode:shared"),
                    ],
                    ("entity:bob", GraphDirection::Outgoing) => {
                        vec![mk("entity:bob", "involved_in", "fact:joint")]
                    }
                    ("fact:joint", GraphDirection::Incoming) => vec![
                        mk("entity:bob", "involved_in", "fact:joint"),
                        mk("entity:carol", "involved_in", "fact:joint"),
                    ],
                    _ => vec![],
                })
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }
            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }

            async fn select_facts_by_triple(
                &self,
                _namespace: &str,
                _query_text: &str,
                _cutoff: &str,
                _limit: usize,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entities_by_ids(
                &self,
                _namespace: &str,
                _entity_ids: &[String],
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(Vec::new())
            }

            async fn select_edges_for_triple(
                &self,
                _namespace: &str,
                _in_id: &str,
                _relation: &str,
                _out_id: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(Vec::new())
            }

            async fn count_facts_needing_reembed(
                &self,
                _namespace: &str,
                _target_signature: &str,
            ) -> Result<usize, MemoryError> {
                Ok(0)
            }

            async fn select_facts_needing_reembed(
                &self,
                _namespace: &str,
                _target_signature: &str,
                _last_completed_fact_id: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(Vec::new())
            }

            async fn select_episodes_by_content(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(NeighborOnlyDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .unwrap();

        let connected = collect_connected_entity_component(
            &service.build_context(),
            &["entity:alice".to_string()],
            "org",
        )
        .await
        .unwrap();

        assert_eq!(
            connected,
            vec![
                "entity:alice".to_string(),
                "entity:bob".to_string(),
                "entity:carol".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn find_overlapping_communities_uses_index_based_lookup() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        static SELECT_COMMUNITIES_BY_MEMBER_CALLED: AtomicBool = AtomicBool::new(false);
        static SELECT_TABLE_CALLED: AtomicBool = AtomicBool::new(false);

        #[derive(Clone)]
        struct IndexLookupDbClient;

        #[async_trait::async_trait]
        impl crate::storage::DbClient for IndexLookupDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<serde_json::Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                SELECT_TABLE_CALLED.store(true, Ordering::SeqCst);
                Ok(vec![])
            }

            #[allow(clippy::too_many_arguments)]
            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
                _fact_types: &[String],
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: crate::storage::GraphDirection,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<serde_json::Value>, MemoryError> {
                Ok(None)
            }

            async fn select_entities_batch(
                &self,
                _namespace: &str,
                _names: &[String],
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_ann(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_vec: &[f64],
                _limit: i32,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_communities_by_member_entities(
                &self,
                _namespace: &str,
                _member_entities: &[String],
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                SELECT_COMMUNITIES_BY_MEMBER_CALLED.store(true, Ordering::SeqCst);
                Ok(vec![])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: serde_json::Value,
            ) -> Result<serde_json::Value, MemoryError> {
                Ok(serde_json::Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: serde_json::Value,
                _namespace: &str,
            ) -> Result<serde_json::Value, MemoryError> {
                Ok(serde_json::Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: serde_json::Value,
                _namespace: &str,
            ) -> Result<serde_json::Value, MemoryError> {
                Ok(serde_json::Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<serde_json::Value>,
                _namespace: &str,
            ) -> Result<serde_json::Value, MemoryError> {
                Ok(serde_json::Value::Null)
            }

            async fn select_entities_by_ids(
                &self,
                _namespace: &str,
                _ids: &[String],
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts(
                &self,
                _namespace: &str,
                _limit: i32,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_episodes_for_archival(
                &self,
                _namespace: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_active_facts_by_episode(
                &self,
                _namespace: &str,
                _episode_id: &str,
                _cutoff: &str,
                _limit: i32,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }

            async fn select_facts_by_triple(
                &self,
                _namespace: &str,
                _query_text: &str,
                _cutoff: &str,
                _limit: usize,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_for_triple(
                &self,
                _namespace: &str,
                _in_id: &str,
                _relation: &str,
                _out_id: &str,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(Vec::new())
            }

            async fn count_facts_needing_reembed(
                &self,
                _namespace: &str,
                _target_signature: &str,
            ) -> Result<usize, MemoryError> {
                Ok(0)
            }

            async fn select_facts_needing_reembed(
                &self,
                _namespace: &str,
                _target_signature: &str,
                _last_completed_fact_id: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(Vec::new())
            }

            async fn select_episodes_by_content(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
                _project: Option<&str>,
            ) -> Result<Vec<serde_json::Value>, MemoryError> {
                Ok(vec![])
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(IndexLookupDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .unwrap();

        let _ = find_overlapping_communities(
            &service.build_context(),
            "org",
            &["entity:alice".to_string(), "entity:bob".to_string()],
        )
        .await;

        assert!(
            SELECT_COMMUNITIES_BY_MEMBER_CALLED.load(Ordering::SeqCst),
            "find_overlapping_communities should call select_communities_by_member_entities"
        );
        assert!(
            !SELECT_TABLE_CALLED.load(Ordering::SeqCst),
            "find_overlapping_communities should NOT call select_table (full scan)"
        );
    }
}
