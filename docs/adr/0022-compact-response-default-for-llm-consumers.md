# ADR-0022: Ship Compact Responses as Default for LLM Consumers

> **Status:** Draft (awaiting Phase B data)
> **Date:** 2026-07-30
> **Related:** [Design Spec](../superpowers/specs/2026-07-30-token-efficient-responses-design.md)

## Context

`memory_mcp` serves LLM and AI-agent consumers exclusively. Every byte of every MCP tool response is read into the consumer's context window, where it consumes tokens and may cause early truncation. The current `assemble_context` and `explain` responses carry redundant and debug-oriented fields that add no decision-quality signal for agent consumers.

The v5 benchmark run (2026-07-29) demonstrates full quality at 17/17 gates passed across PR, Release, and Nightly profiles. Any change to response shape must preserve these quality gates.

We identified three approaches:
1. Always-slim responses (no toggle) — breaks debugging and existing tests irreparably.
2. Opt-in `compact` parameter (default `false`) — LLM agents won't opt in; context savings don't materialize.
3. `compact` parameter defaulting to `true` — agents get slim responses by default; operators and tests can opt out.

Approach 3 was selected with the parameter name `compact` (per user's explicit request).

## Decision

**Add a `compact: bool` parameter to `assemble_context` and `explain` tools, defaulting to `true`.**

When `compact=true`:
- `quote` field is omitted from serialized output (duplicates `content` in every observed case).
- `rationale` is slimmed from a 120-character debug string to just `tier=<tier>`.
- `has_more: false` is omitted from the response envelope.
- `total_count` is omitted when it equals the result list length (redundant).

Under the hood, the wire format changes: the tool handler returns a `serde_json::Value` (pre-serialized JSON) instead of `ToolResponse<Vec<AssembledContextItem>>`. This applies to both `assemble_context` and `explain`. Callers that already consume JSON (MCP clients, the CLI) are unaffected — `Value` serializes identically.

When `compact=false`:
- Full verbose response shape is preserved (backward-compatible debugging).

All fields remain in struct definitions and JSON schema. They are skipped at *serialization* time via `#[serde(skip_serializing_if = ...)]` (or a custom `serialize_with` for `rationale`) reading from a scoped thread-local `CompactGuard`. Deserialization paths, test struct-literal construction, and the schema tests at `mcp/handlers.rs:1420-1495` are unchanged.

## Rationale

1. **User's explicit choice.** The user selected approach 3, named the flag `compact`, and confirmed `compact=true` as the default. The user explicitly stated they are "not really worried about broken tests" in service of the context-reduction goal.

2. **LLM consumers don't need debug fields.** `rationale` (120+ chars of raw scores), `quote` (duplicate of `content`), and `has_more: false` add no signal that agents use for retrieval ranking, citation, or decision-making.

3. **Measurable, not guessed.** Phase B of the design adds a `response-size` eval profile that measures byte reduction with real fixture data. The ADR will be marked "Accepted" only after Phase B data confirms the target reductions.

4. **Escape hatch preserved.** `compact=false` gives operators and test authors full verbose output when needed. No capability is lost.

5. **8-tool surface frozen.** Adding a parameter to existing tools is not a new tool. The `public_surface_snapshot` test remains green.

6. **v5 quality gates are the safety net.** Before claiming this ADR Accepted, we re-run PR/Release/Nightly eval profiles to confirm all 17 gates pass with identical observed values. If any gate regresses, the change is reverted before merge.

## Consequences

- LLM-agent consumers get smaller responses by default, reducing context pressure.
- `compact=false` must be passed explicitly by tests that assert on verbose `rationale` content (2 test files, ~12 assertion sites). See spec §4.1 for the full list.
- The tool handler return type changes from `Result<ToolResponse<T>, MemoryError>` to `Result<Value, MemoryError>` for `assemble_context` and `explain`. This keeps the compact-state guard scoped to the serialization site and eliminates a cross-call thread-local leak.
- A new `response-size` eval profile (gate-free, measurement-only) quantifies the byte reduction.
- `thread_local!` is introduced for cross-cutting serde behavior — scoped to a single async call, not global. Drop happens before the function returns.
- Future debug-only or duplicate fields should follow the same pattern: populate under `compact=false`, skip under `compact=true`.
- Pipeline perf (ingest, extract, NER, context assembly) is not affected — `compact` is serialization-only.

## Validation

- [ ] Phase B `response-size` profile confirms ≥30% byte reduction for `assemble_context`, ≥20% for `explain`.
- [ ] PR profile (`evals/profiles/pr.json`) passes with identical 7/7 gates and 119/119 cases.
- [ ] Release profile (`evals/profiles/release.json`) passes with identical 9/9 gates and 123/123 cases.
- [ ] Nightly profile (`evals/profiles/nightly.json`) passes with identical 1/1 gate and 121/121 cases.
- [ ] Pipeline benchmarks (`benches/pipeline.rs`) show no regression.
- [ ] `cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings` passes with zero warnings.
