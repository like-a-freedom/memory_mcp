# Deprecated semantic retrieval note

This document is superseded by `docs/SIMPLIFIED_SEARCH_REDESIGN_SPEC.md`.

## Status

The earlier plan to keep dormant embedding-based ranking scaffolding is no longer the approved target direction.

The current approved target redesign is:

- remove embeddings from the runtime,
- remove HNSW/vector-index search from the retrieval path,
- make BM25/full-text retrieval the primary candidate generator,
- use bounded graph expansion as the secondary retrieval mechanism,
- merge results deterministically.

Until the code changes land, embedding-related notes in older documents describe historical implementation context only.
