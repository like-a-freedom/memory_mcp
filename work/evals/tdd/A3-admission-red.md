# A3 admission policy RED

The deliberate fail-closed `build_query_evidence` / `evaluate_candidate` stub was replayed before restoring the implementation. Its Git blob was `687b583ef2ce4678ed345f1d868b6eaf42884ec0`.

Command: `rtk proxy cargo test -p memory_mcp --locked --lib admission`

Expected behavioral RED: 11 admission assertions fail while the six negative cases that expect rejection pass. The failures cover malformed semantic scores, reference conflict, exact technical and Unicode references, browse, entity evidence, count intent, paraphrase, scope exclusion, access/time checks, and short exact-query matching. This was a compiling behavioral RED, not a compiler/setup failure.

GREEN commands:

- `rtk cargo test -p memory_mcp --locked --lib admission` — 17 passed.
- `rtk cargo test -p memory_mcp --locked --features eval-support --test assemble_context_semantic_quality` — 6 passed.

The component paraphrase test initially failed because its controlled provider mapped the raw query instead of the normalized embedding input used by the production assembly path. The fixture was corrected to the observed normalized input and then passed, including on the pre-admission semantic selection behavior. That setup failure is not counted as an A3 product RED.
