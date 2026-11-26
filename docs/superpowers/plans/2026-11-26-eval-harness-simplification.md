# Eval Harness Simplification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Status:** Implemented and verified on 2026-11-26; retained for user acceptance.
**Evidence:** [Final review, native capture and verification](../../../evals/paired/harness-simplification-current-2026-11-26/README.md).

**Goal:** Bound execution, preserve reproducible evidence, and remove redundant scenario execution.
**Architecture:** Extend existing adapters and graders in vertical TDD slices; retain historical evaluation contracts.
**Tech Stack:** Rust, Python standard library, public MCP, native Apple Silicon.
**Spec:** [Approved design](../specs/2026-11-26-eval-harness-simplification-design.md).

## Global Constraints

- Run evals locally on native Apple Silicon only; no model/provider dependencies for deterministic checks.
- Preserve historical reports, stop rules, known failures, source/time checks and isolated stores.
- Do not switch the current gate until a reviewed compatible baseline and verdict-equivalence check exist.

## Review Focus

- Timeouts must terminate/reap owned children, not merely stop awaiting them.
- Resume must preserve failures and invalidate incompatible downstream evidence.
- Public synthetic traces must preserve actual requests without environment secrets.
- Regression acceptance must not hide existing failures or trade failures between cases.
- Removing diagnostics must preserve targeted ordering, isolation and rollout coverage.

## Tasks

- [x] Bound Python CLI calls in `evals/longmemeval_v2/memory_mcp_backend.py`: public insert tests in `test_memory_mcp_backend.py`, controlled hanging subprocess, explicit failure, reaped child; validate positive finite timeout configuration. RED: missing timeout fell back to the test's one-second safety bound instead of the requested 0.2 seconds. GREEN: all adapter tests pass. Default 1,800 seconds preserves headroom over the observed 991-second capture; this does not implement a whole-run deadline or a snapshot-copy deadline.
- [x] Propagate a whole-run deadline through `crates/eval-harness/src/runner.rs` and owned blocking/process execution; controlled deadline and partial-outcome tests at runner/public CLI boundaries. Preserve unstarted scheduled cases and ensure cancellation actually stops work.
- [x] Add opt-in complete public synthetic request/response traces at paired-agent capture boundaries; replay the exact request through the isolated product. Keep private mode explicit and exclude incidental environment/secrets. Test identity binding, malformed/missing traces and unchanged privacy defaults.
- [x] Record preparation/startup/formation/query/shutdown separately; preserve wall-time/concurrency semantics. Check public result receipts against controlled ordering.
- [x] Freeze and independently review the pre-refactor compatible default baseline, including per-case/per-dimension failures. Do not substitute shadow or assume historical 5/8 is current truth.
- [x] Add selected-case execution and explicit optional diagnostic variants in the existing memory-decisions suite. Verify default work count and that requested diagnostics still execute; keep old receipt loaders compatible.
- [x] Introduce the single-current-capture regression gate, compare each dimension against the reviewed baseline, report known failures separately, and preserve targeted repeat/order/isolation/default-shadow tests. Verify verdict equivalence before switching.
- [x] Expose offline regrade/resume over existing receipts, preserving all attempts and stage compatibility. A scorer failure must not trigger memory or agent execution.
- [x] Measure and optimize context-collection costs without weakening snapshot isolation or public formation. No speed claim from work-count reduction alone.
- [x] Consolidate the current contributor entry point, run the complete native deterministic gate and required lint checks, critically review, and commit verified changes.

Each task follows one public behavior test failing before its implementation, then the narrow passing check and relevant regression set. Agent/LongMemEval captures require an outstanding empirical question; archived grading compatibility uses saved evidence.

## Completion record

The independently reviewed frozen baseline was accepted before switching the active
gate. One final TDD fix pass addressed all five Important findings; no second final
review cycle was introduced. The full harness passed 899 tests with 22 ignored;
subsequent stdio preservation controls passed 20/20, compatibility/CLI targets 28/28,
Python discovery 41 (5 skipped), actual reconciliation disclosure controls 10/10,
and default configuration 1/1. Public wire/order/exclusion checks remained 23/23.
Strict all-target workspace clippy with the required feature profile and formatting
checks passed. Native current gate: 58.17 s; 5 passed, 3 known failures, no invalid,
unobserved or regressed cases. This completes harness simplification, not the three
remaining product-quality failures or population-level agent/benchmark claims.

All existing historical diagnostic profiles explicitly preserve diagnostics=true;
new omitted policy and the current profile use primary-only. Archived loaders and
source evidence are intentionally retained for supported regrading. Removed scratch
logs, duplicate execution/helpers, unused dataset/worker options and detached tasks
have no replacement dangling path. Context-copy/state-sharing changes were declined:
the bounded pilot did not provide evidence to justify weakening isolation.

Timer and replay boundaries are explicit in the spec/runbook: Runner budget excludes
CLI preparation; Python timeout excludes copying; public sidecars persist per phase;
replay requires caller-supplied isolated state. These are declared limits rather than
claims of total-CLI deadlines, arbitrary-descendant cancellation or crash-safe logs.
