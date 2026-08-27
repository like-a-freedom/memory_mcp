# ADR-0051: Background Classic GLiNER Artifact Refresh

## Status

Accepted (implemented 2026-08-27; see `docs/superpowers/plans/2026-08-27-background-gliner-refresh.md`).

## Context

With `NER_EXTRACTOR=urchade/gliner_multi-v2.1`, service construction
historically resolved Hugging Face HEAD and may download a checkpoint
larger than 1 GB before the stdio MCP transport started. Zed begins its
`initialize` deadline when it launches the process, so network lookup and
acquisition can exhaust that deadline.

The artifact store also conflates three distinct states: downloaded files,
runtime-compatible checkpoint, and active known-good checkpoint. Moving
the existing `prepare()` call into a background task without correcting
the state model would mark an untested revision `RuntimeRegressionVerified`
and could bypass smoke validation after restart.

## Decision

### Classic GLiNER-only scope

This decision applies only to `NER_EXTRACTOR=urchade/gliner_multi-v2.1`
(ADR-0036). The startup behavior of Anno, Regex, Anno ONNX, and
`VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER` is unchanged. Shared artifact
APIs remain compatible with those backends.

### Three durable roles

A Classic GLiNER revision has one of these durable roles:

```text
absent
  |
  | background download + static file/checksum/identity verification
  v
candidate
  |
  | next-start runtime construction + smoke inference succeeds
  v
known_good

candidate
  |
  | next-start construction or smoke inference fails
  v
incompatible
```

A candidate is never returned by the ordinary known-good selector and
never carries `RuntimeRegressionVerified`. Background refresh may create
or update a candidate, but it may not promote it. Promotion happens only
on next-start.

The persisted state schema distinguishes candidates from known-good and
incompatible revisions. Existing schema-version-1 state files remain
readable and migrate in memory to v2 semantics: records with
`incompatible: None` become `KnownGood`; records with `incompatible: Some(_)`
become `Incompatible`. New writes use schema version 2.

### Local-only startup

Classic GLiNER startup performs no Hugging Face request, download, lease
wait, or remote retry. It uses `NerArtifactStore::inspect_local` to read
the persisted state, verify candidate/known-good completeness and
identity, and surface typed local issues. Operational I/O errors
(permission, unreadable directory) remain startup-fatal because background
refresh cannot safely repair an inaccessible store.

The candidate path runs `probe_and_install`; success atomically promotes
the candidate to `KnownGood` with `RuntimeRegressionVerified`. A failed
candidate probe is persisted as `Incompatible`, the artifact directory is
removed, and the previous verified known-good revision is loaded. When no
usable local checkpoint exists, the service starts with
`UnavailableEntityExtractor` — extraction alone is degraded; the active
fingerprint, scheduling, selector, labels, threshold, and runtime version
are preserved so historical projections stay attributable.

### Cancellation-safe download

Cancellation reaches lease waits, HTTP send, HTTP chunk reads, the
between-file boundary, and the pre-commit check via
`tokio_util::sync::CancellationToken`. The shutdown guarantee is:

- network, lease, and inter-file waits observe cancellation promptly;
- no new blocking phase starts after cancellation;
- a currently running bounded local hash or atomic commit may finish
  before join;
- partial `.part` files, the staging directory, and the lease are cleaned
  on cancellation or failure through RAII guards (`PartialFileGuard`,
  `StagingDirGuard`, and the existing `Lease` drop).

No universal 500 ms shutdown bound is promised during an active filesystem
commit.

### Background runtime

`NerArtifactRefreshRuntime` owns one Classic GLiNER refresh task and is
spawned only after `server.serve((stdin, stdout))` succeeds and `main.running`
is logged. The worker performs one refresh attempt per process lifetime,
emits structured events, and exits. It never constructs
`GlinerEntityExtractor`, never runs smoke inference, and never mutates the
active extractor. Cancellation during shutdown joins the task.

Only the Classic GLiNER refresh moves after MCP readiness. Existing claim,
lifecycle, filesystem, and embedding workers retain their startup order.

### Typed MCP error semantics

`MemoryError::ModelNotReady` maps to an MCP error with exactly:

```json
{
  "kind": "model_not_ready",
  "retryable": false,
  "restart_required": true,
  "activation": "next_restart",
  "explanation": "...",
  "guidance": "Wait for background preparation to complete, then restart Memory MCP."
}
```

It is not `retryable` because the active extractor is immutable for the
process lifetime. Empty custom labels do NOT silently return success while
the extractor is unavailable.

## Consequences

- Classic GLiNER MCP initialization performs no Hugging Face operation
  before transport readiness.
- Remote lookup, download, and lease wait cannot consume the MCP
  `initialize` deadline.
- No unvalidated candidate is labeled `runtime_regression_verified` or
  `known_good`.
- The active extractor and fingerprint never change during a process
  lifetime.
- A candidate activates only after successful next-start smoke validation.
- The previous known-good revision remains available for rollback.
- Missing or recoverably corrupt local artifacts degrade only extraction.
- Permission and inaccessible-cache failures remain explicit startup
  errors.
- Refresh cancellation leaves no lease, staging, or `.part` debris.
- Refresh failure does not terminate MCP.
- No new MCP tools, request fields, dependencies, or storage partitions
  are introduced.

## Rejected alternatives

- **Synchronous pre-initialize refresh**: would still consume the `initialize`
  deadline on slow networks and break the existing public contract.
- **Hot-swap of the active extractor**: would change fingerprints and
  historical projections mid-process, violating the "one immutable
  extractor per process" invariant.
- **Treating download as runtime verification**: would mark an
  unvalidated revision `RuntimeRegressionVerified` and let it
  silently replace a known-good one.
- **Globally changing all model-backed backends**: would re-shape Anno
  ONNX and VAGO LFM2 GLiNER startup with no observed benefit; their
  behavior is intentionally preserved.

## Validation

- Schema v1 → v2 migration tests cover known-good and incompatible
  records; the wire format is fully documented in
  `service/model_artifacts/state.rs`.
- `inspect_local` tests prove no resolver, fetcher, lease, or remote
  operation is invoked from the local startup path.
- Cancellation tests cover response wait, mid-stream download, and
  between-file acquisition phases; they assert no `.part`, staging, or
  lease debris after cancellation.
- Blocked-HTTP process test proves `initialize` returns before the
  background resolver HEAD completes.
- A real-fixture ignored test in `tests/ner_gliner_real_activation.rs`
  exercises the production constructor and smoke probe against the local
  1.15 GB `urchade/gliner_multi-v2.1` checkpoint.
- The mandatory `cargo clippy --workspace --all-targets --features
  fs-watch,mcp-apps --locked -- -D warnings` gate remains green.

## Related decisions

- ADR-0029: registry of pluggable NER backends.
- ADR-0035: native GLiNER lazy loading and idle unloading.
- ADR-0036: unified NER extractor selection and model lifecycle.
- ADR-0041: declare NER scheduling at the backend seam.
- ADR-0045: layer boundaries, error home, and service imports.
- ADR-0046: two-tier background task lifecycle.
