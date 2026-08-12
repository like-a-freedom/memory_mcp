# Zero-Config Runtime Onboarding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce clean-machine time-to-value to a first successfully recalled fact in five minutes or less by making the ordinary release binary useful with no prerequisite configuration, while retaining advanced capabilities as runtime environment overrides.

**Architecture:** Ship one ordinary, extractor-capable `memory_mcp` binary. `SurrealConfig::from_env()` owns safe local-first defaults; the same runtime reads orthogonal environment variables when an operator explicitly selects remote storage, a model-backed entity extractor, external embeddings, or hardware acceleration. Onboarding documentation uses progressive disclosure: the release-binary quick start contains only install, host registration, and first-value steps; advanced runtime configuration follows in a separate section. ADR-0036 replaces the NER provider/model pair with one typed extractor selection and one shared model-artifact lifecycle.

**Tech Stack:** Rust 2024, MSRV Rust 1.88, Cargo workspace, `clap` 4.6, Tokio, SurrealDB 3.0 with embedded RocksDB, Anno with explicit ONNX support, ONNX Runtime, Candle, `hf-hub`, `tokenizers`, serde/serde_json, GitHub Actions release artifacts, and the existing shell/Python TTV harness.

## Global Constraints

- Zero-config is a runtime onboarding contract, not a build profile or reduced product edition.
- A casual user must not choose Cargo features, a database, a provider, a model, credentials, or a configuration file before first value.
- “Zero-config” means no prerequisite application configuration, external database, external service, API key, administrator action, or first-run model download. It does not mean the Rust binary has no software dependencies.
- Ship and document one ordinary release artifact per supported platform. Do not create, compare, publish, or require an alternate onboarding artifact.
- The ordinary artifact must retain embedded SurrealDB, lightweight Anno, regex NER, explicit Anno ONNX NuNER, both catalogued native Candle GLiNER extractors, local Candle embeddings, OpenAI-compatible embeddings, Ollama embeddings, remote SurrealDB, and existing platform acceleration capabilities.
- Cargo features remain valid only for genuine platform/build concerns already present in the project (`accelerate`, `metal`, `cli-watch`, `mcp-apps`, `prometheus`, `mimalloc`, and `eval-support`). They must not select the normal provider experience. The existing `mcp-apps` feature remains an optional interactive UI surface and is not required for the eight core tools or first value.
- With all application configuration variables absent, defaults are embedded RocksDB, database `memory`, namespace `main`, embedded credentials `root/root`, Anno NER, disabled embeddings, and immediate lexical/graph retrieval.
- The no-environment first-value path must not access the network or download a model.
- Only `ws`, `wss`, `http`, and `https` `SURREALDB_URL` schemes select remote mode. Remote mode requires non-empty explicit `SURREALDB_USERNAME` and `SURREALDB_PASSWORD` and fails before connection when configuration is incomplete.
- Fresh local state uses `$XDG_DATA_HOME/memory_mcp`, then `$HOME/.local/share/memory_mcp`, then the existing deterministic current-directory fallback. Existing legacy data follows the already-implemented compatibility rule.
- `memory_mcp init` remains output-only and supports exactly `vscode`, `claude-desktop`, `codex`, `zed`, and `env`.
- The public MCP surface remains exactly eight tools.
- Keep `main.rs` limited to CLI parsing and mode dispatch; configuration behavior belongs in `src/config/`, service construction in `src/service/`, and release behavior in the existing workflow.
- Production code must not use `unwrap()`.
- Do not add new MCP tools. The scoped Anno ONNX/ONNX Runtime dependency change is approved by ADR-0036 and this plan; any other dependency addition requires separate approval under `AGENTS.md`.
- Preserve unrelated working-tree changes. Reconcile only the rejected uncommitted build experiment and the documentation it affected.
- Before shipping, run `cargo fmt --all --check` and `cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings`.

### Canonical Runtime Environment Variables

Use these exact spellings in code, tests, generated snippets, README tables, and TTV isolation. Do not introduce aliases in documentation even where parsing preserves historical compatibility aliases.

| Group | Canonical variables |
|---|---|
| Storage | `SURREALDB_URL`, `SURREALDB_EMBEDDED`, `SURREALDB_DB_NAME`, `SURREALDB_NAMESPACE`, `SURREALDB_USERNAME`, `SURREALDB_PASSWORD`, `SURREALDB_DATA_DIR`, `SURREALDB_EMBEDDING_DIMENSION` |
| Embeddings | `EMBEDDINGS_ENABLED`, `EMBEDDINGS_PROVIDER`, `EMBEDDINGS_MODEL`, `EMBEDDINGS_MODEL_DIR`, `EMBEDDINGS_BASE_URL`, `EMBEDDINGS_API_KEY`, `EMBEDDINGS_TIMEOUT_SECS`, `EMBEDDINGS_MAX_TOKENS`, `EMBEDDINGS_SIMILARITY_THRESHOLD` |
| NER | `NER_EXTRACTOR`, `NER_CACHE_DIR`, `NER_LABELS`, `NER_THRESHOLD`, `NER_MAX_CONCURRENCY`, `NER_IDLE_UNLOAD_SECS`, `GLINER_BATCH_SIZE`, `GLINER_MAX_BATCH_TOKENS`, `GLINER_DEVICE` |
| Logging/query analytics | `RUST_LOG`, `QUERY_LOGGING_ENABLED`, `QUERY_LOG_RETENTION_DAYS` |
| Lifecycle | `LIFECYCLE_ENABLED`, `LIFECYCLE_DECAY_INTERVAL_SECS`, `LIFECYCLE_ARCHIVAL_INTERVAL_SECS`, `LIFECYCLE_DECAY_THRESHOLD`, `LIFECYCLE_ARCHIVAL_AGE_DAYS`, `LIFECYCLE_DECAY_HALF_LIFE_DAYS` |
| Claims | `MEMORY_CLAIM_ROLLOUT_STAGE`, `MEMORY_CLAIM_CANDIDATE_PAGE_SIZE`, `MEMORY_CLAIM_INLINE_CANDIDATE_LIMIT`, `MEMORY_CLAIM_INLINE_BUDGET_MS` |
| Other tuning | `ENTITY_FUZZY_THRESHOLD` |
| Harness isolation only | `HOME`, `XDG_DATA_HOME`, `CARGO_HOME` |

Canonical documented extractor/provider values are:

- `NER_EXTRACTOR`: `anno`, `regex`, `anno-onnx`, `urchade/gliner_multi-v2.1`, or `VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER`; unset means lightweight `anno`.
- `EMBEDDINGS_PROVIDER`: `local-candle`, `openai-compatible`, `ollama`.
- `GLINER_DEVICE`: `cpu`, `metal`, `auto`.

`NER_PROVIDER`, `NER_MODEL`, `NER_MODEL_DIR`, `NER_BATCH_SIZE`, `NER_MAX_BATCH_TOKENS`, `NER_DEVICE`, `GLINER_IDLE_UNLOAD_SECS`, and `SURREALDB_NAMESPACES` are removed names and must produce migration guidance rather than aliases. `SURREALDB_NAMESPACE` selects the one Active Namespace for a server process and defaults to `main` when absent, as decided by ADR-0038. `SURREALDB_EMBEDDING_DIMENSION` remains canonical because it is the existing public name, even though `EmbeddingConfig::from_env()` consumes it. `NER_IDLE_UNLOAD_SECS` defaults to `0` and applies only to model-backed extractors.

## Evidence and Design Rationale

- [SQLite Zero-Configuration](https://sqlite.org/zeroconf.html): zero configuration removes setup procedures, server administration, and configuration files before use. This plan applies that principle to embedded local storage.
- [Nielsen Norman Group: Progressive Disclosure](https://www.nngroup.com/articles/progressive-disclosure/): novices should see only the few important choices; specialized options should remain available on request. This determines README order.
- [Ink & Switch: Local-first software](https://www.inkandswitch.com/essay/local-first/): local state gives users ownership and avoids dependence on a remote service for useful operation. This determines the default storage path.
- [The Twelve-Factor App: Config](https://12factor.net/config): deployment-varying configuration belongs in orthogonal environment variables rather than artifact-specific configuration files. This determines the power-user override mechanism.
- [The Cargo Book: Features](https://doc.rust-lang.org/cargo/reference/features.html): Cargo features are conditional compilation and optional-dependency controls. Therefore they are not runtime onboarding settings and must not define the normal user experience.

## Verified Repository Baseline — 2026-08-07

Tasks 1–9 from the earlier plan are committed at `97a3edd8` and are prerequisites, not work to repeat. ADR-0038 and the follow-on plan `2026-08-12-one-active-namespace.md` amend only the namespace/scope contract on top of this baseline:

1. User-owned embedded data directory and legacy compatibility selection.
2. No-environment embedded defaults in `SurrealConfig::from_env()`.
3. Actionable configuration-error envelopes.
4. ADR authorization for the output-only `init` command.
5. Frozen CLI surface update.
6. Deterministic, non-mutating host snippet renderers.
7. Configuration-owned storage backend terminology.
8. Release binary smoke tests and installation documentation.
9. TTV measurement harness with real fact-recall validation.

Measured evidence already distinguishes runtime onboarding from distribution cost:

- The ordinary release-binary application path measured approximately `0.487s` median.
- A clean Rust-source install measured `544.098s`, with `542.634s` spent compiling/installing.
- The correct response to source-build latency is prebuilt releases, checksums, and clear install instructions—not a second runtime product.

The working tree contains an uncommitted rejected build experiment in the files listed below. Task 10 must reconcile those edits carefully; do not use a broad reset because the plan file and any later user edits must be preserved.

## File Map for Remaining Work

| File | Responsibility |
|---|---|
| `crates/memory-mcp/Cargo.toml` | Keep local ML dependencies mandatory, enable Anno ONNX in the ordinary package, and retain platform/tooling features only for genuine build concerns. |
| `crates/memory-mcp/src/config/ner.rs` | Replace the flat provider/model configuration with the discriminated extractor configuration and reject removed or irrelevant settings. |
| `crates/memory-mcp/src/service/entity_extraction/artifacts.rs` | Shared revision resolution, progress, staging, verification, activation, quarantine, and last-known-good artifact lifecycle. |
| `crates/memory-mcp/src/service/entity_extraction/anno_onnx.rs` | Explicit CPU NuNER backend for the stable `anno-onnx` alias; no implicit Anno downloads or fallback. |
| `crates/memory-mcp/src/service/entity_extraction/lfm2_gliner.rs` | Exact native Candle backend for `VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER`. |
| `crates/eval-harness/Cargo.toml` | Keep benchmarks available under their existing feature contract; do not forward an onboarding build feature. |
| `crates/memory-mcp/src/service.rs` | Keep the ordinary public service re-exports available. |
| `crates/memory-mcp/src/service/embedding.rs` | Keep local Candle provider construction in the ordinary binary. |
| `crates/memory-mcp/src/service/entity_extraction.rs` | Keep the complete NER registry and GLiNER provider in the ordinary binary. |
| `crates/memory-mcp/src/service/startup.rs` | Preserve the committed startup fallback/error behavior unless a failing ordinary-runtime test proves a separate bug. |
| `crates/memory-mcp/tests/local_model_integration.rs` | Compile the local-model integration suite in the ordinary test artifact. |
| `crates/memory-mcp/tests/zero_config_embedded.rs` | Prove the ordinary no-environment first-value path remains local and useful. |
| `crates/memory-mcp/src/config/surreal.rs` | Extend focused environment-default and explicit-override tests using the existing shared environment lock/snapshot. |
| `.github/workflows/ci.yml` | Build, smoke-test, checksum, and upload one ordinary artifact per supported target. |
| `README.md` | Put release install and no-configuration first value first; move runtime overrides into clearly grouped advanced sections. |
| `docs/agent_integration/CONTRACT.md` | Keep the agent integration contract aligned with the one-artifact runtime model. |
| `scripts/measure_ttv.sh` | Continue measuring the same ordinary release artifact users download. |
| `crates/memory-mcp/tests/slim_feature_errors.rs` | Delete the untracked experiment-only test; it tests a product mode that must not exist. |
| `crates/memory-mcp/src/config/helpers.rs` | Preserve the committed behavior; keep or drop the uncommitted formatting-only rewrite according to `cargo fmt`, without treating it as product behavior. |

---

# Stage 7 — Preserve One Full-Capability Artifact and Runtime Progressive Disclosure

> **Historical implementation stage:** completed by the verified baseline above. Its code snippets preserve the provider-shaped API that existed when this stage was executed. ADR-0036 and Stage 8 supersede those snippets for future NER configuration work; they do not invalidate the one-artifact decision.

### Task 10: Reconcile the rejected build experiment and lock the runtime contract

**Files:**
- Modify: `crates/memory-mcp/Cargo.toml`
- Modify: `crates/eval-harness/Cargo.toml`
- Modify: `crates/memory-mcp/src/service.rs`
- Modify: `crates/memory-mcp/src/service/embedding.rs`
- Modify: `crates/memory-mcp/src/service/entity_extraction.rs`
- Modify: `crates/memory-mcp/src/service/startup.rs` only to remove experiment-specific behavior not present at `97a3edd8`
- Modify: `crates/memory-mcp/tests/local_model_integration.rs`
- Modify: `crates/memory-mcp/src/config/surreal.rs`
- Modify: `.github/workflows/ci.yml`
- Modify: `README.md`
- Modify: `docs/agent_integration/CONTRACT.md`
- Delete: `crates/memory-mcp/tests/slim_feature_errors.rs`
- Test: existing config, provider, zero-config, local-model, release-smoke, and TTV tests

**Interfaces:**
- Consumes: `SurrealConfig::from_env() -> Result<SurrealConfig, MemoryError>`, `EmbeddingConfig::from_env() -> Result<EmbeddingConfig, MemoryError>`, `NerConfig::from_env() -> Result<NerConfig, MemoryError>`, the existing service provider factories, and the committed release workflow.
- Produces: one ordinary artifact whose absent-environment path uses embedded/Anno/lexical defaults and whose explicit canonical environment variables activate the already-supported advanced providers.

- [ ] **Step 1: Capture the implementation boundary before editing.**

Run:

```bash
git --no-optional-locks status --short
git --no-pager diff -- \
  .github/workflows/ci.yml \
  crates/eval-harness/Cargo.toml \
  crates/memory-mcp/Cargo.toml \
  crates/memory-mcp/src/config/helpers.rs \
  crates/memory-mcp/src/service.rs \
  crates/memory-mcp/src/service/embedding.rs \
  crates/memory-mcp/src/service/entity_extraction.rs \
  crates/memory-mcp/src/service/startup.rs \
  crates/memory-mcp/tests/local_model_integration.rs \
  crates/memory-mcp/tests/slim_feature_errors.rs
```

Expected: only the rejected build experiment plus the formatting-only `helpers.rs` change appears in these paths. Stop and classify any additional diff before editing. Do not reset `docs/superpowers/plans/2026-08-04-zero-config-defaults.md` or any unrelated user file.

- [ ] **Step 2: Add focused configuration tests and expose the failing ordinary-provider compile contract.**

In `crates/memory-mcp/src/config/surreal.rs`, use the existing `env_lock()`, `EnvSnapshot`, `SURREAL_CONFIG_ENV_KEYS`, and `clear_surreal_environment()` helpers. Add these tests next to `from_env_applies_zero_config_embedded_defaults`:

```rust
#[test]
fn ordinary_runtime_defaults_to_local_first_without_provider_selection() {
    let _lock = env_lock().lock().expect("environment lock");
    let _snapshot = EnvSnapshot::capture(SURREAL_CONFIG_ENV_KEYS);
    clear_surreal_environment();

    let config = SurrealConfig::from_env().expect("zero-config defaults");

    assert!(config.embedded);
    assert_eq!(config.db_name, "memory");
    assert_eq!(config.namespaces, vec!["org"]);
    assert_eq!(config.username.as_deref(), Some("root"));
    assert_eq!(config.password.as_deref(), Some("root"));
    assert_eq!(config.ner.provider, NerProviderKind::Anno);
    assert_eq!(config.embedding.provider, EmbeddingProviderKind::Disabled);
}

#[test]
fn ordinary_runtime_accepts_canonical_advanced_provider_overrides() {
    let _lock = env_lock().lock().expect("environment lock");
    let _snapshot = EnvSnapshot::capture(SURREAL_CONFIG_ENV_KEYS);
    clear_surreal_environment();
    unsafe {
        env::set_var("NER_PROVIDER", "local-gliner");
        env::set_var("EMBEDDINGS_ENABLED", "true");
        env::set_var("EMBEDDINGS_PROVIDER", "local-candle");
    }

    let config = SurrealConfig::from_env().expect("canonical provider overrides");

    assert_eq!(config.ner.provider, NerProviderKind::LocalGliner);
    assert_eq!(config.embedding.provider, EmbeddingProviderKind::LocalCandle);
}
```

Import `EmbeddingProviderKind` and `NerProviderKind` through the existing `super::*`/module imports rather than creating duplicate public paths. If equivalent assertions already exist after rebasing, consolidate rather than duplicate them.

Run:

```bash
cargo test -p memory_mcp config::surreal::tests::ordinary_runtime_defaults_to_local_first_without_provider_selection --locked
cargo test -p memory_mcp config::surreal::tests::ordinary_runtime_accepts_canonical_advanced_provider_overrides --locked
```

Expected before reconciliation: both configuration tests may pass because provider enums and environment parsing still exist even when implementations are conditionally absent. Retain them as runtime naming/default regression tests; do not misrepresent them as proof that provider implementations are linked.

To establish the red provider-availability test, remove only the experiment-added crate-level `#![cfg(feature = "ml")]` line from `crates/memory-mcp/tests/local_model_integration.rs`, then run:

```bash
cargo test -p memory_mcp --test local_model_integration --locked
```

Expected before Step 4: compilation fails because the experiment conditionally removed ordinary local-model modules or re-exports. If it unexpectedly compiles and passes, inspect the binary-facing provider factory and registry before proceeding; do not manufacture a failure. The acceptance condition is that this existing integration suite compiles and runs against the ordinary artifact without any provider-selection build flag.

- [ ] **Step 3: Restore the ordinary package dependency and feature contract surgically.**

In `crates/memory-mcp/Cargo.toml`:

- Keep `candle-core.workspace = true`, `candle-nn.workspace = true`, `candle-transformers.workspace = true`, `hf-hub.workspace = true`, and `tokenizers.workspace = true` as ordinary dependencies.
- Keep `[features] default = []`.
- Keep the existing `accelerate`, `metal`, `cli-watch`, `mcp-apps`, `prometheus`, `mimalloc`, and `eval-support` definitions exactly aligned with the committed package contract.
- Do not add a feature whose purpose is selecting ordinary versus reduced application behavior.

In `crates/eval-harness/Cargo.toml`, remove experiment-only feature forwarding and restore `ner_cpu` to its ordinary benchmark definition:

```toml
[[bench]]
name = "ner_cpu"
harness = false
```

Run:

```bash
cargo check -p memory_mcp --all-targets --locked
cargo check -p eval-harness --all-targets --locked
```

Expected: both pass with the ordinary dependency graph and no onboarding feature flags.

- [ ] **Step 4: Restore unconditional provider modules, registry entries, and make the ordinary-provider test green.**

Remove only experiment-added conditional-compilation guards from:

- `service::model_loader` and its consumers.
- `service::embedding::local`, local provider imports, local provider dispatch, and local provider tests.
- `service::entity_extraction::gliner`, `GlinerEntityExtractor` re-export, the `LocalGliner` registry entry, and GLiNER tests.
- `tests/local_model_integration.rs` at the crate root.

The ordinary service exports must again include:

```rust
pub use entity_extraction::{
    AnnoEntityExtractor, EntityExtractor, GlinerEntityExtractor, LlmEntityExtractor,
    RegexEntityExtractor, create_entity_extractor,
};
```

Keep descriptive provider errors that are useful independently of the rejected experiment only if existing ordinary-provider tests demonstrate them. Restore `resolve_embedding_startup` to committed behavior unless a separately failing test proves that explicit invalid runtime configuration is swallowed; do not bundle an unproven semantic change into onboarding reconciliation.

Delete `crates/memory-mcp/tests/slim_feature_errors.rs` because no supported runtime mode corresponds to it.

Run:

```bash
cargo test -p memory_mcp service::entity_extraction::tests --locked
cargo test -p memory_mcp service::embedding::tests --locked
cargo test -p memory_mcp --test local_model_integration --locked
cargo test -p memory_mcp config::surreal::tests::ordinary_runtime_accepts_canonical_advanced_provider_overrides --locked
```

Expected: all tests compile and pass without an onboarding feature argument.

- [ ] **Step 5: Make CI build and publish exactly the ordinary artifact.**

In `.github/workflows/ci.yml`:

- Remove the experiment-only feature-matrix job and its dependency from `build_binaries.needs`.
- Keep the existing release matrix: Linux x86_64, macOS x86_64, macOS arm64, and Windows x86_64.
- Keep the existing native `--version` and `init --target vscode` smoke tests.
- Build the release artifact with exactly:

```yaml
- name: Build binary
  run: cargo build -p memory_mcp --release --target ${{ matrix.platform.target }} --locked
```

- Keep checksum generation and artifact upload pointed at that same binary.
- Do not add a comparison build, separate target directory, alternate artifact name, or release asset.

Validate workflow structure with the repository’s existing YAML tooling if available, then inspect the release job diff:

```bash
git --no-pager diff --check -- .github/workflows/ci.yml
git --no-pager diff -- .github/workflows/ci.yml
```

Expected: one build command feeds smoke test, checksum, and upload for each target.

- [ ] **Step 6: Rewrite README onboarding with two-level progressive disclosure.**

In `README.md`, make `## Quick start` present this order:

1. Download the matching prebuilt release artifact and verify its checksum.
2. Put `memory_mcp` on `PATH`.
3. Run `memory_mcp init --target <host>` and copy the emitted snippet.
4. Start/use the configured MCP server.
5. Verify first value with ingest → extract → assemble-context.

The primary path must not mention Cargo features, provider selection, model directories, API keys, or an external database. Keep source installation as a secondary fallback and state that it compiles the same ordinary application:

```bash
cargo install --path crates/memory-mcp --locked
```

Immediately after the quick start, state the default behavior in plain language:

- Data stays in the user-owned local data directory.
- Embedded storage starts without a separate server.
- Anno extraction and lexical/graph retrieval work immediately.
- Embeddings are off until explicitly enabled.
- No API key, configuration file, network request, or model download is required for first value.

Under `## Configuration`, group advanced overrides in this order:

1. Remote SurrealDB.
2. Local GLiNER.
3. Local Candle embeddings.
4. OpenAI-compatible embeddings.
5. Ollama embeddings.
6. Hardware/runtime tuning.

Use only names from the canonical inventory and only canonical provider values. Show remote credentials as shell placeholders, never defaults. Make clear that `EMBEDDINGS_API_KEY` is relevant to the selected external provider and is not required for local defaults.

Remove wording that implies users should build a different application artifact for ordinary local use. Keep platform acceleration documentation because those are genuine target/build capabilities, but place it under advanced development/deployment documentation rather than first-run setup.

- [ ] **Step 7: Align the agent contract and TTV interpretation.**

In `docs/agent_integration/CONTRACT.md`, state:

- The installed executable works with absent application configuration.
- `init` renders host configuration but does not mutate files.
- Runtime environment overrides select advanced providers in the same executable.
- Remote credentials are required only when remote storage is explicitly selected.
- The eight-tool MCP surface is unchanged.

In the README measurement section, retain the three existing personas but interpret them correctly:

- `release-binary`: primary user onboarding metric.
- `host-config-user`: host snippet preparation plus the same ordinary executable.
- `rust-user`: source-distribution/build metric, not a different runtime product.

Do not change `scripts/measure_ttv.sh` unless its build command differs from the ordinary `cargo install --path crates/memory-mcp --locked` contract.

- [ ] **Step 8: Run focused runtime and release-artifact validation.**

Run:

```bash
cargo fmt --all --check
cargo test -p memory_mcp config:: --locked
cargo test -p memory_mcp init::tests --locked
cargo test -p memory_mcp --test zero_config_embedded --locked
cargo test -p memory_mcp --test local_model_integration --locked
cargo test -p memory_mcp --test agent_memory_lifecycle_release_gate --locked
cargo build --release --locked -p memory_mcp
./target/release/memory_mcp --version
./target/release/memory_mcp init --target vscode
bash -n scripts/measure_ttv.sh
bash -n scripts/test_measure_ttv.sh
bash scripts/test_measure_ttv.sh
scripts/measure_ttv.sh --binary ./target/release/memory_mcp --persona release-binary --repeat 1
```

Expected:

- All tests pass without an onboarding feature argument.
- The same `target/release/memory_mcp` binary passes provider compilation, host rendering, and no-environment first recall.
- The TTV validator rejects malformed, missing-result, empty-fact, and fallback-only fixtures.
- The release-binary sample recalls a real fact and does not report `episode_fallback:` as success.

- [ ] **Step 9: Run the mandatory project gate.**

Run:

```bash
cargo check --workspace --all-targets --locked
cargo test --workspace --lib --bins --tests --locked
cargo test -p memory_mcp --features mcp-apps --locked
cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings
cargo fmt --all --check
git --no-pager diff --check
```

Expected: every command passes with zero warnings and zero formatting diff.

- [ ] **Step 10: Review the final diff for scope and naming consistency.**

Run:

```bash
git --no-optional-locks status --short
git --no-pager diff --stat
git --no-pager diff -- \
  crates/memory-mcp/Cargo.toml \
  crates/eval-harness/Cargo.toml \
  crates/memory-mcp/src/service.rs \
  crates/memory-mcp/src/service/embedding.rs \
  crates/memory-mcp/src/service/entity_extraction.rs \
  crates/memory-mcp/src/service/startup.rs \
  crates/memory-mcp/src/config/surreal.rs \
  crates/memory-mcp/tests/local_model_integration.rs \
  .github/workflows/ci.yml \
  README.md \
  docs/agent_integration/CONTRACT.md
```

Review requirements:

- No alternate onboarding build feature or artifact remains.
- No ordinary provider module or integration test is conditionally absent.
- Every documented environment variable matches the canonical inventory exactly.
- Every documented provider value uses its canonical hyphenated spelling.
- The release workflow builds, tests, checksums, and uploads the same artifact.
- README first-run steps require no environment variables.
- Explicit remote configuration still fails fast when credentials are absent.
- No unrelated user changes are included.

- [ ] **Step 11: Commit the corrected product contract.**

Stage only reviewed files; do not use `git add -A`:

```bash
git add \
  crates/memory-mcp/Cargo.toml \
  crates/eval-harness/Cargo.toml \
  crates/memory-mcp/src/service.rs \
  crates/memory-mcp/src/service/embedding.rs \
  crates/memory-mcp/src/service/entity_extraction.rs \
  crates/memory-mcp/src/service/startup.rs \
  crates/memory-mcp/src/config/surreal.rs \
  crates/memory-mcp/tests/local_model_integration.rs \
  .github/workflows/ci.yml \
  README.md \
  docs/agent_integration/CONTRACT.md

git commit -m "fix: preserve one zero-config runtime artifact"
```

Do not stage `docs/superpowers/plans/2026-08-04-zero-config-defaults.md` unless the user explicitly asks to include planning documents in the implementation commit. The deleted experiment-only test is untracked, so remove it from the working tree rather than attempting to stage its deletion.

---

# Stage 8 — Unified NER Extractor Catalog and Native LFM2 Support

> **Decision source:** ADR-0036. **Implemented 2026-08-08** by `docs/superpowers/plans/2026-08-07-unified-ner-extractors-and-vago-lfm2.md` (Tasks 1–12). The work packages below are recorded as the design record; the implementation plan carries the exact red-green steps, commands, assertions, and staging boundaries against the then-current codebase.

### Task 11: Replace the provider/model pair with one typed extractor selection

- Replace the flat `NerConfig { provider, model, ... }` representation with a discriminated extractor configuration whose variants carry only supported controls.
- Parse one closed `NER_EXTRACTOR` catalog: unset/`anno`, `regex`, `anno-onnx`, `urchade/gliner_multi-v2.1`, and `VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER`.
- Remove `NER_PROVIDER` and `NER_MODEL`; fail with migration guidance when either is present.
- Rename `NER_MODEL_DIR` to `NER_CACHE_DIR`, `NER_BATCH_SIZE` to `GLINER_BATCH_SIZE`, `NER_MAX_BATCH_TOKENS` to `GLINER_MAX_BATCH_TOKENS`, `NER_DEVICE` to `GLINER_DEVICE`, and `GLINER_IDLE_UNLOAD_SECS` to the model-backed `NER_IDLE_UNLOAD_SECS`. Removed names fail rather than alias or silently disappear.
- Reject every explicit setting that is irrelevant to the selected extractor.
- Keep unset `NER_EXTRACTOR` equivalent to lightweight Anno with no network access or model acquisition.
- Update typed builder APIs, registry tests, integration fixtures, eval benches, generated environment inventory, README, and agent documentation in the same change.

### Task 12: Introduce one shared model-artifact lifecycle

- Extract the current classic GLiNER acquisition policy behind a model-artifact service rather than extending global filename constants.
- Default the artifact root to `<resolved memory_mcp data directory>/models/ner`; let `NER_CACHE_DIR` override only that root and never clean the global Hugging Face cache.
- Let each model-backed extractor own its repository and artifact requirements while the shared service owns latest-revision lookup, revision-specific staging, integrity checks, atomic activation, quarantine, and persisted last-known-good state.
- Resolve upstream HEAD at every model-backed startup. If upstream is unavailable, use a complete verified cached revision with `revision_status=unverified_latest`; fail when no usable revision exists.
- Download during startup, not first extraction. Drive interactive CLI rendering and schema-versioned JSON Lines MCP events from one progress stream on stderr; never write progress into MCP stdout. Emit on phase changes, completion/failure, crossed 5% boundaries, or five seconds without another event.
- Prevent backend libraries, including Anno, from downloading implicitly.
- Retain the active and one previous known-good revision per extractor. Remove older good revisions only after successful activation; remove incompatible artifacts after persisting compact failure metadata.
- Coordinate shared-cache startup with a standard-library atomic filesystem lease, heartbeat, waiter progress, conservative stale recovery, and process-unique staging directories.
- Resolve latest revision with two attempts and short backoff under one 10-second deadline. Permit downloads while bytes advance, but fail after a 60-second progress stall.
- Probe every new candidate by constructing the model and running a fixed smoke inference. Promote only on success and retain the loaded candidate for first real extraction. On incompatibility, remove candidate artifacts, persist commit-keyed failure metadata, report `latest_incompatible`, and continue with the previous known-good revision when available.
- Extract a narrow shared loaded-model lifecycle for lazy access, concurrency, activation handoff, retention, and idle unloading. Default `NER_IDLE_UNLOAD_SECS` to `0`; positive values apply to every model-backed extractor. Keep acquisition, construction, device policy, validation, labels, thresholds, and decoding outside this abstraction.

### Task 13: Add explicit Anno ONNX NuNER without changing the default

- Enable Anno's `onnx` capability in the ordinary artifact; do not enable it through an onboarding Cargo feature.
- Keep unset/`anno` on the explicit lightweight pattern-and-heuristic path. Do not call a dynamic `StackedNER::default()` that can select cached ML backends.
- Make `anno-onnx` resolve only to Anno's NuNER ONNX backend and `numind/NuNER_Zero`; do not fall back to BERT, GLiNER, or heuristics.
- Use only artifacts prepared by the shared artifact service and construct NuNER from a local path with Anno downloads disabled.
- Support CPU only in the first implementation. Reject `GLINER_DEVICE`, ONNX CoreML, CUDA, and every other irrelevant native-GLiNER setting.
- Add model-backed parity, label mapping, deterministic ordering, startup acquisition, offline-cache, incompatible-revision, and no-fallback tests.
- Measure entity quality against lightweight Anno and regex. Full Anno ONNX must improve the declared quality slice or remain an explicit experimental catalog entry rather than receiving default status.
- Use canonical lowercase English output labels for RU and EN. Normalize configured labels by trim/lowercase/stable deduplication, and use an evaluated NuNER-specific default threshold rather than assuming GLiNER score calibration.

### Task 14: Add the exact VAGO LFM2 GLiNER backend

- Add a distinct backend for exactly `VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER`; do not branch throughout the classic DeBERTa GLiNER implementation based on repository names.
- Implement the checkpoint's `Lfm2BiModel` semantics natively in Candle, including bidirectional attention, symmetric center-padding convolutions, layer fusion, and its tokenizer contract. Load upstream `pytorch_model.bin` directly through the pinned Candle `VarBuilder::from_pth`; keep tensor-name adaptation backend-local and do not create a derived safetensors cache.
- Reuse only proven operational seams: `EntityExtractor`, backend registry construction, shared artifact lifecycle, device policy, inference concurrency gate, lazy loaded-model lifecycle, and `EntityCandidate` normalization.
- Keep backend-owned artifact requirements separate from the classic GLiNER manifest. Do not generalize into a multi-checkpoint LFM2 framework until another real checkpoint proves a shared abstraction.
- Run F32 on CPU and Metal without a public dtype setting. For `GLINER_DEVICE=auto`, report Metal failure and fall back to CPU; explicit `metal` fails without fallback. Fingerprint the effective device.
- Add tensor-layout, tokenizer, smoke-inference, candidate-parity, Russian, English, mixed RU/EN, batching, concurrency, CPU, optional Metal, idle-unload, and last-known-good activation tests.
- For release-known revisions, require exact Python-versus-Candle entity text, character span, label, normalized order, and accepted-set parity. For unseen latest revisions, require the same exact structural outputs from the embedded RU/EN regression corpus and report `runtime_regression_verified` rather than claiming Python parity. Permit confidence-score differences only within absolute tolerance `1e-4`.
- Treat RU and EN as the supported/evaluated languages; process other languages best-effort without adding a language detector or rejection path.

### Task 15: Record extractor fingerprints without automatic historical re-extraction

- Record selector, resolved backend, repository, resolved revision, artifact identity, normalized ordered labels, backend-owned effective threshold, validation status, and relevant runtime/model-family version for every new model-backed extraction projection.
- Make revision status observable (`latest`, `unverified_latest`, or `latest_incompatible` with the retained active revision) and validation status observable (`release_parity_verified` or `runtime_regression_verified`).
- Apply newly activated revisions only to future extraction. Do not schedule or block on historical re-extraction.
- Treat any future historical re-extraction command or background migration as separate scope requiring its own data-migration and audit design.

### Task 16: Distribution, quality, and operational gates

- Prove that the ordinary release artifact contains lightweight Anno, regex, Anno ONNX NuNER, classic Candle GLiNER, and VAGO LFM2 GLiNER on every supported target.
- Record ONNX Runtime binary-size, linking, startup, and platform behavior on Linux, macOS, and Windows.
- Preserve the absent-environment TTV path: no NER network request, no model download, and lightweight Anno extraction.
- Add startup tests for progress-channel safety: CLI and MCP progress use stderr; MCP stdout remains valid JSON-RPC.
- Gate activation on artifact integrity, full construction, smoke inference, candidate-schema parity, existing entity-quality gates, strict clippy, formatting, workspace tests, and release smoke tests.
- Update ADR-0029's backend-addition instructions and all public configuration docs to use extractor selection rather than provider/model terminology.

---

# Final Acceptance Criteria

- Running the ordinary binary with all application configuration variables absent selects embedded RocksDB, database `memory`, namespace `org`, credentials `root/root`, Anno NER, and disabled embeddings.
- The absent-environment first-value flow ingests, extracts, and recalls a real fact without a remote service, API key, model selection, network request, or model download.
- The ordinary artifact contains all existing advanced embedding providers and every catalogued NER extractor.
- `NER_EXTRACTOR=anno-onnx`, `NER_EXTRACTOR=urchade/gliner_multi-v2.1`, `NER_EXTRACTOR=VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER`, and `EMBEDDINGS_PROVIDER=local-candle` are accepted by runtime configuration in the ordinary artifact.
- `EMBEDDINGS_PROVIDER=openai-compatible` and `EMBEDDINGS_PROVIDER=ollama` remain available through the same artifact and use their existing required runtime settings.
- Explicit remote SurrealDB configuration requires non-empty credentials and fails with an actionable configuration error when incomplete.
- Fresh embedded data uses the user-owned XDG/home path and preserves the implemented legacy compatibility rule.
- `memory_mcp init` remains output-only and supports exactly the five authorized hosts.
- The public MCP surface remains eight tools.
- CI produces one ordinary release artifact per supported platform, and the artifact that is smoke-tested is the artifact that is checksummed and uploaded.
- README quick start contains no Cargo feature selection or provider decision.
- Advanced README examples use only canonical environment-variable names and canonical extractor/provider values.
- TTV is measured against the ordinary release artifact; source compilation time is reported separately as distribution cost.
- Full workspace tests, strict clippy, formatting, release build, smoke test, and TTV validation pass.

# Self-Review

## Spec Coverage

- Runtime zero configuration and local-first defaults: completed Tasks 1–2; retained by Task 10 Steps 2 and 8.
- Friendly failure path and host setup: completed Tasks 3–6; retained by Task 10 Steps 6–9.
- One full-capability artifact: Task 10 Steps 3–5 and Stage 8; `mcp-apps` remains an explicitly optional UI feature.
- Runtime progressive disclosure for power users: Task 10 Steps 2, 6, and 7.
- Prebuilt distribution rather than source-build optimization as onboarding: completed Tasks 8–9; corrected interpretation in Task 10 Steps 6–8.
- Environment-variable naming consistency: canonical inventory plus Task 10 Steps 6, 7, and 10.
- Remote credential safety: global constraints, existing Task 2 tests, and final acceptance criteria.
- No first-run model/network dependency: global constraints, existing zero-config integration test, and Task 10 Step 8.
- Mandatory validation and scoped commit: Task 10 Steps 8–11.

## Placeholder Scan

Stage 7 has no unresolved implementation placeholders. Stage 8 intentionally records design-approved work packages; before implementation, those packages must be expanded into exact red-green steps, commands, assertions, and staging boundaries against the then-current codebase.

## Type and Name Consistency

- `SurrealConfig::from_env()` returns the same `SurrealConfig` used by CLI and MCP startup.
- `config.ner` uses a discriminated extractor configuration for lightweight Anno, regex, Anno ONNX NuNER, classic GLiNER, and VAGO LFM2 GLiNER.
- `config.embedding.provider` uses `EmbeddingProviderKind::{Disabled, LocalCandle, OpenAiCompatible, Ollama}`.
- `SURREALDB_EMBEDDING_DIMENSION` is consistently treated as embedding configuration under its existing public spelling.
- NER tuning names reflect scope: shared model-backed settings, including idle unloading, use `NER_*`; native GLiNER-only batching and device settings use `GLINER_*`.
- The release command, local validation command, and TTV binary path all refer to the ordinary `memory_mcp` artifact.

## Contradiction Check

- `default = []` does not mean reduced capability because extractor and embedding-provider dependencies remain ordinary dependencies.
- Platform acceleration features do not contradict runtime zero-config because casual users do not need them for first value.
- Embeddings being available but disabled by default does not contradict full capability; activation is a runtime operator choice.
- Lightweight Anno as the default does not remove model-backed extraction; Anno ONNX NuNER and both native GLiNER extractors remain available through `NER_EXTRACTOR` in the same binary.
- Source compilation exceeding five minutes does not contradict the release-binary TTV goal; they are separate distribution personas and are reported separately.
