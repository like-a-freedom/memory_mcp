# ADR-0036: Unify NER Extractor Selection and Model Lifecycle

## Status

Accepted (implemented 2026-08-08; see `docs/superpowers/plans/2026-08-07-unified-ner-extractors-and-vago-lfm2.md`)

## Context

NER configuration currently separates `NER_PROVIDER` from `NER_MODEL` and stores provider-specific controls in one flat `NerConfig`. That permits invalid combinations, makes a model checkpoint appear interchangeable across architectures, and becomes more complex when adding the exact `VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER` checkpoint and an explicit Anno ONNX path.

The zero-configuration contract must remain local and download-free. The ordinary release artifact must still expose advanced extractors through runtime configuration rather than build profiles. Model-backed extractors also need one acquisition policy for progress, caching, latest-revision checks, compatibility probes, and last-known-good recovery.

## Decision

### One public selector

`NER_EXTRACTOR` is the only public extractor selector. Its catalog is closed:

- unset or `anno`: lightweight Anno pattern-and-heuristic extraction, with no model download;
- `regex`: the project-owned deterministic regex extractor;
- `anno-onnx`: Anno's explicit NuNER ONNX backend using the `deepanwa/NuNerZero_onnx` export (the `numind/NuNER_Zero` source ships no ONNX files);
- `urchade/gliner_multi-v2.1`: the native Candle classic GLiNER backend;
- `VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER`: a distinct native Candle LFM2 GLiNER backend for that exact checkpoint.

Unknown aliases and arbitrary Hugging Face repository IDs are rejected. `NER_PROVIDER` and `NER_MODEL` are removed rather than deprecated; their presence is an actionable startup error that points to `NER_EXTRACTOR`.

### Typed extractor configuration

The internal configuration is a discriminated extractor configuration. Lightweight Anno and regex carry no model controls. Anno ONNX carries only supported CPU-oriented controls. Classic GLiNER and LFM2 GLiNER may share controls only where their semantics are identical. Explicit settings that do not apply to the selected extractor are rejected rather than ignored.

Canonical settings are:

- selector: `NER_EXTRACTOR`;
- shared model-backed settings: `NER_CACHE_DIR`, `NER_LABELS`, `NER_THRESHOLD`, `NER_MAX_CONCURRENCY`, and `NER_IDLE_UNLOAD_SECS`;
- native GLiNER settings: `GLINER_BATCH_SIZE`, `GLINER_MAX_BATCH_TOKENS`, and `GLINER_DEVICE`.

`NER_IDLE_UNLOAD_SECS` defaults to `0`, which retains a model after first use; a positive value unloads it after that many idle seconds. It controls only in-memory retention, never cached artifacts. The replaced names `NER_MODEL_DIR`, `NER_BATCH_SIZE`, `NER_MAX_BATCH_TOKENS`, `NER_DEVICE`, and `GLINER_IDLE_UNLOAD_SECS` fail with migration guidance.

`anno-onnx` is CPU-only initially. ONNX CoreML and CUDA are outside this decision.

### One ordinary artifact

The ordinary release artifact includes Anno's ONNX capability. Cargo features do not select the normal extractor experience. Lightweight Anno remains the zero-configuration default even though the binary contains ONNX support.

### Shared artifact lifecycle

Memory MCP, not backend libraries, owns acquisition for every model-backed extractor. Extractor definitions own their artifact requirements; one shared artifact store owns:

- resolving the latest upstream repository revision at each startup;
- revision-specific staging and cache layout;
- CLI progress rendering on stderr and schema-versioned JSON Lines MCP-safe progress events on stderr;
- artifact completeness and integrity checks;
- atomic activation;
- incompatible-revision quarantine;
- persisted last-known-good revision state.

Backend loaders receive prepared local checkpoint paths and must not download implicitly. Progress comes from one domain event stream: CLI mode renders it interactively, while MCP mode writes one compact versioned JSON object per line. Events are emitted on phase changes, completion or failure, each crossed 5% download boundary, or after five seconds without another emitted update. MCP stdout remains JSON-RPC only.

The default cache root is `<memory_mcp user data directory>/models/ner`; `NER_CACHE_DIR` overrides only that root. The store does not mutate or clean the global Hugging Face cache. Per extractor it retains the active revision and one previous known-good revision. Older known-good artifacts are removed only after successful activation. Incompatible candidate artifacts are removed after compact revision-keyed failure metadata is persisted.

Concurrent processes coordinate through a per-extractor/revision filesystem lease created with atomic standard-library file creation. The owner records identity, process, timestamps, and heartbeat; waiters emit progress and observe activation rather than downloading duplicate artifacts. Conservative stale-lease recovery uses process-unique staging directories and cleans abandoned staging only after safe lease reclamation.

Latest-revision lookup makes two attempts with short backoff under one ten-second total deadline. When upstream cannot be reached, a complete previously verified revision remains usable with `revision_status=unverified_latest`; without one, startup fails. Downloads have no total wall-clock deadline while bytes advance, but fail after 60 seconds without forward progress. A new revision must construct successfully and pass a fixed smoke inference before activation. A successfully probed model remains loaded and is reused by the first real extraction. An incompatible revision does not replace the last-known-good revision: compact failure metadata is retained, candidate artifacts are removed, startup reports `latest_incompatible`, and that commit is not retried until upstream HEAD changes or its failure record is explicitly cleared.

Previously verified cached revisions preserve lazy model loading. A narrow shared loaded-model lifecycle owns concurrency-safe lazy access, activation handoff, retention, and idle unloading for all model-backed extractors. Artifact acquisition, model construction, device policy, validation, labels, thresholds, and candidate decoding remain backend or artifact-service responsibilities.

The VAGO backend consumes upstream `pytorch_model.bin` directly through the pinned Candle `VarBuilder::from_pth` API. Tensor-prefix and tensor-name adaptation remain local to that backend; the artifact service does not generate or retain a derived safetensors copy. VAGO runs in F32 on CPU and Metal, with no public dtype setting. `GLINER_DEVICE=auto` may fall back from Metal construction or activation failure to CPU with an explicit diagnostic and an effective-device fingerprint; explicit `metal` fails instead of falling back.

Release-known VAGO revisions require exact Python-versus-Candle semantic parity on Russian, English, and mixed RU/EN fixtures: entity text, character spans, labels, normalized ordering, and accepted-candidate set must match; confidence scores use a documented numeric tolerance. An unseen latest revision may activate only after the installed binary's embedded RU/EN regression corpus passes. Fingerprints distinguish `release_parity_verified` from `runtime_regression_verified`; runtime validation never claims Python parity for an unseen revision. Russian and English are the supported/evaluated languages. Other languages remain best-effort and are not rejected by runtime language detection.

Confidence comparisons use one absolute `1e-4` tolerance across F32 CPU and Metal execution; structural outputs remain exact.

### Extractor identity and historical projections

Every model-backed runtime and new extraction records an extractor fingerprint containing the public selector, resolved backend, repository, resolved revision, artifact identity, normalized ordered labels, effective threshold, validation status, and relevant runtime/model-family version. Canonical default labels for Russian and English are `person`, `company`, `location`, `product`, `event`, and `technology`; configured labels are trimmed, lowercased, and deduplicated in first-declared order. Each model-backed extractor owns an evaluated default threshold because scores are not assumed to be calibrated across architectures; an explicit finite in-range `NER_THRESHOLD` overrides it. Activating a newer revision affects future extraction only. Historical episodes are not automatically re-extracted; a historical re-extraction facility would require a separate migration design.

## Consequences

- Zero configuration remains embedded, lightweight Anno, offline, and download-free.
- The public configuration has one truthful extractor selector and no provider/model precedence matrix.
- Invalid backend/checkpoint/tuning combinations are structurally reduced and rejected at the configuration boundary.
- The exact VAGO checkpoint is a separate backend because its LFM2 architecture and artifact contract are not interchangeable with classic DeBERTa GLiNER.
- The ordinary artifact gains ONNX Runtime packaging cost and must pass Linux, macOS, and Windows loading and distribution gates.
- Explicit model-backed startup may perform network access and block while showing progress.
- Following mutable upstream HEAD means two startups can activate different revisions; fingerprints and last-known-good recovery make that difference observable but do not make outputs revision-invariant.
- Model updates do not silently rewrite historical entity projections.

## Related decisions

- ADR-0029: registry of pluggable NER backends;
- ADR-0030: output-only `init` remains network-free and does not prepare models;
- ADR-0034: allocator and accelerator default policy;
- ADR-0035: native GLiNER lazy loading and idle unloading.
