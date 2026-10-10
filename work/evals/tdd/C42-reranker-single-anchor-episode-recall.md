# C42 reranker recovery for single-anchor episode evidence

The integration scenario ingests the published Jon/Gina conversation turns and a lexical distractor through production ingest, then assembles context for the original multi-participant question. One relevant turn names only Jon, so lexical admission rejected it before reranking even though the cross-encoder could distinguish it from the distractor.

RED: `rtk proxy cargo test -p memory_mcp --locked --features eval-support --test assemble_context_semantic_quality episode_reranker_can_recover_a_single_anchor_passage_for_each_participant -- --nocapture` failed because assembly returned only Gina's turn and the reranker never saw Jon's.

GREEN: the same case passed after reranker-enabled episode retrieval retained one-term candidates subject to temporal checks and let the calibrated reranker floor decide relevance. The existing explicit-month test also ran with reranking enabled and continued to reject the out-of-month turn.
