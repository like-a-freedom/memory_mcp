# Pre-capture audit

Observed at 2026-11-23T20:52:19Z. The first runner invocation stopped while deserializing `plan.json`: `shared_identity.revision` is not part of the v2 plan schema. The process did not reach a model turn, and no `runs/run-01` artifact directory was created.

Removed only that unsupported, unused field. Task families and task text, prompts, sampling, endpoint/model alias, budgets, product/evaluator hashes, and evaluation gates remain unchanged. The input manifest was regenerated and is checked again before capture. This is a harness preflight correction, not model-quality evidence.
