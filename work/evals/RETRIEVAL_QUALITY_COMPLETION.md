# Retrieval-quality plan completion status

The code and local TDD plan slices are implemented. Native retrieval-quality acceptance is still incomplete, so the overall plan is not 100% complete.

| Slice | Local status | Evidence boundary |
|---|---|---|
| Scoring contract and eval-harness suite | Green | Full eval-harness package; no source corpus score claimed |
| Semantic evidence, threshold boundary, provider outage | Green | Controlled component provider and embedded SurrealDB |
| Candidate admission, fallback/rescue, graph, community/repeated-topic | Green | Library and production-assembly tests |
| Duplicate view and explain provenance access | Green | Persisted relation and embedded-store integration tests |
| Wire contract and error/guidance behavior | Green | Production serialization and error-path tests |
| Temporal relations and historical projection | Green | Embedded-store and production-assembly tests |
| Exact cache identity and mutation invalidation | Green | Unit, embedded-store, and barrier-controlled tests |
| Strict eval coverage, finite metrics, access gates | Green | Full harness suite plus deliberate mutation failures |
| Frozen native retrieval-quality corpus and release score | Pending | No retrieval-quality.json source bundle and no matching native provider/index signature are present |

The existing 61-case diagnostic has four known recall misses and is diagnostic only. It is not relabeled or used as acceptance. Controlled embeddings validate mechanics and boundaries; they do not stand in for a real model.

To close the last gate without weakening it, add an independently reviewed, licensed, immutable retrieval corpus with query/evidence/prohibited labels and source lineage; record its revision and checksums; prepare the corpus using the actual provider/model and index; pin the provider/model, embedding, index, extractor, and claim-stage signatures; then run the retrieval-quality suite with exact coverage and the fail-closed PR/release thresholds. The current checkout has no provider configuration for that native run.
