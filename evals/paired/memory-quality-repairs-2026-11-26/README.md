# Memory quality repairs: retained evidence

Local native Apple Silicon, 2026-11-26. The consolidated interpretation,
limitations and action plan are in
[the repair report](../../../docs/evals/MEMORY_QUALITY_REPAIRS_2026-11-26.md).

| Record | Result and scope |
| --- | --- |
| [Entity admission](../../results/extraction-entity-repair-2026-11-26/README.md) | Same reviewed v2 labels before/after: 5/9 → 9/9, FP7 → FP0, TP13/FN0 unchanged. |
| [Source-linked packing](../../longmemeval_v2/results/2026-11-26-stage-diagnosis/README.md) | Two isolated real source states: complete packed spans0/2 → 2/2. No new full-history LongMemEval answer score. |
| [Reader v1](closed-reader/README.md) | Failed registration6/12: required a record prefix absent from delivered fact content; no repeat. |
| [Reader v2](closed-reader-source-span-v2/README.md) | Strict11/12: one shortened raw source ID; no repeat. |
| [Reader source handles v3](closed-reader-source-handles-v3/README.md) | Planned12/12+12/12, exposed v2 source bodies, changed citation transport; not an independent source confirmation. |
| [Original autonomous execution](selector-qualification/decision.json) | First capture8/8; global300s deadline stopped second preparation, no completed second-capture agent trial. Incomplete qualification retained. |
| [Repaired autonomous execution](selector-qualification-session-reuse/README.md) | Fewer session startups; same exposed tasks and quality truth, preregistered600s execution erratum. Both captures8/8; actual model queries/selectors, both-arm v3 audit and Rust two-capture qualifier pass. C2 remains ineligible. |
| Native product gate | `native-gate-final/current-regression.json`:8/8, no regressions, invalid0, unobserved0. `final-runner-regrade.json` independently regrades the same raw receipts with the final runner. |
| Invalid first native gate | `native-gate/`: mutable debug binary changed during capture; identity guard rejected it. Its inner suite success is not accepted evidence. |

`reader-offline-regrade.json` verifies every retained reader score using its
archived capture script. All failed registrations remain failed. Model assets
were hashed before and after inference (`model-assets-before.json`,
`model-assets-after.json`); all13 files are unchanged. The standalone offline
MLX server has been stopped. Its model directory happens to be under a local
cache with an LM Studio name; no LM Studio process or API is required.

Product/runner executables stay under ignored `target/evals/`; captures pin
their digests. Current source was based on6b1035a7 with the recorded working-tree
repairs, not an unmodified HEAD build. Fresh destinations and the immutable
binary copies prevented concurrent builds from changing accepted captures.

Disposable reader stores were removed after retaining all raw logical
requests/responses (`scratch-cleanup.json`). They are not required for offline
grading. Historical scripts are preserved because they define the old scorer;
active diagnostics live one directory above this evidence directory. Native
package/lint checks and source identities are recorded in `verification.json`.
