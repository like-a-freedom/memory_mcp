# C43: reranker-aware choice between raw episodes and facts

The production fallback strategy compared raw episode passages and selected facts by lexical overlap only, even though both had already been scored by the same configured cross-encoder. A relevant single-anchor source turn could therefore lose to a lexically stronger distractor fact.

RED: `rtk proxy cargo test -p memory_mcp --lib --locked --features eval-support memory::retrieval::budget::tests::prefers_episode_when_same_reranker_ranks_it_above_lexically_stronger_fact -- --nocapture` failed: the episode score was 8.7 and the fact score was 2.2, but lexical overlap selected the fact.

GREEN: compare reranker scores only when all selected candidates have finite scores from one identical model signature; otherwise retain the existing lexical rule. Graph-expanded facts keep precedence. All 16 budget unit tests and all 8 episode assembly integration tests passed.

Native r29 is valid but unchanged from r28: 37 cases, 23 passed, 14 quality failures, 0 invalid, and the same six metrics. `locomo-conv-30-002` still selected `D2:11`; this implementation-level behavior is covered, but no aggregate native gain is claimed. Artifact: `work/evals/native-r29-reranker-aware-episode-selection.json`.
