# Product-default baseline before harness simplification

Captured locally on Darwin arm64 on 2026-11-26, using the unchanged production
source at `ece2b831f678c1ea62f46b1488da7d9165151593`. The build command, binary
hashes, grader hashes and complete replay identity are in `capture-manifest.json`.
This is development/regression evidence, not an agent-utility or production-data
qualification. Independent review accepted this baseline, and equivalence checks plus the final
native gate passed. See the [review and current gate](../harness-simplification-current-2026-11-26/README.md).

One default-profile run used the existing full diagnostic policy, public MCP
formation and isolated stores, regex extraction and disabled embeddings. It took
172.603 seconds. The source, corpus, oracle and scoring channel were frozen before
changing diagnostic execution. The harness binary includes the deadline and
observation changes under development; its binary hash is recorded separately
from the unchanged production source revision.

| Case | Primary evidence | Primary decision/probe | Primary failure reason | Temporal probe |
|---|---|---|---|---|
| absent-subject | PASS | PASS | — | PASS |
| similar-name | PASS | FAIL | decision_unobservable | PASS |
| prior-knowledge | PASS | PASS | — | PASS |
| peer-conflict | PASS | PASS | — | PASS |
| office-entry | PASS | FAIL | decision_unobservable | PASS |
| single-current | PASS | FAIL | decision_unobservable | PASS |
| unverified-correction | PASS | PASS | — | PASS |
| valid-boundary | PASS | PASS | — | PASS |

Result: **5/8 primary probes pass; 3 quality failures; 0 invalid**. All eight
primary evidence checks and temporal probes pass. The evidence-stage diagnostic
is a separate execution and does not replace the primary result.

`receipts/index.json` and its eight sibling raw receipts preserve request,
response, history, source bindings, snapshot, oracle and score evidence. Loader
verification must recompute scores and bind them to the registered corpus.
A scoped no-regression result must continue to report the three current failures;
it cannot be described as all-cases-pass. No candidate may refresh this baseline
automatically or replace its primary channel with a more convenient diagnostic.
