# Awaited access phase diagnostic

One separately registered local diagnostic of `922a0c56` on a disposable full-store clone. The evaluator-only phase markers preserve business ordering, awaited writes and existing error semantics.

`access_heat.started` was observed at 27.247315 seconds. The cooperative timeout partial receipt arrived at 31.627812 seconds without `access_heat.settled`; no successful heat SQL terminal was observed. At least the observed 4.38-second interval was spent awaiting heat work before cancellation. This locates the delayed phase; it does not partition the engine serialization, index comparison or KV costs, establish eventual completion time, or turn timeout into success.

The process reached the 45-second external boundary (45.136992 seconds; exit 101 after kill race). No context was delivered. The source snapshot stayed unchanged. Raw 9.95 MB trace remains outside git with its digest in `observation.json`; the compact receipt preserves the phase proof.

The locked SurrealDB source encodes the complete fact post-image and compares old/new index values. Unchanged index values skip index mutation: no evidence here proves rebuilding an unchanged FTS index. A separately registered side-record prototype and explicit migration approval are required before changing storage.
