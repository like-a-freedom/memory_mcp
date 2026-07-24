# ADR-0018: Interactive Reembed with Progress, Cancellation, and Continue-on-Error

> Status: Accepted (2026-07-24)
> Related: ADR-0016 (public surface freeze — reembed is CLI-only, not an MCP tool)

## Context

The `memory_mcp reembed` command rewrites all fact embeddings after an embedding
provider switch. Prior to this ADR, the command:

1. Logged structured events to stderr, but with no live progress bar — the
   process appeared to "go silent" and users could not tell if it had started.
2. Failed fast on the first fact error, stopping the entire run even for
   transient remote-provider failures.
3. Had no Ctrl+C handling — interrupting the process left the HNSW index
   dropped and embedding states in "rebuilding" with no clear recovery path.
4. Only logged ETA as a raw `eta_seconds` integer inside a JSON-ish log line.

After switching providers, operators reported confusion: "the app went to the
background and it's unclear whether reembed started or what the status is."

## Decision

Transform `reembed` into an interactive, observable, and resilient maintenance
command with four changes:

### 1. Live TTY progress bar via `indicatif`

When stderr is a TTY, show a live progress bar:
- Percentage, processed/total, ETA in human-readable format, facts/sec
- Success/failure counters (`✓1230 ✗10`)
- Namespace label in the bar prefix
- Spinner during service initialization (model loading)
- Redraw throttled to 10 Hz (`stderr_with_hz(10)`)

When stderr is not a TTY (pipes, CI, scripts), degrade to the existing
structured log events plus two new init-phase events
(`reembed.init_started`, `reembed.init_completed`).

### 2. Graceful Ctrl+C with `CancellationToken`

Register a `tokio::signal::ctrl_c()` handler that cancels a
`CancellationToken`. The fact-processing loop checks `is_cancelled()` after
each fact. On interrupt:

- Finish the current fact (no mid-rewrite abort).
- Persist job state with status `"interrupted"`.
- Show: `⏹ Interrupted at 1240/3000 (41%). Resume with 'memory_mcp reembed'.`
- Exit code 130 (standard SIGINT convention).
- On next `reembed` run, resume from the last cursor; show a resume hint.

### 3. Continue-on-error with quota

Instead of fail-fast, continue processing after a fact error:

- Record the failure, advance the cursor past the failed fact.
- Default quota: 10% of total facts (minimum 10).
- `--max-failures N` CLI flag overrides (0 = fail-fast, legacy behavior).
- If quota exceeded → status `"failed"`, exit code 1.
- If all processed with some failures → status `"completed_with_errors"`,
  exit code 0.
- Persist `failed_fact_ids` per namespace in job state for `--retry-failed`.

New job statuses: `running`, `completed`, `completed_with_errors`, `failed`,
`interrupted`.

### 4. `--retry-failed` flag

After a `completed_with_errors` or `failed` run, `--retry-failed` processes
only the facts in `failed_fact_ids`. If all retries succeed, status becomes
`completed`.

### 5. Final summary in stdout

After the progress bar clears, print a compact summary to stdout:

```
✓ Reembed completed (with errors)

  Total:       3000 facts
  Processed:   3000 (2990 succeeded, 10 failed)
  Duration:    135.2s
  Speed:       22 facts/sec

  10 facts failed. Re-run with --retry-failed to retry only failures.
```

## Consequences

- **New dependency:** `indicatif = "0.17"` — widely used, minimal, no transitive
  bloat. Already compatible with the existing `console` crate ecosystem.
- **Public CLI surface change:** `Command::Reembed` becomes
  `Command::Reembed(ReembedArgs)` with two optional flags. This is a
  backward-compatible change — `memory_mcp reembed` with no flags still works.
- **No MCP tool surface change:** reembed remains CLI-only. ADR-0016 public
  surface freeze is respected.
- **Job state schema extended:** `namespace_progress[ns]` now includes
  `failed_fact_ids` array. Old job records without this field are handled
  gracefully (defaults to empty array).
- **New statuses:** `interrupted` and `completed_with_errors` are persisted in
  the job record. The startup embedding-state check already treats any
  non-`ready` state as "semantic retrieval disabled", so these are safe.
