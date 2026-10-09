# A7 wire contract GREEN evidence

Base revision: `355dc92206e341d02a61a6066cd7ec5e49f21ce0`.

Implemented behavior at the actual tool serialization boundary:

- Empty results receive the exact no-match guidance; nonempty results retain
  the existing `explain` guidance.
- Verbose items expose typed, optional `retrieval_evidence`; compact items omit
  it while preserving the same selected fact IDs.
- The rationale's `relevance=` label is rewritten from the final normalized
  score used by the response item, with the same two-decimal rendering.
- Storage/index failures remain errors through the tool call. A component
  provider/index integration verifies that verbose `semantic_similarity`
  matches the controlled provider/index cosine evidence.

GREEN commands and results:

- `rtk cargo test -p memory_mcp --locked --test assemble_context_wire_contract -- --test-threads=1` — 2 passed.
- `rtk cargo test -p memory_mcp --locked --features test-fixtures,eval-support,streamable-http --lib assembled_context_item` — 10 passed.
- `rtk cargo test -p memory_mcp --locked --features test-fixtures,eval-support --test assemble_context_semantic_quality -- --test-threads=1` — 8 passed.

This is component and embedded-store evidence, not a source-reviewed retrieval
quality acceptance run. Reviewed corpus, independent labels, pinned provider
and index signatures, and G0/G5 remain open.
