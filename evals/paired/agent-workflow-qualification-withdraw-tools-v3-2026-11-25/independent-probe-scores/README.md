# Independent probe scores

These are the per-case outputs of `memory-eval score-independent-probe` for
both arms and both captures. The scorer used the frozen `probes.json`
inputs/oracles and the matching observation from each independent envelope.
All 16 scored observations returned `evidence_success: true` (4/4 per arm and
capture). The candidate scores are also summarized by
[`qualification.json`](../qualification.json); these files preserve the
independently replayable per-probe scorer outputs for both arms.

`SHA256SUMS` covers the JSON score outputs. The repeated offline qualifier
report is preserved at the cohort root in `qualification.json`.
