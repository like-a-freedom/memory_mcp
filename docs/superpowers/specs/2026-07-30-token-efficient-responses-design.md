# Token-Efficient MCP Responses — Design Spec

**Status:** Implemented — validation recorded in ADR-0022
**Date:** 2026-07-30
**Scope:** Presentation-layer change on `assemble_context` and `explain` responses; no pipeline, ingestion, or storage changes.

## 1. Problem

`memory_mcp` serves LLM / AI-agent consumers exclusively. Every byte of every response is read into the agent's context window verbatim. The current responses are verbose in ways that add context overhead without improving decision quality:

- **`assemble_context`**: every `AssembledContextItem` carries a 120–160 character `rationale` string (e.g., `tier=direct fts=0.85 access_count=3 confidence=0.92 relevance=0.92 grounding=0.78 alignment=0.91 semantic=enabled …`) intended for human debugging. Agents don't parse this; they process `content`, `confidence`, and `retrieval_tier` directly.
- **`explain`**: every `ExplainItem` carries `quote` (identical to `content` in every observation), `episode_content` (the full source body, often kilobytes), and `graph_insights` (repeated identically across batch items).
- **Envelope fields**: `has_more: false` and `total_count: N` are emitted even when the list is complete — padding that adds no semantics.

The v5 benchmark run (`docs/evals/BENCHMARK_RUN_REPORT_2026-07-29-v5.md`) demonstrates quality at 17/17 gates passed, 363/363 cases passed across PR/Release/Nightly profiles. This quality gate is sacrosanct — the goal is to reduce context load **without** reducing the information available for decision-making.

### Measurable Target

Reduce JSON wire size for `assemble_context` and `explain` responses by targeting:
- Median `assemble_context` response ≤ 70% of current (verbose) byte count.
- Median `explain` response ≤ 80% of current (verbose) byte count.

Phase B measured a 39.5% mean byte reduction overall; ADR-0022 records the completed validation and the shipped defaults.

## 2. Approach

### 2.1 Chosen approach: `compact` parameter (default `true`)

Add a `compact: bool` parameter to `assemble_context` and `explain`. When `compact=true` (the default), the response omits redundant and debug-oriented fields at serialization time using `#[serde(skip_serializing_if = ...)]`. When `compact=false`, the current verbose response shape is preserved (debugging, audit, backward compatibility).

This is the approach termed "Approach 3" in the earlier exploration.

**Why not approach 1 (always-slim, no toggle):** breaks existing tests that assert on `rationale` content, blocks debugging, and removes the escape hatch for operators who need verbose output.

**Why not approach 2 (opt-in compact, default `false`):** defeats the purpose. LLM agents won't know to ask for compact mode. The context savings happen only when it's the default.

**Trade-off:** `compact=true` as default means existing test fixtures that assert on verbose rationale need updating. User explicitly accepted this trade-off ("it's managable") while the v5 bench gates remain the quality floor.

### 2.2 Field-level decisions

| Field | `compact=true` | `compact=false` | Rationale |
|-------|---------------|-----------------|-----------|
| `content` | **Kept** | Kept | Primary signal for LLM consumers |
| `quote` | **Skipped** | Kept | Duplicates `content` in every observed case |
| `rationale` | **Slimmed** to `tier=<tier>` | Kept (~120 char) | Agents need tier identification; raw scores are debug noise |
| `retrieval_tier` | **Kept** | Kept | Needed for `rationale` slim form |
| `confidence` | **Kept** | Kept | Decision-quality signal |
| `relevance` | **Kept** | Kept | Ranking signal |
| `grounding` | **Kept** | Kept | Fact grounding signal |
| `provenance` (AssembledContextItem) | **Kept** | Kept | Source traceability |
| `provenance` (ExplainItem) | **Kept** | Kept | Rich metadata; tests depend on `kind: hub_entity\|community` |
| `citation_context` | **Kept** | Kept | Source body excerpt for citation |
| `all_sources` (ExplainItem) | **Kept** (including `episode_content`) | Kept | Content body needed for cross-referencing across sources |
| `graph_insights` (ExplainItem) | **Kept** (full, on all items) | Kept | `tests/explain_provenance.rs:753-757` asserts batch equality |
| `has_more: false` | **Skipped** | Kept | `complete_list_compact` sets `has_more: None`; `skip_serializing_if = "Option::is_none"` already handles this |
| `total_count == len(result)` | **Skipped** | Kept | Redundant when list is complete |

### 2.3 What does NOT change

- **8-tool frozen surface:** `compact` is a parameter on existing tools, not a new tool.
- **Pipeline behavior:** Ingestion, extraction, NER, retrieval, and ranking logic is untouched. Only the final serialization changes.
- **PR/Release/Nightly eval profiles:** These profiles measure retrieval, extraction, and claim quality — not response size. They are untouched. The new `response-size` profile (Phase B) is additive and gate-free.
- **Schema contract:** Struct fields and JSON schemas are unchanged. `quote` and verbose `rationale` remain present in the schema; they are conditionally omitted at *serialization* time only (see §3.5), so `handlers.rs` schema tests stay green.
- **CLI surface:** A `--compact`/`--no-compact` flag is added to `assemble-context` and `explain` commands (default `true`, matching tools).

## 3. Architecture

### 3.1 Seam and depth

The **module** receiving `compact` is the response construction boundary:

```
┌──────────────────────────────────────────────────┐
│  ToolResponse<T> + AssembledContextItem /        │
│  ExplainItem (serde-time decisions)              │ ← compact lives here
├──────────────────────────────────────────────────┤
│  Capabilities (AssembleContextCapability,        │
│  ExplainCapability) — pass through               │
├──────────────────────────────────────────────────┤
│  Domain services (context, explanation, NER,     │
│  embedding, storage) — unchanged                 │
└──────────────────────────────────────────────────┘
```

This is a **presentation-layer concern**. The `compact` flag flows through:

```
params → request → capability → construction site → serialization
```

The tool handler sets a **scoped thread-local** before building items, and serde helpers (`skip_if_compact`, `serialize_rationale`) read it at the point of serialization inside the same async call stack. Domain services are not aware of the flag — only the tool handler and the serde helpers are.

### 3.2 Data flow

```
LLM Agent
  │  compact=true (default) or compact=false
  ▼
MCP Handler / CLI adapter
  │  params.compact → request.compact
  ▼
Capability (thin pass-through)
  │  request.compact
  ▼
Domain service (builds items normally)
  │  items with full rationale, quote, etc. -- NO compact knowledge
  ▼
Tool handler (assemble_context / explain)
  │  knows params.compact
  │  ┌─ set CompactGuard(compact) ───────────────────┐
  │  │  build ToolResponse                             │ guard
  │  │  serde_json::to_value(response) ──► Value       │ alive
  │  │  drop(_guard)                                   │
  │  └─ return Value ─────────────────────────────────┘
  ▼
serde_json::Value (pre-serialized JSON, compact fields already omitted)
  ▼
LLM Agent context window
```

The `Value` returned by the tool handler is already serialized — the caller (MCP handler or CLI) cannot trigger `skip_serializing_if` a second time without the guard being present. The serialisation happens inside the tool handler's async scope.

### 3.3 Parameter plumbing

**`AssembleContextParams`** (new field):
```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct AssembleContextParams {
    // ... existing fields ...
    /// Request compact (token-efficient) response. Defaults to true.
    #[serde(default = "crate::tools::parsers::default_compact")]
    pub compact: bool,
}

// In crates/memory-mcp/src/tools/parsers.rs:
pub(crate) fn default_compact() -> bool { true }
```

**`ExplainParams`** (new field):
```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ExplainParams {
    pub context_items: String,
    /// Request compact (token-efficient) response. Defaults to true.
    #[serde(default = "crate::tools::parsers::default_compact")]
    pub compact: bool,
}
```

**`AssembleContextRequest`** (new field):
```rust
pub struct AssembleContextRequest {
    // ... existing fields ...
    #[serde(default = "crate::tools::parsers::default_compact", skip_serializing_if = "is_default_true")]
    pub compact: bool,
}

fn is_default_true(b: &bool) -> bool { *b }
```

**`ExplainRequest`** (new field at the request level, not per-item):
```rust
pub struct ExplainRequest {
    pub context_pack: Vec<ExplainItem>,
    #[serde(default = "crate::tools::parsers::default_compact", skip_serializing_if = "is_default_true")]
    pub compact: bool,
}
```

`ExplainRequest` carries `compact` at the request level because `graph_insights` batching across items makes per-item `compact` nonsensical — the decision is all-or-nothing for the entire batch.

### 3.4 Struct changes

**`AssembledContextItem`** — no field removals, only `serde` skip annotations:

```rust
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct AssembledContextItem {
    pub fact_id: String,
    pub content: String,

    /// Skipped under compact=true (duplicates content).
    #[serde(default, skip_serializing_if = "crate::tools::compact::skip_if_compact")]
    pub quote: String,

    pub source_episode: String,
    // ... confidence, relevance, grounding unchanged ...

    /// Slimmed under compact=true to just "tier=<tier>".
    #[serde(serialize_with = "crate::tools::compact::serialize_rationale")]
    pub rationale: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieval_tier: Option<String>,
    // ...
}
```

The `serialize_rationale` custom serializer produces:
- `compact=true`: `"tier=direct"` (just the tier, no scores)
- `compact=false`: the full debug string unchanged

**`ExplainRequest` carries `compact` at the request level** because `graph_insights` batching across items makes per-item `compact` nonsensical — the decision is all-or-nothing for the entire batch.

**`ExplainItem`** — same pattern:

```rust
pub struct ExplainItem {
    pub fact_id: Option<String>,
    pub content: String,

    /// Skipped under compact=true.
    #[serde(default, skip_serializing_if = "crate::tools::compact::skip_if_compact")]
    pub quote: String,

    pub source_episode: String,
    // scope, t_ref, t_ingested unchanged
    pub provenance: serde_json::Value,
    pub citation_context: Option<String>,

    /// Kept fully populated (episode_content preserved).
    #[serde(default)]
    pub all_sources: Vec<ProvenanceSource>,

    /// Kept fully populated (batch equality contract tested at explain_provenance.rs:753-757).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_insights: Option<GraphInsights>,

    // fact_age_days, decayed_confidence, ingestion_method unchanged
}
```

**`ProvenanceSource`** — no changes. `episode_content` stays populated under both modes.

`ToolResponse<T>` — new constructor for compact mode:

```rust
impl<T> ToolResponse<T> {
    /// Builds a complete-list response with `compact=false`. Under compact,
    /// omits pagination metadata that would carry only default semantics.
    pub(crate) fn complete_list_compact(
        result: T,
        total_count: usize,
        guidance: impl Into<String>,
    ) -> Self {
        Self {
            status: "success".to_string(),
            result,
            guidance: Some(guidance.into()),
            has_more: None,        // compact: omit false (already skipped via Option::is_none)
            total_count: None,     // compact: omit when list is complete
            next_offset: None,
        }
    }
}
```

Note: `has_more: None` and `total_count: None` are already skipped by the existing `skip_serializing_if = "Option::is_none"` on `ToolResponse`. `complete_list_compact` is redundant in its *behavior* (produces the same wire shape), but it exists so the caller in each tool can choose between compact and full names without branching at every call site.

The existing `complete_list` constructor is preserved for backward compatibility when `compact=false`.

### 3.5 `skip_if_compact` mechanism

The `compact` flag needs to be accessible at serialization time. `serde` attribute helpers (`skip_serializing_if`, `serialize_with`) cannot see request-scoped state or sibling fields, so we use a scoped thread-local guarded by an RAII type. The guard lives only within the async tool-handler call and is dropped before the function returns -- the serialized `Value` is already safe by that point.

The struct fields are `pub(crate)` so the serde helpers in `models/request.rs` can reference them via the fully qualified path.

```rust
// crates/memory-mcp/src/tools/compact.rs
use std::cell::Cell;

thread_local! {
    static COMPACT_MODE: Cell<bool> = const { Cell::new(false) };
}

/// RAII guard: sets compact mode on entry, restores previous on drop.
/// Drop the guard AFTER the final serde serialization completes, not before.
/// Because the guard is stack-bound and the serializer runs on the same async task,
/// no two concurrent serializations interfere.
pub(crate) struct CompactGuard {
    prev: bool,
}

/// Enable or disable compact mode for the remainder of the current async scope.
/// The returned guard must be held alive until response serialization completes.
pub(crate) fn set_compact(compact: bool) -> CompactGuard {
    let prev = COMPACT_MODE.with(|c| c.replace(compact));
    CompactGuard { prev }
}

impl Drop for CompactGuard {
    fn drop(&mut self) {
        COMPACT_MODE.with(|c| c.set(self.prev));
    }
}

pub(crate) fn is_compact() -> bool {
    COMPACT_MODE.with(|c| c.get())
}

/// serde `skip_serializing_if` fn — skips the field when compact mode is on.
pub(crate) fn skip_if_compact<T>(_value: &T) -> bool {
    is_compact()
}

/// Custom serializer for `rationale`. Under compact mode, emits only the
/// leading `tier=<tier>` token; otherwise passes the string through.
/// Must be `pub(crate)` because it's referenced from `models/request.rs`.
pub(crate) fn serialize_rationale<S: serde::Serializer>(
    rationale: &str,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if is_compact() {
        let tier = rationale.split_whitespace().next().unwrap_or("tier=unknown");
        serializer.serialize_str(tier)
    } else {
        serializer.serialize_str(rationale)
    }
}
```

**How it's used:** Each tool handler wraps the response build and serialization:

```rust
// In tools/assemble_context.rs, after getting results from the capability:
let _guard = crate::tools::compact::set_compact(params.compact);
let response = if is_compact {
    ToolResponse::complete_list_compact(results, count, "...")
} else {
    ToolResponse::complete_list(results, count, "...")
};
let json_val = serde_json::to_value(response)?;
// _guard drops here. json_val is fully serialized and self-contained.
Ok(json_val) // Value, not ToolResponse — see step 2 of task 4 in the plan
```

**Thread safety:** `CompactGuard` is stack-bound to a single async task. The serde serializer runs synchronously within the same call stack. No shared mutable state crosses `.await` boundaries.

**Why thread-local over parameter threading:** Passing `compact` through every struct and function would pollute 30+ construction sites with a presentation concern. Thread-local is the pragmatic choice for a cross-cutting serialization toggle. The alternative of adding `compact` to the struct fields and trying to read it from `serialize_with` is technically impossible — serde's field-level helpers don't receive parent struct reference.

## 4. Test Impact

### 4.1 Tests that need updating

All tests that assert on verbose `rationale` content need updating -- they must either:
- (a) Set `compact=false` explicitly on the test request and keep current assertions, or
- (b) Change assertions to match the slim `tier=<tier>` compact form.

**Preference: (a) for service_acceptance and service_integration tests** (they're testing tier classification, ranking metadata, and view_mode behaviour in the domain, not the MCP wire format). **(b) for tools_e2e tests** (they exercise the actual MCP surface where compact=true is what an agent sees).

**Important:** `compact: false` must be added to the `AssembleContextRequest` / `ExplainRequest` struct literal in the test -- not to `AssembleContextParams` in a JSON payload. Service tests call `service.assemble_context(request)` directly, so adding the field to the struct literal is correct.

The request structs get a new field with a `serde(default)` of `true`; without `compact: false` the compact path will be taken and assertions on verbose `rationale` will fail.

| File | Approx. Lines | Action |
|------|-------|--------|
| `tests/service_acceptance.rs` | ~577, ~896, ~986, ~1110, ~1196, ~1686 | Add `compact: false` to `AssembleContextRequest` struct literal in `service.assemble_context(...)` calls that assert on verbose `rationale` |
| `tests/service_integration.rs` | ~1579, ~1666, ~1794, ~1853 | Same -- add `compact: false` to `AssembleContextRequest` struct literal |
| `tests/tools_e2e.rs` | ~793 (`citation_context`) | No change needed (field kept populated) |
| `tests/explain_provenance.rs` | ~654, ~753 (`graph_insights`) | No change needed (field kept populated) |

### 4.2 Tests that should NOT break

- `tests/explain_provenance.rs:654-657, 753-757` — `graph_insights` batch equality: unchanged (field kept).
- `tests/tools_e2e.rs:793-796` — `citation_context`: unchanged (field kept).
- `tests/service_integration.rs:1798, 1855` — `provenance` kind assertions (`kind: hub_entity\|community` from `view_mode=map`): unchanged (field kept).
- `tests/eval_agent_memory_lifecycle.rs` — `public_surface_snapshot`: unchanged (compact is a parameter, not a new tool).
- All eval-harness suite tests: unchanged (they exercise the pipeline, not the serialization).
- `handlers.rs:1420-1495` schema tests: unchanged (`Option::is_none` and `skip_serializing_if` do not change the JSON schema).

### 4.3 Schema tests

`tests/tools/params.rs` has schema assertions on `AssembleContextParams` and `ExplainParams` that test for the presence of specific JSON schema properties. The new `compact` field will appear in these schemas as an optional boolean with a default — no existing assertions should break, but confirm during implementation.

## 5. Phase B — Response-Size Eval Harness

### 5.1 Purpose

Measure the byte-level impact of `compact=true` vs `compact=false` using the existing eval-harness infrastructure. This is a **measurement-only** profile — no quality gates, no hard floors. It proves the design works and sets targets for future refinement.

### 5.2 Profile

New file: `evals/profiles/response_size.json`

```json
{
  "schema_version": "memory-mcp-eval-profile/v1",
  "profile": "response_size",
  "time_budget_seconds": 300,
  "suites": [
    {
      "id": "response-size",
      "expected_coverage": {
        "exact_cases": 66
      }
    }
  ],
  "gates": []
}
```

No gates — this profile reports only.

### 5.3 Suite

New file: `crates/eval-harness/src/suites/response_size.rs`

The suite:
1. Loads the same fixtures as `local-retrieval` (`tests/fixtures/retrieval_cases.json`, 66 cases).
2. For each case: seeds facts, runs `assemble_context` twice (compact=false, compact=true), measures `serde_json::to_string(&response).len()` for each.
3. For the first N cases (e.g., 5): also runs `explain` twice and measures.
4. Reports `compact_bytes`, `verbose_bytes`, `delta_pct` per case.
5. Aggregates: median + p95 bytes per tool per mode.
6. Produces a baseline artifact at `target/evals/baselines/response_size_v1.json`.

Package the hydration pipeline exactly the same as `retrieval.rs`: spawn a fresh `make_service`, seed facts, call `service.assemble_context(...)`. The `compact` parameter is passed at the *request* level (not the tool handler) because eval-harness does not go through the MCP tool layer.

### 5.4 Registration

Register in `crates/eval-harness/src/main.rs` inside `cmd_run`:
```rust
"response-size" => {
    suites.push(Box::new(ResponseSizeSuite::new()));
}
```

And add to `crates/eval-harness/src/suites.rs`:
```rust
pub mod response_size;
pub use response_size::ResponseSizeSuite;
```

### 5.5 Success criteria

The byte-reduction percentages were measured rather than guessed. Phase B confirmed the target reductions; the measured results and quality-profile validation are recorded in ADR-0022.

## 6. Phase C — Completed default decision

Phase B confirmed the target reductions. ADR-0022 records the measured byte
reduction, confirms `compact=true` as the shipped default, documents the
`compact=false` debugging escape hatch, and establishes the presentation-layer
policy for future duplicate or debug-only fields.

## 7. Constraints and Guardrails

- **v5 bench quality MUST NOT regress.** The completed PR, Release, and Nightly validation is recorded in ADR-0022; rerun those profiles before changing this contract.
- **Pipeline perf MUST NOT regress.** The completed pipeline benchmark is recorded in ADR-0022; rerun it before changing serialization behavior. The `compact` flag is serialization-only.
- **No new dependencies.** `serde` skip annotations and a thread-local are standard library.
- **No new tool.** `compact` is a parameter. 8-tool surface is frozen.
- **`has_more: false` omission is spec-compliant.** MCP tool responses use JSON; absent boolean fields default to `false` in consumer parsing.
- **`total_count` omission is safe.** Consumers that paginate check `has_more`, not `total_count == len(result)`. Omitting the redundant `total_count` when the list is complete loses no signal.

## 8. Rejected Alternatives

| Alternative | Why rejected |
|-------------|-------------|
| Delete `rationale` entirely | 12+ tests assert on `rationale` content; `retrieval_tier` is needed by agents for tier-aware processing |
| Delete `quote` from struct | Backward-incompatible; deserializers would break. Skip-at-serialize is safer |
| Make `graph_insights` "present on first item only" | Violates batch equality contract tested at `explain_provenance.rs:753-757` |
| Slim `provenance` shape in-place | `view_mode=map` enrichment depends on the provenance shape; breaking it would break map-view tests (service_integration.rs:1798,1855 verify `kind`) |
| Add a new `compact_view` tool | 8-tool surface is frozen; would require ADR |
| Always-slim, no toggle | Blocks debugging; breaks tests irreparably |
| `compact=false` as default | Defeats the purpose — LLM agents won't opt in |
| Thread `compact` through every struct field | Serde cannot guarantee field-level context: `serialize_with`/`skip_serializing_if` receive `&FieldValue`, not `&self`. Field-level threading requires wrapper types, which is more invasive than the scoped thread-local. |

## 9. Artifact Map

| Artifact | Path | Purpose |
|----------|------|---------|
| Design spec | `docs/superpowers/specs/2026-07-30-token-efficient-responses-design.md` | This document |
| Implementation and validation record | `docs/adr/0022-compact-response-default-for-llm-consumers.md` | Accepted decision and measured validation |
| Response-size profile | `evals/profiles/response_size.json` | Gate-free measurement of compact versus verbose output |
| New param types | `crates/memory-mcp/src/tools/params.rs` | `compact` field on `AssembleContextParams`, `ExplainParams` |
| New request fields | `crates/memory-mcp/src/models/request.rs` | `compact` field on `AssembleContextRequest`, `ExplainRequest` |
| Compact module | `crates/memory-mcp/src/tools/compact.rs` | Thread-local `CompactGuard`, `skip_if_compact`, `serialize_rationale` |
| Updated response items | `crates/memory-mcp/src/models/request.rs` | serde annotations on `AssembledContextItem`, `ExplainItem` |
| Updated tool handlers | `crates/memory-mcp/src/tools/{assemble_context,explain}.rs` | Plumb `compact`, wrap serialization, return `Value` |
| Updated envelope | `crates/memory-mcp/src/tools/response.rs` | `complete_list_compact` constructor |
| Updated MCP handler | `crates/memory-mcp/src/mcp/handlers.rs` | Accept `Value` from `assemble_context`/`explain` tools |

| New eval suite | `crates/eval-harness/src/suites/response_size.rs` | Byte-size measurement |
| Updated eval main | `crates/eval-harness/src/main.rs` | Register `response-size` suite |
| Updated eval suites | `crates/eval-harness/src/suites.rs` | Module declaration |
| Updated tests | 2 files listed in 4.1 | `compact=false` on test assertions |
| CLI flag | `crates/memory-mcp/src/cli/args.rs` | `--compact`/`--no-compact` on `assemble-context` and `explain` |
