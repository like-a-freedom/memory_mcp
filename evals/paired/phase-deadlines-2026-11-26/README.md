# Agent-phase deadlines — 26 November 2026

The phase budget now bounds the real owned stdio MCP process, including a blocked
write/read, rather than only rejecting a late result after it returns. An existing
earlier whole-run deadline cannot be extended by a phase. Recall fixed-reply and
public synthetic trace wrappers forward the same absolute deadline.

An interrupted call retains a linked failed-read observation and raw synthetic
call, timeout error class and exactly one terminal timeout event. Completed reads
before it remain available. The paired backend still performs shutdown/reaping;
an expected killed-process exit no longer erases the timed-out observation.
Unrepresentable absolute deadlines return `InvalidConfig` before model/tool calls.

## Behavioral evidence

`attempts.json` distinguishes four observed behavioral REDs from invalid test
setup attempts. Raw logs are retained, including the early unused-import warning.
The final implementation passes strict clippy without warnings.

The final native capture uses real loopback HTTP and a controlled executable MCP
fixture, with fixed HTTP responses and no model inference. A 500 ms phase budget
interrupts a call whose ordinary transport timeout is 2,000 ms. Two scenarios:

1. First read blocks, including through fixed-reply and synthetic-trace wrappers:
   no successful read, one retained failed call, owned process reaped.
2. First read succeeds and second blocks: the earlier successful source/response
   survives, with one retained failed call and owned process reaped.

Exact measured elapsed times, source/binary hashes and toolchain are in
`capture-identity.json`; stdout is `phase-final-green.log`. The test permits
bounded cleanup overhead (1,500 ms outer assertion), rather than requiring an
unrealistic exact scheduler boundary. These two observations prove the regression
controls, not latency percentiles, model reliability or a product speedup.

The captured real execution also passes through public
`run_paired_agent_evaluation`; both serialized artifact arms validate, and loading
the saved candidate preserves its receipts. This fixture replay tests artifact
compatibility only. It is not an independent paired experiment. A separate public
recall-backend test executes both arms with isolated actual child processes and
retains synthetic traces after cancellation. Existing non-timeout session-reuse
and transport tests remain in the full suite.

## Reproduce

```sh
cargo test --locked -p eval-harness --lib deadline_integration_tests -- --nocapture
cargo test --locked -p eval-harness --test recall_session_reuse
cargo test --locked -p eval-harness --test chat_completions_runner
cargo test --locked -p eval-harness --test memory_decisions_stdio
```

No API key, Codex runner, LM Studio or downloaded model is required. This delivery
was verified only natively on Apple Silicon. The controlled shell-process tests
are Unix-gated; they are not evidence of execution on Windows or Linux.

`MemoryToolSession::limit_deadline(Instant)` is now a required adapter method;
custom adapters must implement effective cancellation. There is no default
no-op that could silently bypass the contract. No persisted schema version,
dependency, production MCP tool or memory algorithm changed.

The bound begins at the agent phase, after preparing the store and schemas;
it does not establish a new whole-workflow budget for setup, formation or probes.
The owned direct child is killed and waited for; arbitrary external process trees
are outside this fixture's claim. Cooperative embedded-database timeouts still
need an external process boundary, as the separate
[full-history diagnosis](../../longmemeval_v2/results/2026-11-26-full-history/README.md)
demonstrates.

Full fresh harness/product tests, required workspace clippy, opt-in diagnostic
clippy and formatting results are recorded in `verification.json` with raw logs.
