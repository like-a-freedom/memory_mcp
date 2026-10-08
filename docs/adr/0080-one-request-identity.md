# ADR-0080: One request id across a unit of work

- Status: accepted — 2026-10-08
- Relates to: [ADR-0078](0078-human-readable-logging-on-tracing.md) (whose
  request-span correlation this supersedes), [ADR-0079](0079-log-level-policy.md),
  [ADR-0038](0038-one-active-namespace-per-server.md)

The control plane already treats one request id as a property to preserve: the
`x-request-id` header, the error envelope's `correlation_id`, and the audit
`RequestContext` are documented to name the same value, and `ApiError` carries
the id on the error instead of minting a second one at render time (see
`control/error.rs`, `control/local_admin.rs`). The runtime does not. The access
log writes `req=<uuid>`; the tool layer mints an unrelated `req_NNNN` from a
process-global counter; a service-layer line under a tool call carries no id at
all. In HTTP one request therefore produces two unjoinable families of lines, and
a retrieval or embedding line joins neither. Correlation is what makes a log
usable during an incident; it cannot be decided per subsystem.

## Decision

**One correlation id per inbound unit of work, carried in an ambient task-local
context and adopted by every line the unit of work emits.**

- `logging::correlation` owns a `tokio::task_local!` slot and exposes
  `current() -> Option<String>` and `async fn scope(id, future)`. It is the one
  mechanism. `tracing` spans are no longer the carrier, and the `http_request`
  span is removed.
- Producers do not thread the id. The formatter
  (`MemoryFormat::format_event`) injects `request_id` from the ambient context
  into any event that lacks one — our own events and foreign ones alike. An
  explicit field wins over the ambient value.
- The boundary opens the scope:
  - **HTTP** adopts an inbound `x-request-id` (validated as a UUID; a non-UUID is
    discarded and regenerated, which closes the log-injection vector) or mints
    one, exactly as `request_log` already resolves it, and wraps the rest of the
    request in `correlation::scope`. This is the same value the header, the error
    envelope's `correlation_id`, and the audit trail already share.
  - **Tools** adopt an ambient id when one exists — in HTTP, the request's — and
    otherwise mint a per-call `req_NNNN`. `ToolEvent` no longer carries a request
    id; `tools/request_id.rs` becomes the local fallback generator.
- Correlation is per task. A `tokio::spawn` does not inherit it; a spawn that
  must correlate re-enters `correlation::scope` with the id it captured.
  Best-effort background work that belongs to no request runs uncorrelated.

## Consequences

- In HTTP a tool's `ingest.done` and the access log's completed request carry the
  same `req`, and a dependency warning raised inside the request joins them.
- In stdio each tool call still gets a distinct `req_NNNN`, preserved
  byte-for-byte so existing `grep`/`awk` pipelines and string assertions hold.
- Foreign events render from the ambient context instead of `span_fields`, so
  `span_fields` and the `http_request` span are deleted, and the ADR-0078 tests
  that exercised the span are replaced by task-local equivalents.
- A producer may still attach a `request_id` explicitly — preflight and the HTTP
  access log do. Injection is additive and never overwrites.
- The id is UUID-validated before storage, so a client cannot inject a newline or
  an unbounded value through it.
- A future OpenTelemetry exporter maps this id to `trace_id`; `span_id` waits on
  exported spans. The data model is OTel-compatible, so the step is additive.
