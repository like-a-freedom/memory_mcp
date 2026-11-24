# Historical read before two late corrections

A public MCP case records an inspection, an unrelated but similarly
named neighboring structure, a later inspection correction, and a
contradictory forum report. All four records have the same valid date.
The stored revision is captured before the two corrections are written.
The closed oracle requires only the original inspection, with its
current role at that snapshot, and prohibits the three other sources.

The native Apple Silicon run returned one item, `ledger-original`,
with value `27 metres` and role `current`. The decision was supported,
and the response correctly reported partial completeness. This case
characterizes the earlier snapshot and leaves the disposition after
both corrections for another probe.

## Reproduction

Build the native product and `memory-eval` with locked dependencies.
Run each capture in a fresh, absolute artifact directory.

```sh
MEMORY_DECISIONS_SERVER_BIN="$binary" \
MEMORY_DECISIONS_ARTIFACT_ROOT="$capture" \
MEMORY_DECISIONS_PRODUCT_REVISION="$revision" \
target/debug/memory-eval run \
  --profile evals/profiles/memory_decisions_multiple_corrections.json \
  --artifact "$capture/run.json"
```

The scored lane is `evidence_enabled_temporal`. A low runtime or a
passing summary on another lane cannot substitute for its source, role
and disposition checks.

The incumbent and both candidate captures passed. All three had the same
scored observation and zero failures. The two candidate indexes also
passed `compare-decision-replays`. This is a new regression control.
