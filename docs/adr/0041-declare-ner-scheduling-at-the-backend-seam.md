# ADR-0041: Declare NER Scheduling at the Backend Seam

## Status

Accepted (implemented 2026-08-19)

## Context

Entity extraction currently decides whether work belongs on Tokio's blocking
pool by matching `provider_name()` strings in
`service/episode/entity_extraction.rs`. Adding or renaming a backend therefore
requires editing an unrelated orchestration module, and a backend can be
registered without an explicit scheduling decision. The `anno-onnx` backend
also has a mutex-protected session but does not honor its configured maximum
concurrency through the shared `InferenceGate` abstraction used by the native
GLiNER backends.

Scheduling is a runtime concern of the backend implementation:

- lightweight deterministic extraction can run inline;
- synchronous CPU/Metal inference must not block the async runtime;
- asynchronous or injected I/O should remain on the async path;
- heavyweight inference needs an explicit concurrency gate independent of
  model loading and idle retention.

## Decision

Define a closed `NerScheduling` policy at the extractor seam:

- `Inline` — invoke extraction on the current async task;
- `BlockingPool` — invoke extraction through Tokio's blocking pool.

Every `EntityExtractor` implementation must declare `scheduling()`. Every
configuration-selectable backend also declares the same policy in its registry
entry. The episode extraction orchestration branches only on that declaration;
it never matches backend names or provider strings.

The built-in policies are:

| Backend | Scheduling | Concurrency |
|---|---|---|
| `anno` | `Inline` | backend-local behavior |
| `regex` | `Inline` | backend-local behavior |
| injected `llm` | `Inline` | caller-owned async transport |
| `gliner` | `BlockingPool` | shared `InferenceGate` |
| `anno-onnx` | `BlockingPool` | shared `InferenceGate` |
| `sauerkraut-lfm2.5-gliner` | `BlockingPool` | shared `InferenceGate` |

`anno-onnx` receives the configured `NER_MAX_CONCURRENCY` and acquires an
`InferenceGate` permit for each extraction. Its existing model-session idle
retention remains backend-owned; the gate controls inference concurrency and
is not a model-loading lock.

## Consequences

- Adding a backend requires one explicit scheduling declaration beside its
  builder and implementation, and registry tests can detect omissions.
- The episode orchestration module depends on a stable capability rather than
  backend names, improving locality and making scheduling behavior testable
  with a fake extractor.
- A backend that performs synchronous work but declares `Inline` can still
  block the runtime; the declaration is an explicit backend contract and must
  be covered by its backend tests.
- `anno-onnx` now honors its configured concurrency instead of serializing or
  ignoring the setting accidentally.
- Scheduling and model lifecycle remain separate: blocking-pool placement and
  inference permits do not decide when model artifacts are loaded or unloaded.

## Related decisions

- ADR-0029: registry of pluggable NER backends;
- ADR-0035: GLiNER lazy load with idle unload;
- ADR-0036: unified NER extractor selection and model lifecycle.
