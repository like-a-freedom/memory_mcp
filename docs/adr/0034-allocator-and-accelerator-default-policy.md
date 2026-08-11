# ADR-0034: Allocator and Apple BLAS Default Policy

## Status

Accepted — current policy. The fresh allocator result keeps mimalloc opt-in,
and the 2026-08-06 Accelerate A/B does not pass the strict no-degradation gate;
Accelerate remains an explicit experimental feature with no release target.

## Date

2026-08-06

## Context

Memory MCP has two unrelated performance switches:

- `mimalloc` installs `mimalloc::MiMalloc` as the global allocator in the
  `memory_mcp` server binary (`crates/memory-mcp/src/main.rs`). It does not
  change entity extraction, model weights, tokenization, thresholds, or output
  ordering. ADR-0032 records a production macOS observation of approximately
  5.1–5.2 GB of retained empty `MALLOC_SMALL` regions after allocation churn.
- `accelerate` enables Candle's optional `accelerate-src` dependency and routes
  eligible CPU matrix operations through Apple's Accelerate framework. Candle's
  pinned `candle-core` manifest declares this as an optional dependency; the
  generic `gemm` path remains the portable fallback.

The package contains both a library and a server binary. A global allocator in
`main.rs` affects the server executable only. It does not affect library
consumers, integration-test executables, or eval-harness Criterion processes.
Therefore, a normal Criterion run cannot prove that the production allocator
reduces RSS.

Cargo features are additive and unified through the dependency graph. The
package's `default` feature set is not target-aware. Making `accelerate` a
package default would request an Apple-specific backend from Linux, Windows,
and library consumers. A feature named `accelerate` can remain opt-in for
Apple-specific benchmarks and experiments while portable builds use the generic
CPU backend.

The v5 evaluation report is the quality baseline. Its hard requirements are
zero failed/invalid cases and unchanged retrieval, extraction, claim, lifecycle,
and end-to-end gates. Its benchmark numbers are comparison anchors, not proof
that a new allocator is beneficial.

The fresh controlled matrix in `docs/performance/MEMORY_PROFILE.md` is stronger
than the earlier plan's proposed synthetic probe: each variant used a fresh
release server process, a fresh embedded database, the committed GLiNER fixture,
the MCP stdio path, one successful extraction, and the same idle-unload setting.
Default allocator plus idle unload reached 430 MB RSS / 277 MB physical
footprint. Mimalloc plus idle unload reached 2,556 MB RSS / 190 MB physical
footprint. Mimalloc therefore improved one macOS accounting signal but made the
user-visible RSS signal 2,126 MB higher than the default-allocator comparison.
The result is not suitable for a default-on allocator.

The separate Apple Silicon Accelerate A/B used three Criterion runs for the
warm `ner_cpu` and `pipeline` benchmarks. Direct GLiNER inference improved
substantially (three-run median: -67.17% single-window and -56.32% multi-window),
but `default_service_extract_warm` was slower (+2.93% in the three-run median;
the final paired rerun was still slower with separated Criterion intervals),
and pipeline ingest/context were also slower in the comparison. Under the
strict no-degradation policy, the outcome is `REJECTED_REGRESSION`: no
production optimization or `serve-release-macos` target is added. The raw local
logs are retained under `target/evals/accelerate-ab/`; the summarized result is
in `docs/performance/NER_PERFORMANCE.md`.

## Decision

1. Keep `accelerate` separate from `mimalloc`.
2. Keep `accelerate` out of `default`. Keep it available only for explicit Apple
   Silicon benchmarks and experiments; use Candle's portable CPU backend for
   production and portable builds.
3. Keep `mimalloc` feature-gated and opt-in. The fresh evidence does not support
   default promotion, but the feature remains available to the production server
   for workload-specific experiments and must not be moved into the library's
   business logic.
4. Keep `mimalloc` out of `default` for the current production workload. The
   fresh A/B result fails the promotion requirement because post-unload RSS is
   2,556 MB with mimalloc versus 430 MB with the default allocator. The lower
   190 MB physical footprint does not offset the RSS regression for users and
   tools that monitor RSS.
5. Keep `GLINER_IDLE_UNLOAD_SECS` default semantics unchanged (`0` remains the
   compatibility default; the variable was later renamed to `NER_IDLE_UNLOAD_SECS`
   by ADR-0036, with identical semantics). Document `30` seconds as a measured
   recommendation for infrequent local extraction workloads, not as a universal
   runtime default; the current matrix covers one fresh-process workload, not
   every long-lived concurrency pattern.
6. Do not add the planned synthetic allocator probe or eval-harness global
   allocator solely to decide this default. `MEMORY_PROFILE.md` already records
   a real release-server MCP-stdio comparison with stronger workload fidelity.
   Re-open that work only for a new workload hypothesis or a new allocator.
7. Record the completed Accelerate A/B as `REJECTED_REGRESSION` for the current
   benchmark surface. Accelerate improves direct GLiNER inference, but the
   service and pipeline regressions mean it is not safe to expose as a release
   optimization under the no-degradation policy. Keep the feature available for
   explicit experiments only; do not add `serve-release-macos`, and do not add it
   to the portable package default. Reconsider only with a clean benchmark seam
   and a new all-surface no-regression result.
8. Treat the existing v5 quality gates as hard gates, not a performance budget.
   A performance improvement never justifies lower labels, thresholds, model
   precision, candidate limits, retrieval limits, or fallback behavior.

## Consequences

### Positive

- The current default is supported by a fresh, production-like comparison for
  the reported local-use case.
- Idle unload, rather than mimalloc, is the effective lever for both RSS and
  physical footprint in that measured case.
- Apple acceleration remains available without making cross-platform builds
  depend on an Apple framework.
- The existing quality contract remains unchanged.

### Negative

- Mimalloc remains an opt-in escape hatch and may still be useful for a
  different allocator/OS/workload combination.
- Accelerate remains an explicit experimental feature; the current A/B is not a
  release recommendation because it failed the all-surface no-degradation gate.
- The one-extraction matrix does not prove behavior for every long-lived
  concurrency pattern; future default changes require a new measured report.

## Alternatives considered

1. **Enable both features in `default` immediately.** Rejected: it would make
   the cross-platform contract unclear and would not provide evidence that the
   allocator improves this workload.
2. **Use only the existing Criterion benches to judge mimalloc.** Rejected:
   the allocator static in `memory-mcp/src/main.rs` is not linked into those
   benchmark executables.
3. **Put the global allocator in the `memory_mcp` library.** Rejected: that
   would impose a process-wide allocator on every downstream binary and can
   conflict with another executable's allocator choice.
4. **Make `accelerate` unconditional in the workspace dependency.** Rejected:
   the Candle backend is an optional Apple-specific dependency and the package
   must retain a portable fallback for non-Apple targets.
5. **Enable mimalloc secure mode.** Rejected for this optimization: the
   mimalloc documentation reports an approximately 10% performance penalty, and
   secure mode is unrelated to the RSS-retention problem being measured.

## Verification record

The implementation plan is
`docs/superpowers/plans/2026-08-06-allocator-accelerator-defaults.md`.
The external contracts consulted for this decision were the pinned Candle
`candle-core/Cargo.toml`, the `mimalloc` docs on docs.rs, and Cargo's official
feature/platform-dependency documentation, retrieved through Keenable.

## Final verification (2026-08-06)

- Quality gate: **PASS** — formatting, metadata, compilation, strict clippy,
  workspace tests, feature tests, model-backed parity, and all three eval profiles
  passed.
- Allocator policy: keep the default allocator; keep `mimalloc` opt-in.
- Idle-unload policy: keep the code default at `0`; recommend `30` seconds for
  the measured infrequent-use case.
- Accelerate policy: keep the Apple-specific feature explicit; keep it out of the
  package default and do not add a release target.
- Accelerate performance outcome: **`REJECTED_REGRESSION`** for the current
  all-surface comparison; direct GLiNER speedups do not override service and
  pipeline regressions under the no-degradation rule.
