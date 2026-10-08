# Logging

How this service logs, what an operator can turn up or down, and how to follow
one request across the stream.

Related decisions: [ADR-0078][adr-0078] (the line format), [ADR-0079][adr-0079]
(the level policy), [ADR-0080][adr-0080] (one request identity), [ADR-0081][adr-0081]
(operation names). This page is the operator's contract for all four.

[adr-0078]: ../adr/0078-human-readable-logging-on-tracing.md
[adr-0079]: ../adr/0079-log-level-policy.md
[adr-0080]: ../adr/0080-one-request-identity.md
[adr-0081]: ../adr/0081-operation-naming-contract.md

## Where events go

Events are written to **stderr** — or to `MEMORY_LOG_FILE` when set — one event
per line. Under the stdio transport stdout carries MCP protocol framing, so
nothing is ever written there. The sink, format and colour are read once, when
the subscriber is installed at startup.

## Levels (ADR-0079)

| Level | Use |
|---|---|
| `ERROR` | A failed unit of work, a risk to integrity, a failure after retries, loss of readiness. |
| `WARN` | Handled but unexpected; degraded; a client refusal (4xx class). |
| `INFO` | A business event or a state change. |
| `DEBUG` | Diagnostics, off by default. |
| `TRACE` | Fine-grained flow, opted into by name. Anything repetitive is a metric. |

A failure's level is decided by its error class, not its call site:
`MemoryError::log_level()` maps storage, transience, configuration and
unavailability to `ERROR`, and refusals (not-found, validation, conflict,
budget, model-not-ready, auth) to `WARN`.

## Operation names (ADR-0081)

Every event carries `op = <namespace>` or `op = <namespace>.<event>`. The
namespace is one of a pinned, closed registry (`OP_NAMESPACES` in
`crates/memory-mcp/src/logging.rs`):

```
assemble_context  app_command  cache  config  control  db  embedding  explain
extract  fs_watch  graph  http  ingest  invalidate  knowledge  lifecycle  main
ner  oidc  open_app  reembed  resolve  schema  triple_extraction
```

A bare name (`extract`) is the namespace with an implicit default event. The
first segment is what `RUST_LOG=<namespace>=<level>` selects, so a name outside
the registry is a filter key no directive can reach. A source lint
(`every_emitted_operation_names_a_registered_namespace`) asserts every emitted
`op` names a registered namespace.

### Renamed keys

Eight names were conformed to the registry. A saved query or dashboard naming an
old spelling must be updated:

| Old | New |
|---|---|
| `startup` | `main.config` |
| `startup.version_probe_failed` | `main.version_probe_failed` |
| `startup.versions` | `main.versions` |
| `model_loader` | `embedding.model_loader` |
| `dimension_override_mismatch` | `embedding.dimension_override_mismatch` |
| `cosine_similarity.dimension_mismatch` | `embedding.cosine_similarity_mismatch` |
| `extract_from_episode.start` | `extract.from_episode.start` |
| `extract_from_episode.done` | `extract.from_episode.done` |

## Fields

- `op` — the operation name (above).
- `error` — the one name for error text (the failure's `Display`). Free-text
  error detail is never called `detail` or `message`; `reason` remains only as a
  bounded machine label for a closed branch (for example `http.auth.rejected`
  carries `reason=rate_limited`).
- `request_id` — the correlation id (below), rendered by the formatter.
- `duration_ms` — top-level only, never nested under `args`.
- `args`, `result` — the operation's inputs and outcome.

Never log secrets, credentials, key material, tokens, passwords, or raw memory
content.

## Configuration

| Variable | Meaning |
|---|---|
| `RUST_LOG` | Level and per-subsystem directives. A bare level sets the default (`info`); `prefix=level` sets a subsystem. The most specific prefix wins. |
| `MEMORY_LOG_FORMAT` | `text` (default, human-readable, one line per event) or `json` (NDJSON, one object per event). |
| `MEMORY_LOG_COLOR` | `always`, `never`, or anything else for auto (colour only on a colour-capable terminal, and never for a file sink). `NO_COLOR` set and non-empty disables colour. |
| `MEMORY_LOG_TARGETS` | Third-party `tracing` targets to raise, comma-separated `target=level`. Quiet by default; can never drop this service's own events. |
| `MEMORY_LOG_FILE` | Append the stream to this file instead of stderr. A blank value means "unset". |

`RUST_LOG` examples:

```
RUST_LOG=info                       # default
RUST_LOG=extract=debug              # one subsystem
RUST_LOG=http=error,info            # quiet the HTTP surface, keep the rest
RUST_LOG=fs_watch=trace,http=debug  # two subsystems, the most specific rule wins
```

## Following one request

Every unit of work carries a correlation id. In the HTTP profile it is the
request's `x-request-id` (honoured when supplied, minted otherwise) and is
advertised back on the response; in the stdio profile it is a per-call
`req_NNNN`. The formatter stamps the ambient id onto every event that does not
carry its own, so each line of one request — the access log line, a tool line, a
downstream warning — reads with the same `req=` and follows with a single
filter:

```bash
grep 'req=<id>' memory-mcp.log
```

The HTTP response echoes the id on `x-request-id`, and a control-plane error
envelope carries it as `correlation_id` — a client quoting either names the same
value the log line shows.

## How events are emitted

Three facades, one pipeline:

- `StdoutLogger::log(event, level)` (with `StdoutLogger::from_env()`) — the
  per-instance logger, filtered by its own `RUST_LOG` directives.
- `memory_mcp::logging::emit(event, level)` — a process-level facade for
  producers that hold no logger (a slice of fields, a bound map).
- native `tracing` (`tracing::warn!(target: LOG_TARGET, op = …, …)`) — rendered
  by the same formatter.

## Live smoke check

```bash
# HTTP profile: run at the default level, ingest once, and confirm the tool
# line and the access line share a `req`.
RUST_LOG=info cargo run --features streamable-http --bin memory_mcp_http
# … POST a tools/call ingest; `op=ingest.done` and `op=http.request` carry the
#   same `req=<uuid>`.

# stdio profile: a direct call mints its own id.
cargo run -- serve
# … ingest; the line carries `req=req_NNNN`.
```
