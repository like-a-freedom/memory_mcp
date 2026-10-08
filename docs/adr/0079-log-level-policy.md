# ADR-0079: Log-level policy

- Status: accepted — 2026-10-08
- Relates to: [ADR-0078](0078-human-readable-logging-on-tracing.md) (format and
  the `RUST_LOG` dial), [ADR-0048](0048-bounded-runtime-observability.md) (logs
  vs. metrics), [ADR-0080](0080-one-request-identity.md),
  [ADR-0081](0081-operation-naming-contract.md), `docs/operations/LOGGING.md`

`logging.rs` renders every record, but each callsite chose its own level. The
result was a contract nobody could state: a failed `ingest` was `WARN` while a
failed HTTP job was `ERROR`, and a client's malformed argument was
indistinguishable from a storage outage. A level is an operational promise —
on-call reads `ERROR` as *act* and `WARN` as *watch* — so it has to be written
down and enforced, not inferred per callsite.

## Decision

Five levels, aligned with OpenTelemetry's severity ranges and the SRE rule that
`ERROR` marks an actionable failure, not a routine refusal.

- **ERROR** — a unit of work the service was asked to perform did not complete,
  or a durability/consistency guarantee is at risk. Storage failures, failed
  external calls after bounded retries, background-job failures, failed audit or
  lifecycle writes, quota/policy enforcement failures, loss of readiness.
- **WARN** — something unexpected was handled or degraded, or the caller's
  request was refused. Validation errors, not-found, invalid input, `Conflict`,
  `BudgetExhausted`/rate-limit refusals, `Auth` denials, a degraded fallback, a
  truncated or oversized-input refusal.
- **INFO** — a business event or state change: process lifecycle, one line per
  tool-invocation outcome, scan/backfill/reembed start and completion,
  provisioning transitions, configuration changes.
- **DEBUG** — diagnostics normally off: stage timings, cache decisions, provider
  metadata, scheduler ticks with no work.
- **TRACE** — fine-grained flow an operator opts into by name; anything
  repetitive belongs in a metric.

**A failed operation is classified by its cause, not its callsite.** The shared
error vocabulary answers once: `MemoryError::Storage`, `Transient`,
`ConfigMissing`, `ConfigInvalid` and `Unavailable` are `ERROR`; `NotFound`,
`Validation`, `Conflict`, `BudgetExhausted`, `ModelNotReady` and `Auth` are
`WARN`. A callsite asks the error (`MemoryError::log_level()`) rather than
hardcoding a level, so a client mistake and a storage outage cannot share one.

## Consequences

- Error events that carry a `MemoryError` stop being uniformly `WARN`. This
  covers the six canonical tool error events (`ingest.error` …
  `assemble_context.error`) and the two app-tool ones (`open_app.error`,
  `app_command.error`). A query keyed on `level=warn` sees fewer error lines; one
  keyed on the `op` is unaffected.
- Events that carry no `MemoryError` keep the level their callsite chose. This
  policy changes only the callsites that classify a shared error.
- `FATAL` is unused: a crash is the process exiting non-zero, which the
  environment observes without an in-process record. Revisit only if a crash
  needs a record the exit code cannot carry.
- For an event with no error, the level is still a judgement. This ADR is the
  tie-breaker; `docs/operations/LOGGING.md` lists the questions a reviewer asks
  of a new event.
- Metrics stay the surface for repetitive counts and rates (ADR-0048). A level is
  not a substitute for a counter, and this policy moves no volume onto `INFO`.
