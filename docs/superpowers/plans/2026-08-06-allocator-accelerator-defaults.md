# Allocator and Apple BLAS Default Policy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Apply the fresh allocator evidence, keep mimalloc opt-in because it regresses observed RSS, document idle unload as the effective measured lever, and evaluate Apple Accelerate without weakening cross-platform or quality guarantees.

**Architecture:** The existing release-server MCP-stdio matrix in `docs/performance/MEMORY_PROFILE.md` is the authoritative allocator experiment; no second synthetic probe is needed. Keep the global allocator binary-only and the package default unchanged. Compare Accelerate on the existing NER/pipeline benchmarks, and add no release command unless the complete benchmark surface is useful and strictly non-regressive.

**Tech Stack:** Rust 1.88+, Cargo resolver 3, `mimalloc` 0.1, Candle pinned to `21cca0b`, Criterion 0.5, the existing eval-harness benchmarks and quality profiles, macOS arm64 `ps`/`footprint` measurements, and Keenable-verified Cargo/crate contracts.

## Global Constraints

- `docs/performance/MEMORY_PROFILE.md` is the fresh allocator evidence for this plan: measured 2026-08-06 at commit `2a592624` on macOS Tahoe 26.5.2 arm64.
- Keep `mimalloc` out of `default`; the fresh comparison measured 2,556 MB RSS with mimalloc plus unload versus 430 MB with the default allocator plus unload.
- Do not treat the 190 MB mimalloc physical-footprint result as sufficient to override the 2,556 MB RSS result; both metrics matter, and RSS is the user-visible regression in this case.
- Keep `GLINER_IDLE_UNLOAD_SECS` default semantics unchanged: `0` remains the compatibility default. Document `30` seconds as a workload-specific recommendation, not a universal runtime default.
- Keep `accelerate` separate and out of `default`; use it only through an explicit Apple-specific build or benchmark command.
- Do not change the GLiNER model, labels, threshold, tokenizer, candidate limits, retrieval limits, model precision, fallback semantics, or MCP tool surface.
- Do not add a synthetic allocator probe, eval-harness allocator feature, or second RSS driver in this plan; the existing MCP-stdio matrix has stronger production-path fidelity.
- Any Accelerate candidate must preserve all quality gates and every common benchmark must pass the strict no-degradation rule.
- A real or slower-side-separated benchmark regression blocks adoption; measurement noise must be resolved by rerunning, not hidden by averaging incompatible runs.
- Keep the global allocator in `crates/memory-mcp/src/main.rs`; never move it into the reusable `memory_mcp` library.
- Do not change dependencies or versions; the locked dependency graph is already sufficient.
- Production code must not add `unwrap()`; this plan intentionally makes no production Rust changes.
- Run validation with `--locked` where Cargo supports it.
- Leave the unrelated untracked `docs/superpowers/plans/2026-08-04-zero-config-defaults.md` untouched.

---

## File Map

### Files to modify

- `docs/adr/0034-allocator-and-accelerator-default-policy.md` — record the resolved mimalloc decision and the remaining Accelerate policy.
- `Makefile` — change only if Accelerate passes the useful/non-regressive gate; do not add a mimalloc convenience target.
- `README.md` — document the actual feature defaults, idle-unload recommendation, and valid benchmark commands.
- `AGENTS.md` — document the actual feature policy and the strict validation rules.
- `docs/performance/NER_PERFORMANCE.md` — correct feature-qualified benchmark commands and link the allocator evidence.
- `crates/eval-harness/src/adapters.rs` — update only stale `#[cfg(test)]` episode IDs so quality-gate fixtures satisfy the current `<table>:<id>` contract.

### Read-only evidence and references

- `docs/performance/MEMORY_PROFILE.md` — fresh allocator result; do not replace with a synthetic benchmark.
- `docs/evals/BENCHMARK_RUN_REPORT_2026-07-29-v5.md` — historical quality and performance baseline.
- `crates/memory-mcp/Cargo.toml` — verify that `default = []`, `mimalloc` is optional, and `accelerate` is explicit.
- `crates/eval-harness/benches/pipeline.rs` — pipeline benchmark target.
- `crates/eval-harness/benches/ner_cpu.rs` — actual warm GLiNER CPU benchmark target.
- `crates/eval-harness/benches/ner_metal.rs` — macOS arm64 benchmark target and unsupported-platform behavior.

The fresh `MEMORY_PROFILE.md` already contains the controlled server-process
allocator matrix, methodology, raw comparison numbers, and recommendation. A
second synthetic allocator measurement would create a competing source of truth,
so this plan deliberately keeps the allocator work on the production-like
server path.

---


## Task 1: Close the mimalloc default decision from fresh evidence

**Files:**
- Modify: `docs/adr/0034-allocator-and-accelerator-default-policy.md`
- Read: `docs/performance/MEMORY_PROFILE.md`
- Verify: `crates/memory-mcp/Cargo.toml`

**Interfaces:**
- Consumes: the four-row fresh allocator matrix.
- Produces: a resolved policy: default allocator remains unchanged, mimalloc remains opt-in, and idle unload is the measured recommendation for infrequent extraction.

- [x] **Step 1: Verify the measured comparison**

Record these exact values in the ADR decision rationale:

| Variant | Post-extraction physical footprint | Post-extraction RSS |
|---|---:|---:|
| Default allocator + `GLINER_IDLE_UNLOAD_SECS=30` | 277 MB | 430 MB |
| Mimalloc + `GLINER_IDLE_UNLOAD_SECS=30` | 190 MB | 2,556 MB |

Calculate the relevant deltas:

```text
physical footprint: 190 - 277 = -87 MB
RSS: 2556 - 430 = +2126 MB
```

Expected conclusion: mimalloc fails a no-degradation policy for the observed
local-use case. It must not become the default.

- [x] **Step 2: Verify that the Cargo default was not changed**

Run:

```bash
cargo metadata --locked --format-version 1 --no-deps \
  | python3 -c 'import json,sys; m=json.load(sys.stdin); p=next(p for p in m["packages"] if p["name"] == "memory_mcp"); assert p["features"]["default"] == [], p["features"]; assert p["features"]["mimalloc"] == ["dep:mimalloc"]; assert p["features"]["accelerate"] == ["candle-core/accelerate"]'
```

Expected: all assertions pass. The feature table remains:

```toml
[features]
default = []
accelerate = ["candle-core/accelerate"]
# other existing features remain unchanged
mimalloc = ["dep:mimalloc"]
```

- [x] **Step 3: Update the ADR wording**

The ADR must state all of the following:

1. mimalloc is still available as an explicit server feature;
2. mimalloc is not a universal RSS fix on the measured macOS runtime;
3. default allocator plus idle unload is the best observed configuration for this use case;
4. `GLINER_IDLE_UNLOAD_SECS=30` remains a recommendation, not a changed code default;
5. any future default promotion requires a new workload-specific measurement.

- [x] **Step 4: Do not change production code**

No Rust source or Cargo feature change is required by this task. The measured
result already answers the promotion question. A future mimalloc investigation
must be opened as a new plan only if a different allocator, OS accounting
problem, or long-lived workload produces a new hypothesis.

---

## Task 2: Run the remaining Accelerate A/B

**Files:**
- Read-only: `crates/eval-harness/benches/ner_cpu.rs`
- Read-only: `crates/eval-harness/benches/pipeline.rs`
- Update: `docs/adr/0034-allocator-and-accelerator-default-policy.md`
- Update: `docs/performance/NER_PERFORMANCE.md` with actual results

**Interfaces:**
- Consumes: the current system-CPU benchmark and the same benchmark with `memory_mcp/accelerate` enabled.
- Produces: one of three explicit outcomes: `USEFUL_NON_REGRESSION`, `NON_REGRESSION_ONLY`, or `REJECTED_REGRESSION`.

- [x] **Step 1: Establish the portable CPU baseline**

On the same idle macOS arm64 host, keep power and thermal settings stable and
run both benchmark families three independent times. Reuse the target directory
for compiled artifacts, but save every stdout stream so the comparison is
reproducible:

```bash
mkdir -p target/evals/accelerate-ab/system

CARGO_TARGET_DIR=target/bench-accelerate-system cargo bench --locked \
  -p eval-harness --no-default-features --bench ner_cpu \
  -- --noplot --measurement-time 30 \
  | tee target/evals/accelerate-ab/system/ner_cpu-run-1.txt
CARGO_TARGET_DIR=target/bench-accelerate-system cargo bench --locked \
  -p eval-harness --no-default-features --bench pipeline \
  -- --noplot --measurement-time 30 \
  | tee target/evals/accelerate-ab/system/pipeline-run-1.txt
CARGO_TARGET_DIR=target/bench-accelerate-system cargo bench --locked \
  -p eval-harness --no-default-features --bench ner_cpu \
  -- --noplot --measurement-time 30 \
  | tee target/evals/accelerate-ab/system/ner_cpu-run-2.txt
CARGO_TARGET_DIR=target/bench-accelerate-system cargo bench --locked \
  -p eval-harness --no-default-features --bench pipeline \
  -- --noplot --measurement-time 30 \
  | tee target/evals/accelerate-ab/system/pipeline-run-2.txt
CARGO_TARGET_DIR=target/bench-accelerate-system cargo bench --locked \
  -p eval-harness --no-default-features --bench ner_cpu \
  -- --noplot --measurement-time 30 \
  | tee target/evals/accelerate-ab/system/ner_cpu-run-3.txt
CARGO_TARGET_DIR=target/bench-accelerate-system cargo bench --locked \
  -p eval-harness --no-default-features --bench pipeline \
  -- --noplot --measurement-time 30 \
  | tee target/evals/accelerate-ab/system/pipeline-run-3.txt
```

Do not compare these values with the historical Metal stub or with the v5
numbers as if they were the same run. The CPU benchmark must use the current
warm GLiNER path described in `ner_cpu.rs`; pipeline results must be compared
only with the same pipeline benchmark names.

- [x] **Step 2: Measure Candle Accelerate**

Run the identical six benchmark invocations with the same host settings and
three independent runs, changing only the dependency feature and output
location:

```bash
mkdir -p target/evals/accelerate-ab/candidate

CARGO_TARGET_DIR=target/bench-accelerate-candidate cargo bench --locked \
  -p eval-harness --no-default-features --features memory_mcp/accelerate \
  --bench ner_cpu -- --noplot --measurement-time 30 \
  | tee target/evals/accelerate-ab/candidate/ner_cpu-run-1.txt
CARGO_TARGET_DIR=target/bench-accelerate-candidate cargo bench --locked \
  -p eval-harness --no-default-features --features memory_mcp/accelerate \
  --bench pipeline -- --noplot --measurement-time 30 \
  | tee target/evals/accelerate-ab/candidate/pipeline-run-1.txt
CARGO_TARGET_DIR=target/bench-accelerate-candidate cargo bench --locked \
  -p eval-harness --no-default-features --features memory_mcp/accelerate \
  --bench ner_cpu -- --noplot --measurement-time 30 \
  | tee target/evals/accelerate-ab/candidate/ner_cpu-run-2.txt
CARGO_TARGET_DIR=target/bench-accelerate-candidate cargo bench --locked \
  -p eval-harness --no-default-features --features memory_mcp/accelerate \
  --bench pipeline -- --noplot --measurement-time 30 \
  | tee target/evals/accelerate-ab/candidate/pipeline-run-2.txt
CARGO_TARGET_DIR=target/bench-accelerate-candidate cargo bench --locked \
  -p eval-harness --no-default-features --features memory_mcp/accelerate \
  --bench ner_cpu -- --noplot --measurement-time 30 \
  | tee target/evals/accelerate-ab/candidate/ner_cpu-run-3.txt
CARGO_TARGET_DIR=target/bench-accelerate-candidate cargo bench --locked \
  -p eval-harness --no-default-features --features memory_mcp/accelerate \
  --bench pipeline -- --noplot --measurement-time 30 \
  | tee target/evals/accelerate-ab/candidate/pipeline-run-3.txt
```

Expected on macOS: the candidate resolves Candle's `accelerate-src` backend and
builds successfully. If the host cannot link Accelerate, record a failed
platform build and do not claim a performance result.

- [x] **Step 3: Apply the strict no-degradation rule**

For every common benchmark, compare all three candidate runs with all three
baseline runs using the Criterion median estimate and its 95% confidence
interval. Record the per-run values, not only an average of the three reports.
Use this decision table:

| Outcome | Required evidence | Action |
|---|---|---|
| `USEFUL_NON_REGRESSION` | At least one NER or pipeline median improves by at least 2% with repeatable direction; every other common benchmark is no more than 2% slower, with no slower-side-separated 95% interval; quality parity is exact. | A macOS-only explicit release target may be added; never change package defaults. |
| `NON_REGRESSION_ONLY` | No benchmark violates the no-degradation rule, but no benchmark shows a repeatable >=2% improvement. | Keep the feature available for explicit experiments; do not add a release target or claim an optimization. |
| `REJECTED_REGRESSION` | Any repeated median is more than 2% slower, any slower-side-separated 95% interval is observed, build is not reproducible, or quality parity fails. | Do not expose a new release target and do not recommend Accelerate for this fixture. |

A smaller slowdown is not automatically acceptable: if its confidence interval
is separated on the slower side, it is a regression. If intervals overlap or
runs disagree, rerun the same command until the result is classified; never
hide ambiguity by averaging incompatible runs. A performance improvement never
justifies changing labels, thresholds, model precision, tokenization, candidate
limits, retrieval limits, or fallback behavior.

- [x] **Step 4: Record the result without conflating it with mimalloc**

Add the measured per-benchmark table, run identifiers, host/toolchain, feature
flags, and one of the three outcomes to ADR-0034 and
`docs/performance/NER_PERFORMANCE.md`:

```text
Accelerate policy: explicit macOS feature; package default remains unchanged
Accelerate result: outcome, baseline median, candidate median, delta, 95% CI
Allocator policy: default allocator retained; mimalloc remains opt-in
```

Measured result on 2026-08-06: `REJECTED_REGRESSION`. Direct warm GLiNER
improved by 67.17% (single window) and 56.32% (multi-window), but
`default_service_extract_warm` was slower in every paired run, the final paired
Criterion intervals were separated on the slower side, and pipeline ingest and
context were slower in the three-run comparison. No Make target or production
speed claim is allowed. The detailed table is recorded in
`docs/performance/NER_PERFORMANCE.md` and the policy in ADR-0034.

---

## Task 3: Expose only a validated macOS command

**Files:**
- Modify: `Makefile` only when the Accelerate result is `USEFUL_NON_REGRESSION`
- Test: `make -n` and feature-specific Cargo metadata/build checks

**Interfaces:**
- Consumes: the resolved Accelerate outcome from Task 2.
- Produces: either no Makefile change, or one explicit macOS release command. The portable command and package defaults remain unchanged in both cases.

- [x] **Step 1: Preserve the portable release command**

Leave the existing target unchanged:

```make
serve-release:
	cargo run --release -- serve
```

It must not request `accelerate` or `mimalloc` implicitly. Do not add a
`serve-release-mimalloc` convenience target: the fresh memory profile makes
mimalloc an experiment, and the direct feature command is already documented.

- [x] **Step 2: Apply the Accelerate outcome gate**

Task 2 returned `REJECTED_REGRESSION`, so `Makefile` remains unchanged. The
feature remains available through an explicit Cargo command for experiments, but
there is no release target and no speed claim.

If Task 2 returns `USEFUL_NON_REGRESSION`, add exactly this macOS-only target:

```make
.PHONY: eval-pr eval-release eval-nightly serve-release serve-release-macos

serve-release-macos:
	cargo run --release --locked -p memory_mcp --features accelerate -- serve
```

The target is an explicit convenience command, not a package default. It must
not combine `accelerate` with `mimalloc`.

- [x] **Step 3: Validate only the target that exists**

Always run:

```bash
make -n serve-release
```

Run this additional command only when the target was added:

```bash
make -n serve-release-macos
```

Expected: `serve-release` has no feature flag; if present,
`serve-release-macos` contains only `accelerate`. Do not run the macOS target on
a non-macOS host.

---

## Task 4: Synchronize README, AGENTS.md, and performance documentation

**Files:**
- Modify: `README.md`
- Modify: `AGENTS.md`
- Modify: `docs/performance/NER_PERFORMANCE.md`
- Read-only: `docs/performance/MEMORY_PROFILE.md`

**Interfaces:**
- Consumes: the fresh allocator recommendation and the measured Accelerate result.
- Produces: documentation with one consistent feature policy and valid Cargo commands.

- [x] **Step 1: Update README feature guidance**

Document this exact policy:

```text
mimalloc is optional and remains opt-in. The fresh macOS matrix showed lower
physical footprint but substantially higher post-unload RSS, so it is not the
server default.

GLINER_IDLE_UNLOAD_SECS=30 is a measured recommendation for infrequent local
extraction workloads. The compatibility default remains 0.

accelerate is an explicit Apple-specific CPU backend. It is not part of the
portable package default. Do not describe it as faster or add a release target
until the recorded A/B outcome is USEFUL_NON_REGRESSION.
```

Link both ADR-0034 and `docs/performance/MEMORY_PROFILE.md`. If Task 3 adds
`serve-release-macos`, document that command only after the outcome gate passes.

- [x] **Step 2: Correct benchmark commands**

Use dependency-qualified features in documentation:

```bash
cargo bench -p eval-harness --features memory_mcp/metal \
  --bench ner_metal -- --noplot

cargo bench -p eval-harness --features memory_mcp/accelerate \
  --bench ner_cpu -- --noplot
```

Do not document `cargo bench -p eval-harness --features metal`; the eval-harness
package does not define a `metal` feature of its own.

- [x] **Step 3: Update AGENTS.md**

The feature summary must say:

```text
mimalloc: optional server allocator; not default based on MEMORY_PROFILE.md
accelerate: explicit Apple Silicon Candle backend; not default
metal: explicit dependency-qualified backend for macOS benchmarks
```

Keep the existing zero-warning clippy command. Mention `serve-release-macos`
only if Task 3 adds it after a `USEFUL_NON_REGRESSION` result. Do not state that
mimalloc reduces RSS for this project.

- [x] **Step 4: Update NER performance documentation**

Correct the Metal command, add the Accelerate command, and link the memory
profile. State clearly that memory footprint and RSS are separate metrics and
that the default allocator plus idle unload is the measured best configuration
for the documented single-shot workload.

- [x] **Step 5: Validate documentation consistency**

Run:

```bash
python3 -c 'from pathlib import Path; paths=[Path("README.md"),Path("AGENTS.md"),Path("docs/performance/NER_PERFORMANCE.md")]; text="\n".join(p.read_text() for p in paths); assert "--features metal --bench ner_metal" not in text; assert "0034-allocator-and-accelerator-default-policy.md" in text; assert "MEMORY_PROFILE.md" in text'
git diff --check
```

Expected: the stale unqualified Metal command is absent, the current ADR and
memory report are linked, and there are no whitespace errors.

---

## Task 5: Run the final strict quality gate

**Files:**
- Read-only: all Rust source and test targets
- Modify (test-only): `crates/eval-harness/src/adapters.rs`
- Update: `docs/adr/0034-allocator-and-accelerator-default-policy.md` with final Accelerate result

**Interfaces:**
- Consumes: all documentation/Makefile changes and the Accelerate A/B result.
- Produces: a completed policy update with an explicit candidate outcome; production adoption is allowed only when the performance gate is non-regressive.

- [x] **Step 1: Run formatting, metadata, and lint checks**

Run:

```bash
cargo fmt --all --check
cargo metadata --locked --format-version 1 --no-deps
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings
```

Expected: clean formatting, metadata, compilation, and zero-warning clippy.

- [x] **Step 2: Run the complete test matrix**

Run:

```bash
cargo test --workspace --locked
cargo test --workspace --features cli-watch,mcp-apps --locked
cargo test -p memory_mcp --features mimalloc --locked
cargo test -p memory_mcp --features accelerate --locked
```

Run the ignored local-model parity test when the committed fixture is present:

```bash
cargo test -p memory_mcp --test local_model_integration -- --ignored
```

If the fixture is unavailable, record that limitation and do not claim a
model-backed parity run.

- [x] **Step 3: Run every evaluation profile**

Run:

```bash
make eval-pr
make eval-release
make eval-nightly
```

Expected: the v5 quality contract remains green: no failed cases, no invalid
cases, and no regression in retrieval, entity, claim, lifecycle, or end-to-end
metrics.

- [x] **Step 4: Re-run the final performance comparison**

The Task 2 system/candidate comparison was retained under
`target/evals/accelerate-ab/` after the documentation and Cargo-feature policy
changes. The only later Rust change corrected test fixtures behind `#[cfg(test)]`
and cannot affect Criterion bench binaries; no runtime or benchmark execution
code changed. Do not use the historical v5 numbers as the only comparison.

- [x] **Step 5: Record the final policy**

The ADR must end with these resolved statements:

```text
Allocator policy: keep default allocator; keep mimalloc opt-in
Idle-unload policy: keep code default 0; recommend 30 seconds for the measured infrequent-use case
Accelerate policy: explicit Apple-specific feature; package default remains unchanged
Accelerate outcome: REJECTED_REGRESSION for the current all-surface comparison
Quality gate: PASS — all tests, eval profiles, and model-backed parity checks passed
Performance gate: REJECTED_REGRESSION for Accelerate; no candidate adoption or release target
```

A `NON_REGRESSION_ONLY` or `REJECTED_REGRESSION` result does not change
allocator policy and does not justify enabling Accelerate by default. It also
must not produce a release target or speed claim. Only `USEFUL_NON_REGRESSION`
may add the explicit macOS target, and even then the package default remains
portable.

---

## Self-review

### Spec coverage

- Fresh `MEMORY_PROFILE.md` evidence is incorporated: Task 1.
- Mimalloc default promotion is explicitly rejected for the measured workload: Tasks 1 and 4.
- Idle unload is documented without silently changing its compatibility default: Tasks 1 and 4.
- Accelerate receives an independent three-run benchmark and a three-way outcome policy: Task 2.
- Cross-platform defaults remain portable; a Make target is conditional on a useful non-regressive result: Task 3.
- README, `AGENTS.md`, and NER performance docs are synchronized without claiming unmeasured speedups: Task 4.
- Full tests, evaluations, clippy, formatting, and final performance checks remain mandatory: Task 5.

### Consistency checks

- No task creates the removed synthetic allocator probe or duplicate A/B report.
- No task changes `default = []` or adds `accelerate` to `default`.
- `memory_mcp/metal` and `memory_mcp/accelerate` are used when enabling dependency features from eval-harness.
- `MEMORY_PROFILE.md` is the sole fresh allocator evidence source.
- The plan's current decision matches ADR-0034 and the measured table.
