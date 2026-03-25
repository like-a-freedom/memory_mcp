//! Embedding provider abstractions.

use async_trait::async_trait;

use super::MemoryError;

/// Produces optional vector embeddings for text content.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Returns an embedding for the supplied text, or `None` when embedding is disabled.
    async fn embed_text(&self, text: &str) -> Result<Option<Vec<f32>>, MemoryError>;
}

/// Default embedding provider used in tests and local development until a real provider is configured.
#[derive(Debug, Default)]
pub struct NullEmbedder;

#[async_trait]
impl EmbeddingProvider for NullEmbedder {
    async fn embed_text(&self, _text: &str) -> Result<Option<Vec<f32>>, MemoryError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn null_embedder_returns_none() {
        let embedder = NullEmbedder;
        let embedding = embedder.embed_text("hello world").await.unwrap();
        assert_eq!(embedding, None);
    }
}
