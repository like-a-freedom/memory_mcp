# Retained invalid identity attempt

The profile completed eight registered cases with a PASSED suite verdict, but the outer gate correctly rejected the attempt: target/debug/memory_mcp changed while the multi-process capture was running (initial121763… vs end80e9ef…). Individual cases therefore cannot be accepted as one proven immutable product execution. No product regression or accepted eight-case baseline is inferred from this attempt.

The corrected final attempt uses a read-only copy under target/evals/memory-quality-repairs-2026-11-26/bin and a separate native-gate-final destination. Its result, including binary hash, is the acceptance evidence. This directory and original failure log remain to explain the invalid attempt; the original receipts are not overwritten.
