# A2 semantic evidence and eval-service RED

## Raw semantic evidence

`rtk cargo test -p memory_mcp --locked --lib raw_similarity_survives_fusion` produced a behavioral failure after the candidate/evidence scaffolding compiled: actual `semantic_similarity` was `None`, expected `Some(0.82)`. The test exercised a semantic candidate merged with a lexical candidate.

## Eval-service seam

Tests were written against the planned public, feature-gated `eval_support::retrieval` factory. The first run correctly showed the factory module did not exist. Once a compile-only incomplete factory was added, `rtk cargo test -p memory_mcp --locked --features eval-support,test-fixtures --test assemble_context_semantic_quality` compiled and all three behavior assertions failed: invalid thresholds had no threshold-specific rejection, invalid namespace/stage had no targeted rejection, and valid service construction failed. This distinguishes the behavioral RED from the initial missing-symbol compile failure.

## Database dimension evidence

The first real assembly probe used a 2-D controlled component provider against the migrated in-memory database's 1536-D fact index. SurrealDB rejected the ANN query with `Incorrect vector dimension (2). Expected a vector of 1536 dimension.` The final regression asserts that this mismatch returns a storage error instead of being silently treated as an empty semantic result.
