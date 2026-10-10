//! Native inference diagnostics. This adapter never changes model inputs or scores.
use async_trait::async_trait;
use memory_mcp::{MemoryError, embedding::providers::EmbeddingProvider};
use std::sync::{Arc, Mutex};

pub(super) struct TracedProvider {
    inner: Arc<dyn EmbeddingProvider>,
    rows: Mutex<Vec<serde_json::Value>>,
}
impl TracedProvider {
    pub(super) fn new(inner: Arc<dyn EmbeddingProvider>) -> Self {
        Self {
            inner,
            rows: Mutex::new(Vec::new()),
        }
    }
    pub(super) fn drain(&self) -> Vec<serde_json::Value> {
        std::mem::take(
            &mut *self
                .rows
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}
#[async_trait]
impl EmbeddingProvider for TracedProvider {
    fn is_enabled(&self) -> bool {
        self.inner.is_enabled()
    }
    fn provider_name(&self) -> &'static str {
        self.inner.provider_name()
    }
    fn dimension(&self) -> usize {
        self.inner.dimension()
    }
    fn reranking_signature(&self) -> Option<&str> {
        self.inner.reranking_signature()
    }
    fn reranking_admission_floor(&self) -> f64 {
        self.inner.reranking_admission_floor()
    }
    async fn embed(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
        self.inner.embed(input).await
    }
    async fn embed_query(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
        self.inner.embed_query(input).await
    }
    async fn rerank(
        &self,
        query: &str,
        documents: &[String],
    ) -> Result<Option<Vec<f64>>, MemoryError> {
        let result = self.inner.rerank(query, documents).await?;
        if let Some(scores) = &result {
            let mut rows = self
                .rows
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (document, score) in documents.iter().zip(scores) {
                rows.push(serde_json::json!({"query":query, "document":document, "score":score}));
            }
        }
        Ok(result)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Provider;
    #[async_trait]
    impl EmbeddingProvider for Provider {
        fn is_enabled(&self) -> bool {
            true
        }
        fn provider_name(&self) -> &'static str {
            "controlled-trace-test"
        }
        fn dimension(&self) -> usize {
            1
        }
        async fn embed(&self, _: &str) -> Result<Vec<f64>, MemoryError> {
            Ok(vec![1.0])
        }
        async fn embed_query(&self, _: &str) -> Result<Vec<f64>, MemoryError> {
            Ok(vec![2.0])
        }
        fn reranking_signature(&self) -> Option<&str> {
            Some("trace-test-v1")
        }
        fn reranking_admission_floor(&self) -> f64 {
            0.5
        }
        async fn rerank(
            &self,
            query: &str,
            documents: &[String],
        ) -> Result<Option<Vec<f64>>, MemoryError> {
            assert_eq!(query, "Which team?");
            assert_eq!(
                documents,
                &["Original question".to_owned(), "Original answer".to_owned()]
            );
            Ok(Some(vec![4.0, -2.0]))
        }
    }
    #[tokio::test]
    async fn diagnostics_keep_negative_candidates_without_changing_inference() {
        let provider = TracedProvider::new(Arc::new(Provider));
        let scores = provider
            .rerank(
                "Which team?",
                &["Original question".into(), "Original answer".into()],
            )
            .await
            .expect("inference");
        assert_eq!(scores, Some(vec![4.0, -2.0]));
        let rows = provider.drain();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1]["score"], -2.0);
        assert_eq!(rows[1]["document"], "Original answer");
        assert_eq!(rows[1]["query"], "Which team?");
        assert!(provider.drain().is_empty());
        assert_eq!(provider.embed_query("q").await.expect("query"), vec![2.0]);
        assert_eq!(provider.reranking_signature(), Some("trace-test-v1"));
        assert_eq!(provider.reranking_admission_floor(), 0.5);
    }
}
