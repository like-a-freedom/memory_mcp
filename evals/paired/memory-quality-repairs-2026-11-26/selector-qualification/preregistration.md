# Autonomous temporal recall qualification

Source-first preparation, before model capture. This is a development screen of
one exact local configuration, with identical product arms. It estimates no
treatment effect and does not reopen stopped C1 or qualify C2.

## Frozen source policy

Four fresh synthetic families have independent source-derived answers:

| Family | Requested boundary | Source truth | Answer |
|---|---|---|---|
| Aster current | No historical selectors | Aster ledger | Cabinet-734 |
| Beryl valid time | valid_at 2026-05-15T10:00:00Z, current knowledge | May ledger; June ledger is later valid time | Drawer-268 |
| Cobalt checkpoint | valid_at 2026-05-01T10:00:00Z and public revision before correction ingestion | Original ledger at checkpoint | Vault-462 |
| Dahlia pre-ingestion | valid_at 2026-05-01T10:00:00Z, known_at 2020-01-01T00:00:00Z | No source existed at knowledge boundary | unknown, citations [] |

All source texts, values, and required/forbidden sources are registered in
source-first-truth.json before observing predictions. Every Dahlia seeded event
is forbidden. Its fixed independent control uses known_revision 0, an explicitly
different request representing the empty pre-ingestion knowledge state. That
control does not attest the agent used known_at; actual request fingerprints
must separately match the registered known_at and omit known_revision.

Preflight may resolve exact public revision numbers and product/runtime hashes;
it may not change sources, answers, or expected temporal semantics. The
*.preflight.json files are preparatory inputs, not final frozen registrations.
Final tasks.json, probes.json, plan.json and freeze-manifest.sha256 must exist
before any model call. No oracle is supplied to the model.

## Agent and product boundary

The existing Rust run-paired-chat-completions runner exposes the public
assemble_context tool and forwards the model's live chosen query and selectors.
It supplies the public checkpoint token in the Cobalt prompt. It does not replace
the model's request with a canonical request or inject an answer. The agent can
make one read and then must produce its own final JSON answer and source cites.

Both product arms use the same current native Apple binary and evidence rollout
stage. The existing public formation/probe adapter explicitly selects regex NER,
disables embeddings, and uses an isolated embedded store. This qualification is
limited to that deterministic lexical profile; it is not a default-Anno or
semantic-retrieval profile claim. Source formation is public ingest + extract.

Use direct loopback MLX only, temperature zero, thinking disabled, one tool call,
three turns, 384 output tokens, 4096 context tokens, 25 seconds per trial. No
remote model, download, LM Studio runtime, or additional candidate tuning.

## Gates and stopping

Each capture contains four families and both identical product arms. Require,
for each arm independently, 4/4 exact task answers with required citations,
4/4 agent evidence outcomes (including the expected empty Dahlia read), 4/4
independent probes, zero hard violations and zero resource exits.

Additionally inspect every actual read: exactly one observed successful public
read per trial, nonblank model-selected query, all ten registered effective
selector comparisons true, requested/returned valid_at true where registered,
and requested/returned revision cutoff true for Cobalt. Current recall must omit
historical axes. Default/unrelated filters are registered too; equivalent
normalization follows the runner's existing selector validator. Full raw public
synthetic traces are retained so query nonemptiness is independently reviewable.

The existing offline qualifier remains candidate-scoped. Its eligibility result
alone is insufficient for this screen: an explicit per-arm audit must meet all
the above gates. The qualifier's registered read contract now rejects missing
reads, selector mismatches, valid-time/cutoff mismatches and noncompliant or
unobserved registered snapshot retries. This cohort registers no retries.

No invented returned-known_at metadata check is claimed. Requested known_at is
fingerprinted; the returned valid time, empty result and absence of all forbidden
current sources establish the observable pre-ingestion behavior.

Run capture 02 only if capture 01 passes every gate for both arms. Require both
independent captures to pass before calling this exact workflow eligible.
Stop at the first failure; retain failures unchanged and report NO-GO. Model work
is bounded to five minutes total, with no exposed-case retry or tuning. A pass
would permit a separately frozen study, without establishing general stability,
statistical benefit, temporal benchmark performance, or C2 eligibility.

## Gate regression evidence

Observed RED:
`rtk cargo test -p eval-harness --lib registered_temporal_read_cannot_qualify_from_answer_and_probe_alone`
failed on the explicit assertion that a correct answer cannot prove the
requested temporal read happened: four answers and fixed probes passed, but
the registered agent read was unobserved.

GREEN after the minimal gate:
`rtk cargo test -p eval-harness --lib paired_agent::independent::tests`
passed 14 tests, one opt-in test ignored. Public runner-to-qualifier scenarios
also prove matching captured known_at qualifies and an incorrect captured
known_at rejects the same otherwise correct answers/probes. Ordinary legacy
tasks without read expectations retain their existing gate semantics.
