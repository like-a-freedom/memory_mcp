# C44: preserve selected fact evidence during episode fallback

## Failure reproduced (RED)

The same configured reranker scored a raw Jon episode above the selected fact pack, so `assemble_context` returned only the episode and discarded the independently selected Gina billing fact. The unit regression and the embedded-database production assembly regression both failed before the policy change.

```text
rtk proxy cargo test -p memory_mcp --lib --locked --features eval-support memory::retrieval::budget::tests::partial_episode_cannot_replace_collective_fact_evidence_even_with_higher_reranker_score -- --nocapture
exit 101: the higher-scoring partial episode was preferred.

rtk proxy cargo test -p memory_mcp --locked --features eval-support --test assemble_context_semantic_quality higher_scoring_partial_episode_does_not_replace_a_complementary_fact -- --exact --nocapture
exit 101: the assembled pack omitted the persisted Gina fact.
```

## Minimal fix (GREEN)

Before either reranker or lexical preference can replace selected facts, union the query terms supported by the selected fact pack and by the episode pack. Keep the facts if the episode pack does not preserve every selected fact term. Comparable same-model reranker scores still decide the winner when that evidence is preserved; different or missing model signatures keep the conservative lexical path.

The positive same-model case remains covered: an episode can win when it preserves the selected fact terms. The production test uses real embedded storage, ingestion, retrieval and assembly with a controlled provider to establish deterministic candidate order; it makes no native model-quality claim.

```text
rtk proxy cargo test -p memory_mcp --lib --locked --features eval-support memory::retrieval::budget::tests:: -- --nocapture
rtk proxy cargo test -p memory_mcp --locked --features eval-support --test assemble_context_semantic_quality -- --nocapture
```

Native r30 must be run with the unchanged corpus, model bytes, profile and thresholds. No aggregate gain is assumed.
