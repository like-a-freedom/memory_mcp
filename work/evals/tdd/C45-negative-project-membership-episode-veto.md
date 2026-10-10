# C45: explicit negative project membership vetoes episode recall

## Failure reproduced (RED)

For `Memory MCP release notes`, the source `Release notes are part of Trackly, not Memory MCP.` passed the single-term episode admission and a controlled positive cross-encoder returned it. The production assembly regression failed before the veto was added. The no-reranker regression also failed when the general episode admission path lacked the veto.

```text
rtk proxy cargo test -p memory_mcp --locked --features eval-support --test assemble_context_semantic_quality positive_reranker_cannot_rescue_episode_with_explicitly_excluded_project_membership -- --exact --nocapture
exit 101: the explicitly excluded source episode was returned.

rtk proxy cargo test -p memory_mcp --locked --features eval-support --test assemble_context_semantic_quality lexical_episode_admission_rejects_explicitly_excluded_project_membership -- --exact --nocapture
exit 101 with the general admission veto removed: the source episode was returned.
```

## Minimal fix (GREEN)

Apply `negative_project_membership_conflicts` as a hard refusal before temporal and topical checks in both normal and rerankable episode admission. A reranker cannot recover a candidate that contradicts an explicit project exclusion.

The regression tests use the real production assembly path and embedded database. The positive controlled reranker is configured to return `+10`; the assertion also verifies that hard exclusion happens before model scoring. A separate no-reranker production case covers the legacy lexical admission path.

```text
rtk proxy cargo test -p memory_mcp --locked --features eval-support --test assemble_context_semantic_quality -- --nocapture
```
