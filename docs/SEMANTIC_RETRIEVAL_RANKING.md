# Semantic retrieval ranking inputs

This repository now contains the scaffolding required to support hybrid retrieval, but hybrid ranking is **not enabled by default** yet.

## Current posture

- Full-text search remains the active retrieval primitive for ranked context assembly.
- Graph traversal remains the active retrieval primitive for relationship-aware context expansion.
- Embedding fields and vector indexes are present only as schema/application scaffolding.
- The default `NullEmbedder` intentionally returns no vectors, so current runtime behavior stays deterministic.

## Planned hybrid ranking inputs

When semantic retrieval is enabled in a later wave, ranking should combine three independent signals:

1. **FTS relevance**
   - lexical match quality against fact and episode content
   - exact token matches and analyzer-normalized matches
2. **Graph relevance**
   - relationship distance from requested entities
   - edge confidence, validity window, and provenance quality
3. **Embedding relevance**
   - vector similarity between the query embedding and stored episode/entity/fact embeddings
   - used as an additional signal, not a replacement for temporal or policy filtering

## Guardrails for later enablement

Before a real embedder is turned on by default:

- choose and document a single production vector dimension
- confirm migration/index compatibility for all persisted embedding fields
- keep policy/scope filtering ahead of ranking
- validate that hybrid ranking is stable under missing vectors
- add explicit acceptance tests for mixed FTS + graph + embedding scoring

## Deployment note for vector dimensions

- The repository default remains `4` so test fixtures and inert `NullEmbedder` flows stay lightweight.
- Real embedder configurations should set `SURREALDB_EMBEDDING_DIMENSION` explicitly:
   - `nomic-embed-text` → `768`
   - `mxbai-embed-large` → `1024`
   - `text-embedding-3-small` → `1536`
- Changing the dimension for an already-initialized database is a manual migration step today. SurrealDB HNSW indexes must be dropped and recreated (or the database rebuilt) before persisting vectors with the new size.

## Why the default stays inert

Keeping the default embedder inert avoids accidental ranking drift while the retrieval blend is still being designed. That gives us the extension points now without changing observable behavior today.
