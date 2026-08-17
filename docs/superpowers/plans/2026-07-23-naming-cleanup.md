# Naming and Dead-Code Suppression Cleanup

> Status: ✅ Executed (verified 2026-08-17 audit) — `STORED_EMBEDDING_SAMPLE_SIZE` renamed; false `#[allow(dead_code)]` removed from policy.rs
> Audit candidate: 4 (retire misleading legacy markers)

## Context

The audit found code that misrepresents itself. This is the smallest and safest of the four candidates — mechanical cleanup with no architectural impact.

## Plan

### Step 1 — Rename `LEGACY_EMBEDDING_SAMPLE_SIZE`

**File:** `src/service/startup.rs:9`

Despite the name, this is not legacy code. It is the sample size for
`sample_stored_embedding_dimensions()`, called from `resolve_embedding_startup()`
to infer the dimension of already-stored embeddings when no explicit
`embedding_state` record exists. It is active startup logic called on every
server start.

**Action:** rename to `STORED_EMBEDDING_SAMPLE_SIZE` (or inline the literal
`16` at the single call site).

### Step 2 — Remove false `#[allow(dead_code)]` from `policy.rs`

**File:** `src/service/agent_memory/policy.rs`

Five items carry `#[allow(dead_code)]` but are **actually called** via
`capture.rs` (`LifecycleCapture::execute` calls `CapturePolicy::evaluate`):

- `CapturePolicy` (struct)
- `CapturePolicy::evaluate`
- `trust_for_source`
- `is_recognized_capture_signal`
- `accepted_reason`

**Action:** remove the `#[allow(dead_code)]` annotations. The compiler will
verify they are live.

**Dependency:** do this **after** the lifecycle-wiring plan (Candidate 2)
confirms the recall-side items in `recall.rs` are also wired — otherwise
removing the policy.rs annotations while recall.rs still has accurate
`#[allow(dead_code)]` creates an inconsistent picture. Actually, policy.rs and
recall.rs are independent: policy.rs items are called via capture.rs (already
wired), recall.rs items are called via the future `LifecycleRecall`. So Step 2
can proceed independently.

### Step 3 — Leave these alone

| Item | Reason |
|------|--------|
| `reject_legacy_context_item_aliases` (`tools/parsers.rs`) | Real backward-compat guard for the frozen public surface. Accurate name. |
| `TrustClass::LegacyUnknown` (`models/memory_event.rs`) | Domain concept from the trust model (ADR-0016). Not technical debt. |
| `SourceKind::LegacyUnknown` (`models/memory_event.rs`) | Domain concept. Not technical debt. |
| `#[allow(dead_code)]` in `recall.rs` (10 items) | Accurate until `LifecycleRecall` is wired (Candidate 2). Remove as part of that plan. |
| `#[allow(dead_code)] tool_router` (`mcp/handlers.rs:78`) | Field stored for future use; verify with the team but leave for now. |
| `#[allow(dead_code)] from_raw` (`models/ids.rs`) | Constructor on a validated ID type; may be used by future constructors. Leave. |

## ADR needed?

No — mechanical cleanup, no decisions to record.
