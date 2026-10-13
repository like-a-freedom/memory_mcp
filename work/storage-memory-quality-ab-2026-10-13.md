# Local storage-remediation quality A/B

This is supplemental local quality evidence. It does not close G4, G7, G8, or the complete G9 gate.

- Source parent: `2566128d03ba5c60f6fb5d8ebce0d069c6916d86`; isolated release binary SHA-256 `852c5d95b8926850106b59878e87ebf27487786e34ed97b2764f78ce8840b398`.
- Candidate: `756a33ea7a21377616df79306db19dd5aff2f883`; isolated release binary SHA-256 `35fd58fdb3108195c1a5eacdc39134e64d38487e5fe8e85c4fcf578e76adc8c4`.
- Both builds used `Cargo.lock` SHA-256 `6ccc43a6bf7113e241e9636e425a5082de7ed4cce58a6e1f750e557445ab6cbb`, `rustc 1.99.0 (b940084d7 2026-09-28)`, macOS/aarch64, the same verified local-candle e5 and mMARCO model bundles, and release profile SHA-256 `c717bacb28e8d379255837321cce7637d2ee44d3d16c781ccd73ed3741e40f0c`.
- Source-parent artifact: run `run-1791919626`, 154/154 outcomes and 12/12 gates passed; artifact SHA-256 `dd9899cd213ea5b06174d0914e795575abfa192e393e2d07e1ff55d05d3f45d4`.
- Candidate artifact: run `run-1791919005`, 154/154 outcomes and 12/12 gates passed; artifact SHA-256 `4ed603ff9d62d81a6263103cdabaa7e56e4c238205be06292ed2ad7abae6373c`.
- All six retrieval-quality aggregate metrics were `1.0` on both runs (37/37 cases). `native_quality_has_zero_regression` compared the isolated artifacts and passed.
- Evaluation duration is recorded for provenance only; it is not a performance comparison. This macOS/aarch64 A/B does not substitute for native Linux/amd64 memory or performance evidence, nor the HTTP/stdio/CLI mode-specific quality matrix.

Raw JSON artifacts are retained in `/private/tmp/memory-mcp-quality/` as `source-parent-release-isolated.json` and `candidate-release-isolated.json` for this local run.
