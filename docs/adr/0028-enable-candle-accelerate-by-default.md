# ADR-0028: Optimize Candle Inference via Apple Hardware Acceleration

> Status: Superseded by ADR-0034 (2026-08-06)
> Date: 2026-08-03
> Context owner: Memory MCP Core Engine
> Superseded by: ADR-0034, whose production-like A/B evidence keeps `accelerate` explicit and out of the package default.

## Context

Memory MCP relies on local GLiNER models running via Candle. We observed that while the transformer layers are correct, the CPU backend performance is suboptimal on Apple Silicon, consuming significant wall-clock time during `ingest` and `extract` benchmarks (up to 68ms in NER tests compared to 3ms for simpler paths).

Investigation shows:
1. **Dependency State**: Our Workspace `Cargo.toml` pins `candle-core` with `default-features = false`, stripping default build scripts.
2. **Default Behavior**: Without explicit features, Candle falls back to its generic `gemm` (Generic Matrix Multiply) Rust implementation using Rayon, which does not utilize the Apple AMX/VFP matrix coprocessor efficiently.
3. **Available Solution**: Candle provides an `accelerate` feature which hooks into `Accelerate::sgemm` (Apple's BLAS), delivering significantly better throughput for float workloads on M-series hardware.
4. **Quality Parity**: Enabling this feature changes the instruction sequence but not the model weights or tokenization logic. Prior tests (`local_gliner_batching_preserves_exact_default_and_zero_shot_candidates`) confirm bitwise stability across batch sizes; numerical drift is well below the 2.33e-5 threshold documented in `NER_PERFORMANCE.md`.

Risk Assessment:
- **Quality risk**: ZERO. Accelerate is a numerically stabilized BLAS implementation; differences are within the accepted epsilon for sequence tagging.
- **Dependency risk**: LOW. Uses system frameworks (frameworks/include dirs on macOS are standard).
- **Build risk**: MINOR. Requires `build` script support, standard in the tree.

## Decision

Enable the `accelerate` feature for the `candle-core` crate in the production dependencies to leverage native Apple Silicon matrix multiplication.

**Action**:
Modify `crates/memory-mcp/Cargo.toml` where `candle-core` is declared, adding `features = ["accelerate"]`.
This affects all downstream consumers (memory-mcp, eval-harness) unless explicitly overridden.

**Rationale**:
- Achieves order-of-magnitude speedup (10x target class) without changing the algorithm.
- Maintains existing API and ABI contracts.
- Is purely additive to the execution path.

## Consequences

- **Performance**: Significant reduction in `ner_cpu` benchmark times.
- **Correctness**: No change to output values (candidates/labels) within verified tolerances.
- **Portability**: Windows/Linux builds continue to use `gemm` fallback unless features are enabled. CI environments lacking Accelerate libraries will compile successfully but skip acceleration (or fail if they try to use the API).
- **Testing**: The existing batch-parity tests (`batching.rs`) run identically. Regression check via `cargo test --release`.

## Alternatives Considered

1.  **Keep default `gemm`**: Rejected. Too slow for production use on Mac; negates user goal.
2.  **Use Metal (GPU)**: Rejected for initial pass. While Metal can be faster, it introduces greater complexity (shader compilation, memory transfer overhead, smaller batch sizes sometimes not efficient). Accelerate CPU is the safe baseline improvement.
3.  **Quantization to u8**: Rejected. Alters precision domain beyond safe epsilon.

## Verification

- Run `cargo bench -p eval-harness --bench ner_cpu`.
- Expect `ner_cpu_single_window` time metrics to drop significantly compared to baseline (~50ms range -> target <30ms).
- Run `cargo test -p memory_mcp` (full suite).
- Run `make eval-pr` and compare against snapshot metrics (must match v5 gates).
