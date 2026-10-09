# A3 admission policy GREEN

The admission policy now derives query evidence, preserves explicit references and resolved entity links, validates access and temporal visibility, rejects invalid cosine values, and admits candidates only through browse, exact reference, entity, direct lexical, or above-threshold semantic evidence. The policy is invoked after retrieval-channel fusion and before selection. Graph-only admission remains unavailable until graph anchor provenance is implemented.

Validation:

- `rtk cargo test -p memory_mcp --locked --lib admission` — 17 passed.
- `rtk cargo test -p memory_mcp --locked --features eval-support --test assemble_context_semantic_quality` — 6 passed against an isolated SurrealDB database with production migrations.

The semantic paraphrase integration case uses controlled vectors and verifies only component wiring and the production `assemble_context` path. It is not a corpus-backed quality acceptance result.
