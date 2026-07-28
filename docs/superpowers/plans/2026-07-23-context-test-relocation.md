# Context Module Test Relocation Plan

> Status: Proposed (2026-07-23)
> Audit candidate: 3 (deepen `context.rs`)

## Context

The audit reported `context.rs` as a 5686-line monolith. On closer inspection,
On closer inspection, the production code is already decomposed: 18 submodules (`pipeline`, `ranking`, `scoring`, `semantic`, `lexical`, `graph`, `temporal`, `community`, `rescue`, `experience`, `views`, …) hold the logic, and `context.rs` is a thin orchestrator.

| Section | Lines |
|---------|-------|
| `track_fact_accesses` (helper) | 27 |
| `assemble_context` (orchestrator) | 251 |
| `mod tests` | **5365** |

94% of the file is tests that were never relocated to the submodule they test.
The same shape exists in `core.rs` (1334 lines of `mod tests` out of 2606).

This is not an architecture problem — the decomposition the header promises is
**done**. It is a test-locality problem: tier-specific tests sit in the parent
file instead of next to the tier they test.

## Plan

### Step 1 — Relocate tier-local tests in `context.rs`

Walk `context.rs::mod tests`. For each test, identify which submodule's logic
it exercises and move it into that submodule's `mod tests`:

- `sort_facts_by_recency*` → `context/filtering.rs`
- `rank_lexical_records*` → `context/lexical.rs`
- `select_ranked_context_facts*` → `context/ranking.rs`
- `should_prefer_episode_content*` → `context/budget.rs`
- `first_person_episode_item*` → `context/rescue.rs`
- `query_is_first_person_memory*` → `context/ranking.rs`
- `infer_temporal_window*` → `context/temporal.rs`

Keep only cross-tier integration tests in `context.rs::mod tests` (tests that
exercise `assemble_context` end-to-end across multiple tiers).

### Step 2 — Relocate tests in `core.rs`

Apply the same rule: tests exercising `builder` behavior → `core/builder.rs`,
tests exercising helpers → `core/helpers.rs`, tests exercising `MemoryService`
methods that migrate to capabilities (per the capability-seam plan) → those
capability modules.

### Step 3 — Verify

```bash
cargo test -p memory_mcp --lib context
cargo test -p memory_mcp --lib core
cargo clippy --all-targets
cargo fmt --all --check
```

## Sequencing

This is mechanical and low-risk. Do it **after** the capability-seam migration
(Candidate 1), because Step 2 depends on knowing which `core.rs` methods move
to which capability. If done first, Step 2 would need rework.

## ADR needed?

No — the decomposition is already the stated design. This just finishes it.
