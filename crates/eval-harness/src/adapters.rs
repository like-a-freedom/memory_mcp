use crate::corpus::adapters::ExternalCase;
use crate::error::EvalError;

#[derive(Debug, Clone)]
pub struct CanonicalFact {
    pub fact_id: String,
    pub episode_id: String,
    pub content: String,
    pub scope: String,
    pub project: Option<String>,
    pub t_valid: String,
    pub embedding: Option<Vec<f32>>,
    pub embedding_model: Option<String>,
    pub embedding_provider: Option<String>,
    pub embedding_dimension: Option<usize>,
}

#[derive(Debug)]
pub struct ImportedFact {
    pub fact_id: String,
    pub episode_id: String,
}

#[derive(Debug)]
pub struct ImportResult {
    pub facts: Vec<ImportedFact>,
    pub total_imported: usize,
}

pub fn validate_canonical_facts(facts: &[CanonicalFact]) -> Result<(), EvalError> {
    for fact in facts {
        if fact.content.trim().is_empty() {
            return Err(EvalError::InvalidInput(format!(
                "fact {} has empty content",
                fact.fact_id
            )));
        }

        if fact.embedding.is_some() {
            if let Some(ref model) = fact.embedding_model {
                if model.is_empty() {
                    return Err(EvalError::InvalidInput(format!(
                        "fact {} has embedding but empty model",
                        fact.fact_id
                    )));
                }
            } else {
                return Err(EvalError::InvalidInput(format!(
                    "fact {} has embedding but no model",
                    fact.fact_id
                )));
            }
        }
    }
    Ok(())
}

pub fn import_canonical_facts(facts: &[CanonicalFact]) -> Result<ImportResult, EvalError> {
    validate_canonical_facts(facts)?;

    let imported: Vec<ImportedFact> = facts
        .iter()
        .map(|f| ImportedFact {
            fact_id: f.fact_id.clone(),
            episode_id: f.episode_id.clone(),
        })
        .collect();

    Ok(ImportResult {
        total_imported: imported.len(),
        facts: imported,
    })
}

pub fn facts_for_case(case: &ExternalCase) -> Vec<CanonicalFact> {
    case.facts
        .iter()
        .enumerate()
        .map(|(idx, fact)| {
            let fact_id = format!("{}:fact:{idx}", case.id);
            let episode_id = format!("{}:ep:{idx}", case.id);

            CanonicalFact {
                fact_id,
                episode_id,
                content: fact.content.clone(),
                scope: case.scope.clone(),
                project: None,
                t_valid: fact.t_valid.clone(),
                embedding: None,
                embedding_model: None,
                embedding_provider: None,
                embedding_dimension: None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::adapters::{RetrievalExpectation, SeedFact};

    fn test_case() -> ExternalCase {
        ExternalCase {
            id: "test:case-1".into(),
            dataset: "test".into(),
            description: "test case".into(),
            query: "test query".into(),
            scope: "org".into(),
            budget: 5,
            facts: vec![
                SeedFact {
                    content: "Fact one content".into(),
                    t_valid: "2026-01-01T00:00:00Z".into(),
                },
                SeedFact {
                    content: "Fact two content".into(),
                    t_valid: "2026-01-02T00:00:00Z".into(),
                },
            ],
            expected: RetrievalExpectation {
                tier: "direct".into(),
                must_contain: vec!["Fact one".into()],
                min_recall_at_k: 1.0,
            },
            metadata: serde_json::json!({}),
        }
    }

    #[test]
    fn facts_for_case_generates_stable_ids() {
        let case = test_case();
        let facts = facts_for_case(&case);
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].fact_id, "test:case-1:fact:0");
        assert_eq!(facts[0].episode_id, "test:case-1:ep:0");
        assert_eq!(facts[1].fact_id, "test:case-1:fact:1");
    }

    #[test]
    fn facts_preserve_content_and_scope() {
        let case = test_case();
        let facts = facts_for_case(&case);
        assert_eq!(facts[0].content, "Fact one content");
        assert_eq!(facts[0].scope, "org");
        assert_eq!(facts[0].t_valid, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn facts_have_no_embedding_by_default() {
        let case = test_case();
        let facts = facts_for_case(&case);
        assert!(facts[0].embedding.is_none());
    }

    #[test]
    fn importer_rejects_empty_content() {
        let facts = vec![CanonicalFact {
            fact_id: "f1".into(),
            episode_id: "e1".into(),
            content: "  ".into(),
            scope: "org".into(),
            project: None,
            t_valid: "2026-01-01T00:00:00Z".into(),
            embedding: None,
            embedding_model: None,
            embedding_provider: None,
            embedding_dimension: None,
        }];
        assert!(import_canonical_facts(&facts).is_err());
    }

    #[test]
    fn importer_rejects_embedding_without_model() {
        let facts = vec![CanonicalFact {
            fact_id: "f1".into(),
            episode_id: "e1".into(),
            content: "valid content".into(),
            scope: "org".into(),
            project: None,
            t_valid: "2026-01-01T00:00:00Z".into(),
            embedding: Some(vec![0.1, 0.2, 0.3]),
            embedding_model: None,
            embedding_provider: None,
            embedding_dimension: Some(3),
        }];
        assert!(import_canonical_facts(&facts).is_err());
    }

    #[test]
    fn importer_accepts_valid_facts() {
        let facts = vec![CanonicalFact {
            fact_id: "f1".into(),
            episode_id: "e1".into(),
            content: "valid content".into(),
            scope: "org".into(),
            project: None,
            t_valid: "2026-01-01T00:00:00Z".into(),
            embedding: Some(vec![0.1, 0.2]),
            embedding_model: Some("test-model".into()),
            embedding_provider: Some("test".into()),
            embedding_dimension: Some(2),
        }];
        let result = import_canonical_facts(&facts).unwrap();
        assert_eq!(result.total_imported, 1);
        assert_eq!(result.facts[0].fact_id, "f1");
    }
}
