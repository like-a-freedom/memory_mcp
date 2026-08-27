# Background Classic GLiNER Artifact Refresh Design

**Status:** Approved direction; strengthened after adversarial review  
**Date:** 2026-08-27

## Problem

With `NER_EXTRACTOR=urchade/gliner_multi-v2.1`, service construction currently resolves Hugging Face HEAD and may download a checkpoint larger than 1 GB before the stdio MCP transport starts. Zed starts its `initialize` deadline when it launches the process, so network lookup and acquisition can exhaust that deadline.

The artifact store also currently conflates three distinct states: downloaded files, runtime-compatible checkpoint, and active known-good checkpoint. Moving the existing `prepare()` call into a background task without correcting that state model would mark an untested revision `RuntimeRegressionVerified` and could bypass smoke validation after restart.

## Goal

Make Classic GLiNER remote revision lookup and artifact acquisition independent of MCP readiness while preserving runtime validation, rollback, provenance, cancellation cleanup, and one immutable extractor fingerprint per process lifetime.

## Scope

This design applies only to `NER_EXTRACTOR=urchade/gliner_multi-v2.1` (Classic GLiNER).

The startup behavior of Anno, Regex, Anno ONNX, and `VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER` is unchanged. Shared artifact APIs must therefore remain compatible with those backends until they receive separate designs.

## Non-goals

- Hot-swapping an extractor in a running process.
- Making all background maintenance start after MCP readiness.
- Changing the public MCP tool surface.
- Supporting multiple processes against one embedded RocksDB directory.
- Adding dependencies or changing `Cargo.toml`.
- Redirecting production artifact downloads through an undocumented endpoint.

## Required State Machine

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
  | next-start runtime construction or smoke inference fails
  v
incompatible
```

A candidate is never returned by the ordinary known-good selector and never carries `RuntimeRegressionVerified`. Background refresh may create or update a candidate, but it may not promote it.

The persisted state schema must distinguish candidates from known-good and incompatible revisions. Existing schema-version-1 state files must remain readable and interpret their non-incompatible records as known-good for backward compatibility. New writes use schema version 2.

## Startup Contract

Startup performs no Hugging Face request, download, artifact lease wait, or remote retry for Classic GLiNER.

### Candidate exists

1. Read local state and verify candidate completeness and content identity.
2. Construct Classic GLiNER from the candidate.
3. Run the existing smoke inference.
4. On success, atomically promote candidate to known-good and use it for this process.
5. On failure, atomically mark candidate incompatible and attempt the previous verified known-good revision.
6. If fallback construction also fails or no fallback exists, start with an unavailable Classic GLiNER extractor.

Candidate runtime validation is allowed to delay readiness because it is local model initialization, not remote refresh. This design guarantees independence from remote acquisition, not zero-cost model loading.

### No candidate, known-good exists

1. Verify required files and compare the recomputed artifact identity with persisted identity.
2. Construct the extractor from that known-good checkpoint.
3. Start MCP without network access.

### No usable local checkpoint

Start MCP with `UnavailableEntityExtractor`. Only extraction is degraded. The extractor preserves configured selector, labels, threshold, runtime version, and the backend-declared `BlockingPool` scheduling. Artifact identity, revision, validation status, and effective device remain absent.

`extract` returns:

```json
{
  "kind": "model_not_ready",
  "retryable": false,
  "restart_required": true,
  "activation": "next_restart",
  "explanation": "The configured Classic GLiNER checkpoint is not available locally.",
  "guidance": "Wait for background preparation to complete, then restart Memory MCP."
}
```

Same-process retries are intentionally marked non-retryable because the active extractor is immutable.

## Typed Local Inspection

Local artifact inspection must not collapse missing, corrupt, and operational failures into one error. Introduce explicit outcomes:

```rust
pub enum LocalCheckpointIssue {
    Incomplete { revision: String },
    IdentityMismatch { revision: String },
    MalformedState { summary: String },
    UnsupportedStateVersion { found: u8 },
}

pub struct LocalCheckpointSet {
    pub candidate: Option<PreparedCheckpoint>,
    pub known_good: Option<PreparedCheckpoint>,
    pub issue: Option<LocalCheckpointIssue>,
}
```

Missing state is normal and yields an empty set. Incomplete files, identity mismatch, malformed state, and unsupported state schema degrade Classic GLiNER and are logged without paths or secrets. Permission errors and an unreadable cache directory remain startup-fatal because background refresh cannot safely repair an inaccessible store.

Every returned checkpoint must pass:

1. all required paths exist and are non-zero;
2. recomputed artifact identity equals persisted identity;
3. persisted role is appropriate for the requested selector;
4. persisted validation status is preserved rather than manufactured.

## Artifact Store Operations

Keep the existing network-enabled `prepare()` for Anno ONNX and VAGO LFM2 GLiNER.

Add Classic-GLiNER-specific operations:

```rust
impl NerArtifactStore {
    pub fn inspect_local(
        &self,
        spec: &NerArtifactSpec,
    ) -> Result<LocalCheckpointSet, MemoryError>;

    pub async fn refresh_candidate(
        &self,
        spec: &NerArtifactSpec,
        cancellation: CancellationToken,
    ) -> Result<CandidateRefreshOutcome, MemoryError>;

    pub fn promote_candidate(
        &self,
        spec: &NerArtifactSpec,
        revision: &str,
    ) -> Result<PreparedCheckpoint, MemoryError>;

    pub fn reject_candidate(
        &self,
        spec: &NerArtifactSpec,
        revision: &str,
        reason: &str,
    ) -> Result<Option<PreparedCheckpoint>, MemoryError>;
}
```

`CandidateRefreshOutcome` distinguishes `UpToDate`, `CandidateReady`, and `SuppressedIncompatible`. Refresh reuses existing resolver, lease, staging, checksum, identity, and retention mechanics but writes candidate state instead of known-good activation.

## Cancellation Safety

Dropping a refresh future is insufficient because current error cleanup does not run when a future is cancelled. Add RAII cleanup guards:

- a staging-directory guard removes its directory on drop unless committed;
- a partial-file guard removes `.part` on drop unless atomically renamed;
- the existing lease guard continues releasing the lease on drop.

Pass a `CancellationToken` through candidate refresh and fetch loops. Check cancellation while waiting for leases, before each required file, while reading response chunks, and before the atomic candidate commit.

Hashing and blocking filesystem work execute through `spawn_blocking`. Cancellation cannot forcibly stop an already-running blocking task, so the shutdown guarantee is:

- network, lease, and inter-file waits observe cancellation promptly;
- no new blocking phase starts after cancellation;
- a currently running bounded local hash/atomic commit may finish before join;
- partial and staging data are cleaned on cancellation or failure.

Do not promise a universal 500 ms shutdown bound during an active filesystem commit.

## Background Runtime

`NerArtifactRefreshRuntime` owns one Classic GLiNER refresh task, a cancellation token, and join handles. It starts only after `server.serve((stdin, stdout)).await` succeeds and `main.running` is logged.

The worker performs one refresh attempt per process lifetime. Success persists a candidate for the next restart. Failure is logged and does not terminate MCP. It never constructs `GlinerEntityExtractor`, runs smoke inference, or mutates the active extractor.

Only this NER refresh moves after MCP readiness. Existing claim, lifecycle, filesystem, and embedding workers retain their current startup order.

## Observability

Two channels remain separate:

### Structured logger

- `ner.local_checkpoint.unavailable`
- `ner.artifact_refresh.started`
- `ner.artifact_refresh.up_to_date`
- `ner.artifact_refresh.candidate_ready` with `activation=next_restart`
- `ner.artifact_refresh.failed` with `activation=unchanged`
- `ner.artifact_refresh.stopped`

### ModelProgressSink

Existing schema-version-1 resolve/download byte progress continues on stderr or the configured sink. It never writes to MCP stdout.

Refresh event classification uses the typed local inspection result, not `active_revision()`, because that accessor suppresses read errors and does not verify identity.

## Test Seam

Default tests must not download the real 1.15 GB model or depend on public Hugging Face.

Unit and integration tests inject `RevisionResolver` and `ArtifactFetcher` directly into `NerArtifactStore` and `NerArtifactRefreshRuntime`. The default suite separates:

1. process-level MCP readiness and unavailable-error behavior, which requires no successful real model load;
2. artifact lifecycle candidate persistence/promotion/rejection tests using fake bytes;
3. ignored real-fixture tests proving candidate construction and smoke validation after restart.

For the process-level blocked-refresh test, add an `eval-support`-gated artifact-source override accepted only when the binary is compiled with that existing feature. Tests run that target with `--features eval-support`. Normal builds contain no endpoint override behavior.

## Testing Requirements

- Local inspection invokes neither resolver nor fetcher.
- Local inspection rejects removed, zero-byte, replaced, and identity-mismatched files.
- Schema v1 migrates in memory to v2 semantics and remains readable.
- Background refresh writes candidate, not known-good.
- Candidate is never selected without next-start smoke validation.
- Candidate success promotes it and preserves previous known-good for rollback.
- Candidate failure marks it incompatible and falls back.
- Unavailable extractor scheduling matches Classic GLiNER registry scheduling.
- Unavailable fingerprint preserves configured labels and threshold.
- Blocked refresh does not delay MCP `initialize`.
- `extract` returns `retryable=false`, `restart_required=true` while unavailable.
- Cancellation removes lease, staging directory, and `.part` file during lease wait, response wait, mid-stream download, and multi-file acquisition.
- Structured lifecycle events and model progress are tested as separate channels.
- No stdout contamination occurs.
- Existing Anno ONNX and VAGO LFM2 GLiNER behavior and tests remain unchanged.

## Documentation and Decision Record

Add an ADR documenting:

- Classic GLiNER-only scope;
- candidate versus known-good state;
- local runtime validation on next start;
- rejection of hot swap and silent fallback;
- cancellation limits around blocking filesystem work;
- unchanged behavior of other model-backed extractors.

Update README with first-install behavior, `candidate_ready` restart guidance, and download-free Anno/Regex alternatives.

## Acceptance Criteria

- Classic GLiNER MCP initialization performs no Hugging Face operation before transport readiness.
- Remote lookup/download/lease wait cannot consume the MCP initialization deadline.
- No unvalidated candidate is labeled runtime-verified or known-good.
- The active extractor and fingerprint never change during a process lifetime.
- A candidate activates only after successful next-start smoke validation.
- Previous known-good remains available for rollback.
- Missing or recoverably corrupt local artifacts degrade only extraction.
- Permission and inaccessible-cache failures remain explicit startup errors.
- Refresh cancellation leaves no lease, staging, or `.part` debris.
- Refresh failure does not terminate MCP.
- No new MCP tools, request fields, dependencies, or storage partitions are introduced.
