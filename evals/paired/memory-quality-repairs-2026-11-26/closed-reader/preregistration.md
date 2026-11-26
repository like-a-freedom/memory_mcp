# Closed evidence-use qualification

Frozen source-first development cases: complete three-element set, adjacency in an ordered list, count of nine explicit actions, and contradicted approval premise. All questions, answers and exact source lines are in local-config.json before any model response is inspected. These are new synthetic sources; no archived answer has been changed.

Three controls use the same reader instruction: registered query through actual memory, model-selected query through actual memory, and the complete source as a sufficient fixed reply. Every answer must pass value/set, source ID, abstention and exact full-line quote independently. No partial score admits an arm. Extra or duplicate list elements fail; reordered sets pass.

First capture schedules 12 reader and four query-selection calls. A second fresh-store/process capture of the same 12 arms occurs only if every first arm passes. Qualification requires all 24 reader outcomes; failure/invalid stops without selecting a third attempt. Two planned captures share model/cache and fixed arm order, so they are repeatability checks rather than independent population samples. Source labels remain evaluator-only; the reader receives the question and public evidence, never the expected answer.

Only native Apple Silicon, an already-cached model and a standalone loopback Chat Completions endpoint are used. Runtime/model selection is supplied configuration; LM Studio is not required. Shared 300-second model-call eligibility and per-call 20-second socket I/O timeout are not an absolute whole-capture deadline; MCP response waits are separately bounded. This screen establishes only the finite reader workflow, not a treatment effect or C2 admission.
