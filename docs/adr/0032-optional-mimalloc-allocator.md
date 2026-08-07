# 0032: Optional mimalloc Allocator

## Status
Accepted; amended by ADR-0034 (2026-08-06), which retains `mimalloc` as an opt-in experiment and rejects default promotion for the measured workload.

## Context
vmmap shows ~5.1-5.2 GB of MALLOC_SMALL (empty) regions — freed pages macOS
malloc retains in per-thread arenas (1317-1346 regions, ~1 MB dirty). This is
the dominant RSS term and does NOT shrink with model unload alone: it is not
counted in the physical footprint (1.9 GB), but it is what the per-process RSS
number the user watches stays stuck at after unload without an allocator that
returns freed memory.

## Decision
Add an optional Cargo feature `mimalloc` (default off) that installs
mimalloc as the process global allocator via #[global_allocator] in main.rs.
mimalloc returns freed spans to the OS aggressively, bounding RSS to live
allocations. Feature-gated so the default build is untouched.

## Consequences
+ RSS converges to ~live model (1.5 GB) instead of ratcheting to 7 GB.
+ It remains available as an opt-in allocator experiment for workloads where
  the platform allocator retains freed model memory.
- Later production-like measurement in ADR-0034 did not reproduce the original
  RSS benefit: default allocator plus idle unload reached 430 MB RSS, while
  mimalloc plus idle unload reached 2,556 MB. The earlier target is historical
  evidence, not the current default-policy basis.
- Requires user approval for the Cargo.toml change (AGENTS.md).
- Binary-only: the static lives in main.rs, so library tests don't exercise
  it; soak verification (Task 11) validates it on the real binary.
