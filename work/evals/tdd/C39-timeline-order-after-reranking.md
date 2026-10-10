# C39 timeline order after reranking

The regression uses the public `assemble_context` path, two timeline facts returned in chronological order, and a deterministic provider that scores the later fact higher.

RED: `rtk proxy cargo test -p memory_mcp --locked --lib assemble_context_preserves_timeline_order_after_reranking -- --nocapture` failed because output was `timeline-late, timeline-early` rather than `timeline-early, timeline-late`.

GREEN: the same command passed after timeline mode was carried to final reranking and the filter preserved its input order for timelines. Relevance sorting remains active for ordinary queries.
