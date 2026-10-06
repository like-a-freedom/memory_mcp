# ADR-0078: Human-readable logging on `tracing`

- Status: accepted — 2026-10-06
- Relates to: [ADR-0048](0048-bounded-runtime-observability.md) (logs vs.
  metrics separator), `docs/BACKLOG.md` (log format entry)

The service logged through a bespoke logger (`StdoutLogger`) that rendered a
`HashMap<String, Value>` per event as `[ts] LEVEL req=… op=… k=v …`. The format
was bracketed and millisecond-truncated, carried `req=-` when no request id
existed, inlined nested payloads (`args={…}`), and had no colour, no
machine-readable mode, and no TTY/`NO_COLOR` handling. Third-party `tracing`
output (the embedded `surrealdb`, `tokio`, `reqwest`) was discarded because no
subscriber was installed. The backlog asked to make the logs more human
readable, with `surrealdb`'s `Full` output as the reference.

## Decision

**Adopt `tracing` + `tracing-subscriber` as the process-wide logging facility;
keep `StdoutLogger` as the single emission choke point.**

A global subscriber is installed once at startup (and lazily, idempotently, on
first use so library/test consumers need no changes). Producers keep emitting
`HashMap` events through `StdoutLogger::log`; the logger filters with the
existing `RUST_LOG` op-prefix rules *before* emitting, then dispatches one
`tracing` event carrying the serialised payload in a single field. A custom
`FormatEvent` renders the human line (default) or NDJSON (`MEMORY_LOG_FORMAT`),
with TTY-aware colour honouring `NO_COLOR`.

**Filtering stays bespoke and has one source of truth.** The documented
`RUST_LOG` semantics (`prefix=level`, most-specific wins, bare-level default)
live in `Directives` and are applied in two places that read the same rules:
the per-instance logger filters a recorded map event before emission, and the
subscriber's `OpFilter` filters any event that reached `tracing` directly. That
second path is what lets a native `tracing` producer — `tracing::warn!(target:
"memory_mcp", op = "…", …)`, or a span — obey the same dial instead of
bypassing it. A `Targets` layer only silences third-party targets (default
`WARN`), so foreign noise stays quiet and `RUST_LOG` keeps meaning exactly what
it meant.

**Dynamic producers keep the map facade.** A field set built at runtime (a
slice of `(key, value)`, a bound map) cannot be a `tracing` field macro, so the
`op` travels as a first-class field beside the serialised payload and the map
facade `logging::emit(event, level)` emits without constructing a logger. The
deep modules that used to build a logger per call (`StdoutLogger::from_env()`)
now call `emit`, and the contexts that forced a level with
`StdoutLogger::new("trace"/"warn")` do too: with the subscriber's filter in
place that forced level was already redundant, so one facade governs them and
`RUST_LOG` is the dial.

**The dynamic payload is not modelled as `tracing` fields.** `tracing` fields
are static per callsite; our events are arbitrary maps. The map therefore
travels as one `&str` field and the formatter flattens it, reusing the existing
value-rendering (`value_to_string`, `quote_if_needed`, `render_duration`). A
native event (an `op`, no payload) is rendered from its own fields in the same
shape.

*Rejected — no dependency; only reformat `StdoutLogger`.* That is strictly
simpler for the formatting outcome alone, but it leaves the bespoke writer/
ANSI/TTY plumbing in place and does not unlock the two wins above: capturing
third-party output and a native event path that retires the scattered
`StdoutLogger::from_env()` calls.

*Rejected — a full rewrite of the ~190 producers onto `tracing` macros now.*
Most producers build dynamic field sets that `tracing` macros cannot express, so
this is not mechanical; it would change `RUST_LOG`'s selector for map events and
touch ~72 files for no user-visible gain. The native path is additive: any
static-field producer can move to it, and the rest keep the facade.

## Consequences

- Logs keep going to **stderr** (never stdout — MCP stdio framing) or the
  `MEMORY_LOG_FILE` sink; the sink is resolved per write, so an install after
  the subscriber still takes effect.
- New env: `MEMORY_LOG_FORMAT=text|json`, `MEMORY_LOG_COLOR=auto|always|never`;
  `NO_COLOR` and non-TTY disable colour; a file sink is always uncoloured.
- Third-party `tracing` output is capped at `warn` and is not selected by
  `RUST_LOG` (which keeps selecting this service's `op` prefixes). An operator
  raises a dependency's level with `MEMORY_LOG_TARGETS`, a separate
  comma-separated `target=level` list.
- `op=` and `req=` remain whitespace-separated tokens, so existing integration
  tests and `grep`/`awk` keep working.
- No new fields are logged; the bounded/no-PII guarantees of `http/logging.rs`
  are unchanged. ADR-0048's log/metric separation is unchanged.
- A producer with static fields may emit a native `tracing` event
  (`target: "memory_mcp"`, `op = "…"`); `RUST_LOG` selects it and the formatter
  renders it in the same shape. A producer with a dynamic field set calls
  `logging::emit`. The `log_warn`/`RequestWarning` facade in `http/logging.rs`
  keeps its logger parameter because tests pass a bound-directive logger there.
- Third-party output was discarded before, so capturing it at `warn` by default
  is additive; `MEMORY_LOG_TARGETS` opts a dependency up when needed.
- `log_warn_dedup` had no callers and is removed.
