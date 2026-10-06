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

**Filtering stays bespoke.** The documented `RUST_LOG` semantics (`prefix=level`,
most-specific wins, bare-level default) are preserved by reusing
`is_event_enabled` pre-emit, not replaced by `EnvFilter`. A `Targets` layer only
silences third-party targets (default `WARN`), so foreign noise stays quiet and
`RUST_LOG` keeps meaning exactly what it meant.

**The dynamic payload is not modelled as `tracing` fields.** `tracing` fields
are static per callsite; our events are arbitrary maps. The map therefore
travels as one `&str` field and the formatter flattens it, reusing the existing
value-rendering (`value_to_string`, `quote_if_needed`, `render_duration`).

*Rejected — no dependency; only reformat `StdoutLogger`.* That is strictly
simpler for the formatting outcome alone, but it leaves the bespoke writer/
ANSI/TTY plumbing in place and does not unlock two near-term wins: capturing
third-party output and spans that would retire the scattered
`StdoutLogger::from_env()` calls (a service-locator smell). If those follow-ups
are not pursued, `tracing` should be dropped rather than half-adopted.

*Rejected — a full rewrite of the ~190 producers onto `tracing` macros now.*
It changes `RUST_LOG`'s selector from `op` to module target and touches ~72
files for no user-visible gain in this change. It is a follow-up (phases 2–3).

## Consequences

- Logs keep going to **stderr** (never stdout — MCP stdio framing) or the
  `MEMORY_LOG_FILE` sink; the sink is resolved per write, so an install after
  the subscriber still takes effect.
- New env: `MEMORY_LOG_FORMAT=text|json`, `MEMORY_LOG_COLOR=auto|always|never`;
  `NO_COLOR` and non-TTY disable colour; a file sink is always uncoloured.
- `op=` and `req=` remain whitespace-separated tokens, so existing integration
  tests and `grep`/`awk` keep working.
- No new fields are logged; the bounded/no-PII guarantees of `http/logging.rs`
  are unchanged. ADR-0048's log/metric separation is unchanged.
- Third-party levels are not configurable in this change; `RUST_LOG` does not
  raise them (they were discarded before, so this is not a regression).
- `log_warn_dedup` had no callers and is removed.
