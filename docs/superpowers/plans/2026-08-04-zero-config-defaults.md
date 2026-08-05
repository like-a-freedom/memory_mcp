# Zero-Config Defaults and Fast First Value Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce median time from a clean-machine first install to the first successfully recalled fact to five minutes or less by making embedded local operation work without configuration, explaining failures with actionable output, and providing copy-paste host setup.

**Architecture:** Keep `main.rs` thin and make zero-config behavior a configuration-layer concern. `SurrealConfig::from_env()` will supply safe local defaults and infer embedded versus remote operation from the URL scheme; the existing `data_dir_or_default()` method will continue to be the single consumer-facing path resolver, but its helper will move from executable-relative storage to an XDG-conventional user-data directory. CLI onboarding remains a thin adapter: `init` only renders host-specific configuration snippets and never starts services or mutates files. Later stages improve packaging, isolate the existing storage-engine seam, optionally remove the unconditional ML build cost, and measure time-to-value rather than assuming it.

**Tech Stack:** Rust 2024, MSRV Rust 1.88, Cargo workspace, `clap` 4.6, Tokio, SurrealDB 3.0 with embedded RocksDB/in-memory support, serde/serde_json, existing `StdoutLogger`, integration tests under `crates/memory-mcp/tests`, GitHub Actions release artifacts, shell-based TTV measurement.

## Global Constraints

- The default configuration must require no `SURREALDB_*` environment variables for embedded local operation.
- “Zero dependencies” means no external database, service, credential, model download, or host-specific setup is required for the default local path; it does not mean the compiled Rust binary has no crate/runtime dependencies.
- `SURREALDB_URL` schemes `ws`, `wss`, `http`, and `https` mean remote mode; an unset URL or any other scheme means embedded mode unless `SURREALDB_EMBEDDED` explicitly overrides the inference.
- The default database name is `memory`.
- The default namespace list is exactly `vec!["org".into()]`; do not introduce multiple default namespaces.
- Embedded local mode defaults to `root` / `root`; remote mode requires explicit `SURREALDB_USERNAME` and `SURREALDB_PASSWORD` and must never silently attempt remote authentication with those embedded defaults.
- A fresh install must use a user-owned XDG-conventional data path: `$XDG_DATA_HOME/memory_mcp` when `XDG_DATA_HOME` is set, otherwise `$HOME/.local/share/memory_mcp`, with a deterministic current-directory fallback only when neither home variable is available. An existing legacy executable-relative directory may be selected only by the documented compatibility rule.
- Default application must construct one structured `config.default_applied` event per zero-config storage default introduced by this plan, without logging secret values; pre-existing nested lifecycle, embedding, NER, and `RUST_LOG` defaults are outside this event contract.
- The existing `SurrealConfigBuilder::default()` is independent from `SurrealConfig::from_env()`; tests of environment defaults must call `from_env()` directly.
- The existing NER default is `NerProviderKind::Anno`; this plan does not silently change it to regex or claim that NER has zero first-run cost.
- The public MCP surface remains exactly eight tools; the ordinary CLI surface is amended from the current frozen list by exactly one output-only `init` command.
- Adding the public `init` CLI subcommand requires and is blocked on ADR-0029, which explicitly amends the existing ordinary-CLI freeze, plus a live parser/surface test update.
- `init` supports exactly `vscode` (default), `claude-desktop`, `codex`, `zed`, and `env`; it prints snippets and does not edit host files.
- Do not add a runtime dependency for XDG lookup; implement the lookup with `std::env` and `PathBuf` unless a separately approved ADR changes that decision.
- The measurement harness may require POSIX shell and Python 3 from the test environment; these are not application runtime dependencies.
- Do not promise a slim default binary before the ML feature-gating work is implemented and measured; Candle, `hf-hub`, and `tokenizers` are currently unconditional workspace dependencies.
- Production code must not use `unwrap()`; use `Result`, `Option`, and descriptive `MemoryError` values.
- Before shipping, run `cargo fmt --all --check` and `cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings`.

---

## Scope and sequencing

Stages 1–3 are the first release slice and provide the highest time-to-value leverage. Stage 4 is a small internal refactor that clarifies the existing storage seam. Stage 5 packages the result for clean-machine installation. Task 9 in Stage 6 is the required measurement and must be implemented and run immediately after Stage 5; only then may the optional ML feature-gating Task 10 in Stage 7 proceed. Stage 7 is deliberately deferred until that baseline establishes whether the unconditional ML graph is the dominant cold-start cost.

The first release slice must be usable independently:

1. A clean user can run the binary without `SURREALDB_*` configuration.
2. A missing/invalid configuration error contains a next step.
3. A user can run `memory_mcp init --target vscode` and copy the emitted snippet.
4. The frozen CLI and MCP surface tests remain green.

This document is an umbrella roadmap with independently testable slices, not one indivisible change: Tasks 1–6 form the first user-facing release slice; Task 7 is an optional internal seam cleanup; Task 8 is release packaging; Task 9 is the required measurement follow-up; and Task 10 is a conditional ML-build follow-up that must not block the first release. Implement and review each slice independently, and do not merge Task 10 merely because the earlier slices are complete.
Before beginning TDD, run `cargo check --workspace --all-targets --locked` on the clean implementation branch. If the baseline reports extraction-related compile failures across the backend registry/provider modules (including missing provider `build` functions, imports/signature mismatches, or a GLiNER partial-move error), treat all of those as separate baseline defects and do not mix them into this feature’s commits. Keep unrelated user changes untouched. Once the baseline is clean, add or retain the exact `create_entity_extractor_defaults_to_anno` regression test named in Task 2.

---

## File map

| File | Responsibility in this plan |
|---|---|
| `crates/memory-mcp/src/config/helpers.rs` | Parse environment values and resolve the default user-owned data directory; add URL-scheme classification and compatibility-selection helpers. |
| `crates/memory-mcp/src/config/surreal.rs` | Apply environment defaults, select embedded/remote mode, retain explicit overrides, and record default provenance. |
| `crates/memory-mcp/src/config.rs` | Preserve the shared `env_lock()` test helper and re-export the Stage 4 `StorageBackend` type. |
| `crates/memory-mcp/src/runner.rs` | Add onboarding hints to only the config-related CLI error envelopes; dispatch `init`. |
| `crates/memory-mcp/src/cli.rs` | Add the `Init` command variant without changing existing command semantics. |
| `crates/memory-mcp/src/cli/args.rs` | Define `InitArgs` and its exact target value parser/default. |
| `crates/memory-mcp/src/cli/commands.rs` | Register the `init` handler module. |
| `crates/memory-mcp/src/cli/commands/init.rs` | Render deterministic host-specific JSON snippets and environment instructions. |
| `crates/memory-mcp/tests/agent_memory_lifecycle_release_gate.rs` | Extend the frozen ordinary CLI-subcommand snapshot with `init`. |
| `crates/memory-mcp/src/storage/client.rs` | Stage 1 URL-normalization coverage and Stage 4 only: move the already-existing `DbEngine` choice behind configuration-owned storage terminology. |
| `docs/adr/0029-zero-config-cli-init.md` | Record the explicit public CLI surface expansion and why `init` is output-only; amend the existing CLI freeze. |
| `.github/workflows/ci.yml` | Stage 5: extend the existing release matrix and smoke-test the generated binaries; do not create a duplicate release workflow. |
| `README.md`, `docs/agent_integration/CONTRACT.md` | Stage 5: update installation and first-run instructions. |
| `scripts/measure_ttv.sh` | Stage 6: measure clean-machine install-to-first-recall time for three personas. |
| `crates/memory-mcp/Cargo.toml`, ML modules, and `crates/memory-mcp/tests/local_model_integration.rs` | Stage 7 only: feature-gate the ML stack behind explicitly selected `slim`/`ml` features. |

---

# Stage 1 — Zero-config defaults and safe user data

### Task 1: Add focused helper contracts for URL classification and the default data directory

**Files:**
- Modify: `crates/memory-mcp/src/config/helpers.rs`
- Test: `crates/memory-mcp/src/config/helpers.rs` in a new `#[cfg(test)]` module within the existing helper file.

**Interfaces:**
- Consumes: `std::env`, `std::path::PathBuf`, existing helper conventions.
- Produces: `pub(crate) fn is_remote_url(url: Option<&str>) -> bool`, `pub(crate) fn normalize_url_scheme(raw: &str) -> String`, `pub(crate) fn default_user_data_dir() -> String`, `pub(crate) struct EmbeddedDataDirResolution { pub(crate) path: String, pub(crate) legacy_path: Option<String> }`, and `pub(crate) fn resolve_embedded_data_dir() -> EmbeddedDataDirResolution`, consumed by `default_embedded_data_dir()` and `SurrealConfig::from_env()`.

- [ ] **Step 1: Write failing tests for the URL rule.**

Add tests with these exact cases:

```rust
#[test]
fn remote_url_detection_accepts_only_remote_schemes() {
    assert!(is_remote_url(Some("ws://localhost:8000")));
    assert!(is_remote_url(Some("wss://db.example.com")));
    assert!(is_remote_url(Some("http://localhost:8000")));
    assert!(is_remote_url(Some("https://db.example.com")));
    assert!(!is_remote_url(None));
    assert!(!is_remote_url(Some("mem://local")));
    assert!(!is_remote_url(Some("rocksdb://local")));
    assert!(!is_remote_url(Some("not a URL")));
    assert!(!is_remote_url(Some("ws://")));
    assert!(!is_remote_url(Some("https://")));
    assert!(!is_remote_url(Some("https:///path")));
    assert!(!is_remote_url(Some("https://?query")));
    assert!(!is_remote_url(Some("https://db.example.com/path with space")));
    assert!(is_remote_url(Some("  https://db.example.com  ")));
    assert!(is_remote_url(Some("HTTPS://db.example.com")));
    assert!(!is_remote_url(Some("")));
}
```

- [ ] **Step 2: Run the focused tests and verify they fail.**

Run:

```bash
cargo test -p memory_mcp remote_url_detection_accepts_only_remote_schemes -- --exact
```

Expected: compilation fails because `is_remote_url` does not yet exist.

- [ ] **Step 3: Write the minimal URL classifier.**

Implement URL classification without adding a dependency. Trim surrounding whitespace, require a non-empty authority after `://`, reject any whitespace inside the post-scheme authority/path/query portion, and accept only the four remote schemes. Keep the scheme comparison case-insensitive; the next step must normalize the accepted URL before the remote connector uses it.

```rust
pub(crate) fn is_remote_url(url: Option<&str>) -> bool {
    let Some(raw) = url.map(str::trim) else { return false };
    let Some((scheme, authority_and_path)) = raw.split_once("://") else { return false };
    let authority = authority_and_path
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    if authority.is_empty() || authority_and_path.chars().any(char::is_whitespace) {
        return false;
    }
    matches!(scheme.to_ascii_lowercase().as_str(), "ws" | "wss" | "http" | "https")
}
```

Implement scheme normalization without turning it into URL validation:

```rust
pub(crate) fn normalize_url_scheme(raw: &str) -> String {
    let trimmed = raw.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return trimmed.to_string();
    };
    format!("{}://{rest}", scheme.to_ascii_lowercase())
}
```

The implementation must return `false` for `mem://`; that scheme is not implemented by this repository. Do not classify an invalid URL as remote merely because its prefix resembles a remote scheme. Add a pure normalization test proving `normalize_url_scheme("  HTTPS://db.example.com  ") == "https://db.example.com"`; it trims only surrounding whitespace and lowercases only the scheme, preserving the authority/path.

- [ ] **Step 4: Write failing tests for XDG/home resolution.**

Because environment variables are process-global, keep the helper tests pure and do not mutate process-global variables there. The repository already provides the shared `crate::config::env_lock()` for environment-mutating tests; Task 2 reuses it. Keep `default_user_data_dir_from_env(xdg_data_home, home, current_dir)` as a private pure function and test it directly. Compare returned paths as `PathBuf` values rather than platform-specific strings. Test the pure resolution behavior:

```rust
use std::path::{Path, PathBuf};

#[test]
fn default_user_data_dir_prefers_xdg_data_home() {
    let path = default_user_data_dir_from_env(
        Some("/tmp/xdg-data"),
        Some("/Users/alice"),
        None,
    );
    assert_eq!(PathBuf::from(path), PathBuf::from("/tmp/xdg-data").join("memory_mcp"));
}

#[test]
fn default_user_data_dir_uses_home_when_xdg_is_unset() {
    let path = default_user_data_dir_from_env(None, Some("/Users/alice"), None);
    assert_eq!(
        PathBuf::from(path),
        PathBuf::from("/Users/alice").join(".local").join("share").join("memory_mcp")
    );
}

#[test]
fn default_user_data_dir_has_deterministic_fallback_without_home() {
    let path = default_user_data_dir_from_env(None, None, Some("/tmp/worktree"));
    assert_eq!(PathBuf::from(path), PathBuf::from("/tmp/worktree").join(".memory_mcp"));
}
```

- [ ] **Step 5: Run the focused data-directory tests and verify they fail.**

Run:

```bash
cargo test -p memory_mcp default_user_data_dir_ -- --nocapture
```

Expected: compilation fails because the testable resolver does not yet exist.

- [ ] **Step 6: Implement the resolver and public wrapper.**

Implement a private/testable resolver with this exact precedence:

```rust
fn default_user_data_dir_from_env(
    xdg_data_home: Option<&str>,
    home: Option<&str>,
    current_dir: Option<&str>,
) -> String {
    if let Some(base) = xdg_data_home.filter(|value| !value.is_empty()) {
        return PathBuf::from(base)
            .join("memory_mcp")
            .to_string_lossy()
            .into_owned();
    }
    if let Some(base) = home.filter(|value| !value.is_empty()) {
        return PathBuf::from(base)
            .join(".local")
            .join("share")
            .join("memory_mcp")
            .to_string_lossy()
            .into_owned();
    }
    PathBuf::from(current_dir.unwrap_or("."))
        .join(".memory_mcp")
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn default_user_data_dir() -> String {
    default_user_data_dir_from_env(
        env::var("XDG_DATA_HOME").ok().as_deref(),
        env::var("HOME").ok().as_deref(),
        env::current_dir().ok().and_then(|path| path.to_str()),
    )
}
```

Do not silently strand existing installations that used the current executable-relative default. Add a pure/testable `legacy_embedded_data_dir_from_exe(exe_path: Option<&Path>) -> Option<String>` that returns `<executable-parent>/data/surrealdb` when an executable parent exists. Change `default_embedded_data_dir()` into a compatibility selector:

1. If the new XDG/home path already exists, use it.
2. Else if the legacy executable-relative path exists, use the legacy path and return it as `legacy_path`; `MemoryService::new_from_env_with_mode()` emits the separate `config.legacy_data_dir_detected` startup event containing only the path and no credentials.
3. Else use the new XDG/home path.

Keep `default_user_data_dir()` as the canonical new-path resolver and keep `default_embedded_data_dir()` as the existing consumer-facing compatibility wrapper. Introduce an internal `EmbeddedDataDirResolution { path: String, legacy_path: Option<String> }` plus a pure selector that receives the new path, optional legacy path, and their `is_dir()` results. The selector must choose the new path when it exists, the legacy path only when the new path is absent and the legacy directory exists, and otherwise the new path. Implement `resolve_embedded_data_dir()` by passing `Path::new(&new_path).is_dir()` and `legacy_path.as_deref().is_some_and(|path| Path::new(path).is_dir())` into that selector; implement `default_embedded_data_dir()` as `resolve_embedded_data_dir().path`. Add tests for new-path preference, legacy-path fallback, and fresh-install new-path selection. Document the recovery/migration policy: users may copy the legacy directory to the new path while the server is stopped, or set `SURREALDB_DATA_DIR` explicitly; the server must not copy a live RocksDB directory automatically. Derive the legacy candidate from `env::current_exe().ok().as_deref()` and pass its `Option<&Path>` through `legacy_embedded_data_dir_from_exe`; do not use the current working directory as a legacy executable path.

- [ ] **Step 7: Run the helper tests and formatting.**

Run:

```bash
cargo test -p memory_mcp default_user_data_dir_ -- --nocapture
cargo test -p memory_mcp remote_url_detection_accepts_only_remote_schemes -- --exact
cargo fmt --all --check
```

Expected: all focused tests pass and formatting reports no changes.

- [ ] **Step 8: Commit the helper slice.**

```bash
git add crates/memory-mcp/src/config/helpers.rs
git commit -m "feat: resolve embedded data in user data directory"
```

### Task 2: Make `SurrealConfig::from_env()` genuinely zero-config

**Files:**
- Modify: `crates/memory-mcp/src/config/surreal.rs:7-11,25-53,55-130,139-144,338-437`
- Create: `crates/memory-mcp/tests/zero_config_embedded.rs` for the real RocksDB root/root round-trip
- Modify: `crates/memory-mcp/src/service/core/builder.rs:84-126` to emit provenance events through the existing startup logger
- Modify: `crates/memory-mcp/src/config/helpers.rs` for the embedded data-dir resolution result and default-path provenance
- Modify: `crates/memory-mcp/src/storage/helpers.rs` and its existing tests so accepted remote URLs are trimmed and scheme-normalized before the existing HTTP/HTTPS-to-WebSocket conversion
- Test: `crates/memory-mcp/src/config/surreal.rs` existing tests plus a temporary-directory compatibility test
- Modify/Test: `crates/memory-mcp/src/service/entity_extraction.rs` to add or retain the exact Anno-default regression test
- Test: `crates/memory-mcp/src/service/core/builder.rs` or its existing test module for the startup-event contract

**Interfaces:**
- Consumes: `is_remote_url`, `parse_bool_env`, `parse_comma_list`, `parse_env`, `default_user_data_dir`, the existing `default_embedded_data_dir()` wrapper, and existing `StdoutLogger`/structured logging conventions.
- Produces: `SurrealConfig::from_env()` with explicit environment overrides and these defaults: `db_name = "memory"`, `namespaces = ["org"]`, `username = "root"`, `password = "root"`, inferred embedded mode, and a resolved embedded data directory. It also records the names of defaulted variables plus an optional legacy-directory path in non-secret provenance fields consumed by the existing startup logger.

- [ ] **Step 1: Write a failing empty-environment test that calls `from_env()` directly.**

Reuse the existing `crate::config::env_lock()` helper rather than defining a second mutex: the loader calls `EmbeddingConfig::from_env()`, `NerConfig::from_env()`, and `LifecycleConfig::from_env()`, so a local lock would not serialize with related configuration tests. The test-only snapshot must cover every environment variable read transitively by `SurrealConfig::from_env()` and restore each value in `Drop`:

```rust
#[cfg(test)]
const SURREAL_CONFIG_ENV_KEYS: &[&str] = &[
    "SURREALDB_URL", "SURREALDB_EMBEDDED", "SURREALDB_DB_NAME",
    "SURREALDB_NAMESPACES", "SURREALDB_USERNAME", "SURREALDB_PASSWORD",
    "SURREALDB_DATA_DIR", "RUST_LOG", "QUERY_LOGGING_ENABLED",
    "QUERY_LOG_RETENTION_DAYS", "LIFECYCLE_ENABLED",
    "LIFECYCLE_DECAY_INTERVAL_SECS", "LIFECYCLE_ARCHIVAL_INTERVAL_SECS",
    "LIFECYCLE_DECAY_THRESHOLD", "LIFECYCLE_ARCHIVAL_AGE_DAYS",
    "LIFECYCLE_DECAY_HALF_LIFE_DAYS", "EMBEDDINGS_ENABLED",
    "EMBEDDINGS_PROVIDER", "EMBEDDINGS_TIMEOUT_SECS",
    "SURREALDB_EMBEDDING_DIMENSION", "EMBEDDINGS_MAX_TOKENS",
    "EMBEDDINGS_SIMILARITY_THRESHOLD", "EMBEDDINGS_MODEL_DIR",
    "EMBEDDINGS_MODEL", "EMBEDDINGS_BASE_URL", "EMBEDDINGS_API_KEY",
    "NER_PROVIDER", "NER_MODEL", "NER_LABELS", "NER_THRESHOLD",
    "NER_BATCH_SIZE", "NER_MAX_BATCH_TOKENS", "NER_MAX_CONCURRENCY",
    "NER_DEVICE", "NER_MODEL_DIR", "XDG_DATA_HOME", "HOME",
];

#[cfg(test)]
struct EnvSnapshot {
    keys: &'static [&'static str],
    values: Vec<Option<String>>,
}

#[cfg(test)]
impl EnvSnapshot {
    fn capture(keys: &'static [&'static str]) -> Self {
        Self { keys, values: keys.iter().map(|key| std::env::var(key).ok()).collect() }
    }
}

#[cfg(test)]
impl Drop for EnvSnapshot {
    fn drop(&mut self) {
        for (key, value) in self.keys.iter().zip(&self.values) {
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }
}

fn clear_surreal_environment() {
    for key in SURREAL_CONFIG_ENV_KEYS {
        unsafe { std::env::remove_var(key) };
    }
}

#[test]
fn from_env_applies_zero_config_embedded_defaults() {
    let _lock = crate::config::env_lock().lock().expect("environment lock");
    let _snapshot = EnvSnapshot::capture(SURREAL_CONFIG_ENV_KEYS);
    clear_surreal_environment();
    unsafe { std::env::set_var("SURREALDB_DATA_DIR", "/tmp/memory-mcp-zero-config-test") };

    let config = SurrealConfig::from_env().expect("empty environment should be valid");

    assert_eq!(config.db_name, "memory");
    assert_eq!(config.namespaces, vec!["org"]);
    assert_eq!(config.username, "root");
    assert_eq!(config.password, "root");
    assert!(config.embedded);
    assert_eq!(config.url, None);
    assert_eq!(config.data_dir_or_default(), "/tmp/memory-mcp-zero-config-test");
    assert!(!config.defaulted_variables.contains(&"SURREALDB_DATA_DIR"));
    assert!(config.legacy_data_dir.is_none());
}
```

Add a second environment-loader test that leaves `SURREALDB_DATA_DIR` unset, sets `XDG_DATA_HOME` to a `tempfile::TempDir`, pre-creates only the new `<xdg>/memory_mcp` directory so the compatibility selector cannot choose an unrelated legacy directory, and asserts the selected `config.data_dir` equals that new path. Assert the provenance set contains exactly `SURREALDB_DB_NAME`, `SURREALDB_NAMESPACES`, `SURREALDB_EMBEDDED`, `SURREALDB_USERNAME`, `SURREALDB_PASSWORD`, and `SURREALDB_DATA_DIR`.

This test proves the loader itself supplies the storage default; Step 9 separately proves the real RocksDB connector can use root/root.

`clear_surreal_environment()` must remove every key in `SURREAL_CONFIG_ENV_KEYS`, including the transitive lifecycle, embedding, NER, `XDG_DATA_HOME`, and `HOME` keys. The snapshot must restore all of them with explicit Rust 2024 `unsafe { std::env::set_var/remove_var }` calls in `Drop`. Do not use `SurrealConfigBuilder::default()` for this test.

- [ ] **Step 2: Run the test and verify the current required-variable failure.**

Run:

```bash
cargo test -p memory_mcp from_env_applies_zero_config_embedded_defaults -- --exact
```

Expected: FAIL with `ConfigMissing`, currently beginning with `SURREALDB_DB_NAME`.

- [ ] **Step 3: Implement the exact default and inference logic.**

Replace the required database configuration section with this defaulting core:

```rust
let raw_url = env::var("SURREALDB_URL").ok();
let url = raw_url
    .as_deref()
    .map(normalize_url_scheme)
    .filter(|value| !value.is_empty());
let embedded_was_explicit = env::var("SURREALDB_EMBEDDED").is_ok();
let embedded = parse_bool_env("SURREALDB_EMBEDDED")
    .unwrap_or_else(|| !is_remote_url(raw_url.as_deref()));
let db_name_was_explicit = env::var("SURREALDB_DB_NAME").is_ok();
let db_name = env::var("SURREALDB_DB_NAME").unwrap_or_else(|_| "memory".into());
let namespaces_were_explicit = env::var("SURREALDB_NAMESPACES").is_ok();
let namespaces = parse_comma_list("SURREALDB_NAMESPACES")
    .unwrap_or_else(|_| vec!["org".into()]);
let username_was_explicit = env::var("SURREALDB_USERNAME").is_ok();
let password_was_explicit = env::var("SURREALDB_PASSWORD").is_ok();
let username = env::var("SURREALDB_USERNAME").unwrap_or_else(|_| "root".into());
let password = env::var("SURREALDB_PASSWORD").unwrap_or_else(|_| "root".into());

let mut defaulted_variables = Vec::new();
if !db_name_was_explicit { defaulted_variables.push("SURREALDB_DB_NAME"); }
if !namespaces_were_explicit { defaulted_variables.push("SURREALDB_NAMESPACES"); }
if !embedded_was_explicit { defaulted_variables.push("SURREALDB_EMBEDDED"); }
if embedded && !username_was_explicit { defaulted_variables.push("SURREALDB_USERNAME"); }
if embedded && !password_was_explicit { defaulted_variables.push("SURREALDB_PASSWORD"); }
```

Normalize accepted remote URL input at the storage boundary: keep the `SurrealConfig.url` value trimmed with a lower-case scheme, and update `storage::helpers::normalize_url` to perform the same trim/scheme normalization for builder-created configurations before its existing HTTP/HTTPS-to-WebSocket conversion. Add a test for `  HTTPS://db.example.com  ` reaching the connector as a valid normalized remote URL.

The storage helper must begin with the same scheme normalization before its existing conversion logic:

```rust
let normalized = url.trim();
let Some((scheme, rest)) = normalized.split_once("://") else {
    return normalized.to_string();
};
let normalized = format!("{}://{rest}", scheme.to_ascii_lowercase());
```

Its existing `http://`/`https://` conversion must then operate on `normalized`, while `ws://`/`wss://` values remain WebSocket URLs. Add unit assertions for uppercase and surrounding whitespace, plus the existing `/rpc` suffix behavior.

Before returning a remote configuration, validate the security-sensitive exceptions:

```rust
if !embedded && !url.as_deref().is_some_and(is_remote_url) {
    return Err(MemoryError::ConfigMissing("SURREALDB_URL".to_string()));
}
if !embedded
    && (env::var("SURREALDB_USERNAME")
        .ok()
        .is_none_or(|value| value.trim().is_empty())
        || env::var("SURREALDB_PASSWORD")
            .ok()
            .is_none_or(|value| value.trim().is_empty()))
{
    return Err(MemoryError::ConfigMissing(
        "SURREALDB_USERNAME and SURREALDB_PASSWORD are required for remote mode".to_string(),
    ));
}
```

The `root/root` values therefore apply only to inferred or explicitly selected embedded mode. A remote URL without non-empty explicit credentials fails instead of silently attempting remote authentication with embedded credentials. Add tests for remote URL without credentials, remote URL with empty credentials, remote URL with explicit credentials, and `SURREALDB_EMBEDDED=false` without a valid remote URL.

After validation, resolve the embedded data path exactly once during `from_env()`:

```rust
let (data_dir, legacy_data_dir) = if let Ok(explicit) = env::var("SURREALDB_DATA_DIR") {
    (Some(explicit), None)
} else if embedded {
    let resolution = resolve_embedded_data_dir();
    defaulted_variables.push("SURREALDB_DATA_DIR");
    (Some(resolution.path), resolution.legacy_path)
} else {
    (None, None)
};
```

Only push `SURREALDB_DATA_DIR` when the environment variable was absent and embedded mode selected a default; an explicit `SURREALDB_DATA_DIR` must be retained unchanged and must not emit a default event. Use the resulting `data_dir` in `SurrealConfig`, and keep `data_dir_or_default()` as the fallback for builder-created configurations. Preserve the existing empty-namespace validation for an explicitly supplied empty list. `parse_comma_list` returns `Ok(empty)` for an explicitly empty string, so only its `Err` result should trigger the `org` default. Update the imports to use `is_remote_url`. Update the module documentation table so these variables are marked optional with their defaults, while remote credentials remain required in remote mode.

- [ ] **Step 4: Record default provenance in the config without logging from `from_env()`.**

Add a field such as:

```rust
/// Environment variables whose values were supplied by zero-config defaults.
pub(crate) defaulted_variables: Vec<&'static str>,
```

Include `SURREALDB_DB_NAME`, `SURREALDB_NAMESPACES`, and `SURREALDB_EMBEDDED` when their corresponding environment variables were absent. Include `SURREALDB_USERNAME` and `SURREALDB_PASSWORD` only when embedded mode uses the local `root/root` defaults, and include `SURREALDB_DATA_DIR` only when embedded mode resolves a default path rather than an explicit data directory. Do not include values, especially not the password. Add `defaulted_variables: Vec<&'static str>` and `legacy_data_dir: Option<String>` to `SurrealConfig`, add matching fields to `SurrealConfigBuilder`, and initialize both builder fields to empty/`None` so every builder-created `SurrealConfig` remains valid. Search all `SurrealConfig { ... }` literals after adding the fields. In `from_env()`, use the `EmbeddedDataDirResolution` from Task 1 only for embedded mode, store its selected path in `data_dir`, and retain its optional `legacy_path` for startup provenance; remote mode must not emit storage-path defaults. `SurrealConfig::from_env()` must remain a loader and validator; it must not write to stdout or otherwise log.

Update `SurrealConfigBuilder::default()` with `defaulted_variables: Vec::new()` and `legacy_data_dir: None`, and pass both fields through the direct `SurrealConfig { ... }` literal in `SurrealConfigBuilder::build()`. Add a builder test asserting a programmatically built config has an empty provenance list and no legacy path.

In `crates/memory-mcp/src/service/core/builder.rs`, add a pure helper with this exact contract:

```rust
fn startup_config_events(config: &SurrealConfig)
    -> Vec<std::collections::HashMap<String, serde_json::Value>>
```

It must return one `config.default_applied` map per `config.defaulted_variables`, with only `op` and `variable`, plus one optional `config.legacy_data_dir_detected` map with only `op` and `path` when `config.legacy_data_dir` is `Some`. Immediately after constructing `startup_logger` and before connecting storage, log every map at `Info`:

```rust
for event in startup_config_events(&config) {
    startup_logger.log(event, crate::logging::LogLevel::Info);
}
```

Do not emit an event when the corresponding variable is explicitly set. The event contract covers only the storage defaults introduced here, not pre-existing nested defaults such as lifecycle intervals, disabled embeddings, NER settings, or `RUST_LOG`. `Info` means the logger receives the event; normal logger filtering may suppress its output when `RUST_LOG=warn`. Add unit tests for default provenance, explicit configuration with no default events, legacy-path detection, and the invariant that no event contains a credential value. A dedicated provenance test with `tempfile::TempDir` as `XDG_DATA_HOME`, no explicit `SURREALDB_DATA_DIR`, and the same environment snapshot must contain exactly these storage defaults (order may be tested as a set): `SURREALDB_DB_NAME`, `SURREALDB_NAMESPACES`, `SURREALDB_EMBEDDED`, `SURREALDB_USERNAME`, `SURREALDB_PASSWORD`, and `SURREALDB_DATA_DIR`; it must contain neither a password value nor nested lifecycle/embedding/NER default names.

- [ ] **Step 5: Write failing inference tests for explicit remote and explicit override cases.**

Add these tests using the same `crate::config::env_lock()` and `EnvSnapshot` helpers from the preceding test. Each test must acquire the shared lock, capture `SURREAL_CONFIG_ENV_KEYS`, call `clear_surreal_environment()`, set only the variables under test, and let the snapshot restore them:

```rust
#[test]
fn from_env_rejects_remote_url_without_explicit_credentials() {
    let _lock = crate::config::env_lock().lock().expect("environment lock");
    let _snapshot = EnvSnapshot::capture(SURREAL_CONFIG_ENV_KEYS);
    clear_surreal_environment();
    unsafe { std::env::set_var("SURREALDB_URL", "ws://localhost:8000") };

    let error = SurrealConfig::from_env().expect_err("remote credentials are required");

    assert!(matches!(error, MemoryError::ConfigMissing(message) if message.contains("USERNAME")));
}

#[test]
fn from_env_rejects_remote_url_with_empty_credentials() {
    let _lock = crate::config::env_lock().lock().expect("environment lock");
    let _snapshot = EnvSnapshot::capture(SURREAL_CONFIG_ENV_KEYS);
    clear_surreal_environment();
    unsafe { std::env::set_var("SURREALDB_URL", "ws://localhost:8000") };
    unsafe { std::env::set_var("SURREALDB_USERNAME", " ") };
    unsafe { std::env::set_var("SURREALDB_PASSWORD", "secret") };

    let error = SurrealConfig::from_env().expect_err("remote credentials must be non-empty");

    assert!(matches!(error, MemoryError::ConfigMissing(message) if message.contains("USERNAME")));
}

#[test]
fn from_env_accepts_remote_url_with_explicit_credentials() {
    let _lock = crate::config::env_lock().lock().expect("environment lock");
    let _snapshot = EnvSnapshot::capture(SURREAL_CONFIG_ENV_KEYS);
    clear_surreal_environment();
    unsafe { std::env::set_var("SURREALDB_URL", "  HTTPS://localhost:8000  ") };
    unsafe { std::env::set_var("SURREALDB_USERNAME", "memory_user") };
    unsafe { std::env::set_var("SURREALDB_PASSWORD", "secret") };

    let config = SurrealConfig::from_env().expect("explicit remote credentials should be valid");

    assert!(!config.embedded);
    assert_eq!(config.url.as_deref(), Some("https://localhost:8000"));
    assert_eq!(config.username, "memory_user");
    assert_eq!(config.password, "secret");
}

#[test]
fn explicit_embedded_false_without_remote_url_is_invalid() {
    let _lock = crate::config::env_lock().lock().expect("environment lock");
    let _snapshot = EnvSnapshot::capture(SURREAL_CONFIG_ENV_KEYS);
    clear_surreal_environment();
    unsafe { std::env::set_var("SURREALDB_EMBEDDED", "false") };

    let error = SurrealConfig::from_env().expect_err("remote mode needs a URL");

    assert!(matches!(error, MemoryError::ConfigMissing(message) if message.contains("SURREALDB_URL")));
}

#[test]
fn explicit_embedded_override_wins_over_remote_url() {
    let _lock = crate::config::env_lock().lock().expect("environment lock");
    let _snapshot = EnvSnapshot::capture(SURREAL_CONFIG_ENV_KEYS);
    clear_surreal_environment();
    unsafe { std::env::set_var("SURREALDB_URL", "https://localhost:8000") };
    unsafe { std::env::set_var("SURREALDB_EMBEDDED", "true") };

    let config = SurrealConfig::from_env().expect("explicit embedded mode should win");

    assert!(config.embedded);
    assert_eq!(config.url.as_deref(), Some("https://localhost:8000"));
}
```

- [ ] **Step 6: Run the inference and remote-credential tests and verify the old behavior is exposed.**

Run:

```bash
cargo test -p memory_mcp from_env_rejects_remote_url_without_explicit_credentials -- --exact
cargo test -p memory_mcp from_env_rejects_remote_url_with_empty_credentials -- --exact
cargo test -p memory_mcp from_env_accepts_remote_url_with_explicit_credentials -- --exact
cargo test -p memory_mcp explicit_embedded_false_without_remote_url_is_invalid -- --exact
cargo test -p memory_mcp explicit_embedded_override_wins_over_remote_url -- --exact
```

Expected: the missing-credential, empty-credential, and explicit-remote tests expose the old required-variable behavior; the explicit-embedded override test exposes the old URL/credential behavior.

- [ ] **Step 7: Update the existing broken data-directory expectation and add legacy compatibility tests.**

Keep the pure selector tests for new-path preference, legacy-path fallback, and both-path preference. Add an end-to-end test named `legacy_data_dir_subprocess_emits_startup_event` in `crates/memory-mcp/tests/zero_config_embedded.rs`: copy `env!("CARGO_BIN_EXE_memory_mcp")` into a uniquely named executable under a `tempfile::TempDir`, create only that copied executable’s sibling `data/surrealdb` directory, leave `SURREALDB_DATA_DIR` unset, set a different temporary `XDG_DATA_HOME`, run the copied binary’s `ingest` command with `env_clear()`, and assert success, stderr containing `config.legacy_data_dir_detected`, and the event path pointing at the temporary legacy directory. Because the executable and legacy database live under the test’s `TempDir`, no shared target directory is mutated. Keep the custom absolute and custom relative path tests unchanged. Add the recovery text to the configuration documentation: explicit `SURREALDB_DATA_DIR` is the escape hatch, and copying a stopped RocksDB directory is a manual migration, not an automatic startup action.

- [ ] **Step 8: Add/retain the Anno-default regression and run all config tests.**

In `crates/memory-mcp/src/service/entity_extraction.rs`, keep the exact test name `create_entity_extractor_defaults_to_anno` and assert the default extractor reports `provider_name() == "anno"`; this test is part of the zero-config contract and must remain available after any backend-registry cleanup.

Run:

```bash
cargo test -p memory_mcp config::surreal
cargo test -p memory_mcp config::helpers
cargo test -p memory_mcp storage::helpers
cargo test -p memory_mcp create_entity_extractor_defaults_to_anno -- --exact
cargo fmt --all --check
```

Expected: all configuration tests pass; the builder tests remain valid and are not being used as a substitute for the `from_env()` tests.

- [ ] **Step 9: Add and run the embedded RocksDB root/root round-trip smoke test.**

Use a `tempfile::TempDir`, construct a `SurrealConfigBuilder` with `db_name("memory")`, `namespace("org")`, `credentials("root", "root")`, `embedded(true)`, and `data_dir(tempdir.path().display().to_string())`, then call the existing `SurrealDbClient::connect(&config, "org")`. Use the existing `DbClient` methods to create a deterministic record and read it back:

```rust
client
    .create("zero_config_smoke", serde_json::json!({"value": "ok"}), "org")
    .await
    .expect("create record");
let record = client
    .select_one("zero_config_smoke", "org")
    .await
    .expect("select record")
    .expect("record exists");
assert_eq!(record["value"], "ok");
```

The assertion must prove connection, root/root authentication, namespace selection, and one write/read round trip. Do not use the in-memory connector for this test because it does not exercise `RocksDb` or `signin(root/root)`.

Treat a failure at `db.signin(root)` as a blocking storage defect to diagnose before proceeding: verify the SurrealDB 3 RocksDB authentication contract, then make the smallest tested correction in `connect_embedded` and document it in ADR-0029. The final test must cover the chosen embedded behavior and remote connections must continue to authenticate explicitly; never remove authentication based on an assumption.

Run:

```bash
cargo test -p memory_mcp --test zero_config_embedded -- --nocapture
```

Expected: PASS with a temporary RocksDB directory and no external database process.

- [ ] **Step 10: Run the Stage 1 validation gate.**

```bash
cargo fmt --all --check
cargo test -p memory_mcp config::
cargo test -p memory_mcp --test agent_memory_lifecycle_release_gate
```

Expected: formatting, focused configuration tests, and the existing release gate all pass.

- [ ] **Step 11: Commit the zero-config configuration slice.**

```bash
git add crates/memory-mcp/src/config/helpers.rs crates/memory-mcp/src/config/surreal.rs crates/memory-mcp/src/service/core/builder.rs crates/memory-mcp/src/storage/helpers.rs crates/memory-mcp/tests/zero_config_embedded.rs
git commit -m "feat: make embedded configuration zero config"
```

Include `crates/memory-mcp/src/storage/client.rs` in this commit only when the root/root smoke test required the narrowly scoped embedded-auth correction described in Step 9; the storage URL normalizer and startup provenance changes are always included.

---

# Stage 2 — Actionable first failure

### Task 3: Add hints to configuration-related CLI error envelopes

**Files:**
- Modify: `crates/memory-mcp/src/runner.rs:132-146,200-209`
- Test: `crates/memory-mcp/src/runner.rs` unit tests for the shared envelope builder
- Create: `crates/memory-mcp/tests/cli_error_envelope.rs` for the real startup/configuration failure path

**Interfaces:**
- Consumes: `MemoryError`, existing `error_kind()` and `error_exit_code()` functions, and the `Box<dyn Error>` startup path returned by `build_memory_service`.
- Produces: JSON stderr envelopes with a `hint` field only for `ConfigMissing` and `ConfigInvalid`, including failures that occur before one-shot command handlers run.

- [ ] **Step 1: Write failing unit tests for the envelope policy.**

Extract the exact `fn cli_error_json(err: &MemoryError) -> serde_json::Value` helper from the existing `report_cli_error` body, then add tests with these expectations:

```rust
#[test]
fn cli_error_config_missing_contains_zero_config_hint() {
    let value = cli_error_json(&MemoryError::ConfigMissing("SURREALDB_URL".into()));
    assert_eq!(value["kind"], "ConfigMissing");
    assert_eq!(value["hint"], "Run `memory_mcp init` for host configuration, or unset remote database variables to use embedded mode.");
}

#[test]
fn cli_error_config_invalid_contains_repair_hint() {
    let value = cli_error_json(&MemoryError::ConfigInvalid("bad namespace".into()));
    assert_eq!(value["kind"], "ConfigInvalid");
    assert_eq!(value["hint"], "Check the environment values or run `memory_mcp init` to print a known-good configuration.");
}

#[test]
fn cli_error_non_config_error_has_no_hint_field() {
    let value = cli_error_json(&MemoryError::Validation("bad input".into()));
    assert!(value.get("hint").is_none());
}
```

- [ ] **Step 2: Run the tests and verify the missing-field failures.**

```bash
cargo test -p memory_mcp runner::tests -- --nocapture
```

Expected: the config tests fail because the current envelope contains only `error`, `kind`, and `exit_code`; the non-config test should continue to assert no hint. If the runner has no test module yet, add the `#[cfg(test)] mod tests` in the same red step so this command names the actual module.

- [ ] **Step 3: Implement the conditional hint.**

Construct the existing fields unchanged and add `hint` conditionally:

```rust
fn cli_error_json(err: &MemoryError) -> serde_json::Value {
    let code = error_exit_code(err);
    let mut envelope = serde_json::json!({
        "error": err.to_string(),
        "kind": error_kind(err),
        "exit_code": code,
    });

    match err {
        MemoryError::ConfigMissing(_) => {
            envelope["hint"] = serde_json::json!(
                "Run `memory_mcp init` for host configuration, or unset remote database variables to use embedded mode."
            );
        }
        MemoryError::ConfigInvalid(_) => {
            envelope["hint"] = serde_json::json!(
                "Check the environment values or run `memory_mcp init` to print a known-good configuration."
            );
        }
        _ => {}
    }
    envelope
}

fn boxed_to_failure(err: Box<dyn std::error::Error>) -> ExitCode {
    if let Some(memory_error) = err.downcast_ref::<MemoryError>() {
        eprintln!("{}", cli_error_json(memory_error));
    } else {
        eprintln!("{}", serde_json::json!({"error": err.to_string(), "exit_code": 1u8}));
    }
    ExitCode::FAILURE
}
```

Change `report_cli_error` to print `cli_error_json(&err)` and remove its duplicate envelope construction. The downcast branch is required because `build_memory_service` boxes startup errors before `dispatch` handles one-shot commands. This makes the hint policy apply to `serve`, `watch`, `reembed`, and all service-building one-shot commands. Do not add a hint to storage, transient, validation, not-found, conflict, or budget errors.

- [ ] **Step 4: Add and run the real startup-failure integration test.**

Create `crates/memory-mcp/tests/cli_error_envelope.rs` with this test shape:

```rust
#[test]
fn startup_config_failure_uses_the_same_hint_envelope() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_memory_mcp"))
        .env_clear()
        .env("SURREALDB_EMBEDDED", "false")
        .args(["ingest", "--source-type", "note", "--source-id", "test", "--content", "x", "--t-ref", "2026-08-04T00:00:00Z"])
        .output()
        .expect("run binary");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ConfigMissing"));
    assert!(stderr.contains("memory_mcp init"));
}
```

Run:

```bash
cargo test -p memory_mcp runner::tests -- --nocapture
cargo test -p memory_mcp --test cli_error_envelope
cargo test -p memory_mcp --test agent_memory_lifecycle_release_gate
cargo fmt --all --check
```

Expected: PASS, with the startup test proving that configuration failures before the command handler still include the hint.

- [ ] **Step 5: Commit the error UX slice.**

```bash
git add crates/memory-mcp/src/runner.rs crates/memory-mcp/tests/cli_error_envelope.rs
git commit -m "feat: explain configuration errors"
```

---

# Stage 3 — Copy-paste host setup through `init`

### Task 4: Record the public CLI-surface decision in ADR-0029

**Files:**
- Create: `docs/adr/0029-zero-config-cli-init.md`
- Modify: `docs/adr/0016-agent-memory-lifecycle-integration.md` to mark the ordinary-CLI freeze amended by the output-only `init` exception
- Modify: `CONTEXT.md` to list the exception and preserve the eight-tool freeze
- Modify: `docs/agent_integration/CONTRACT.md` to list `init` separately from lifecycle/ordinary memory commands
- Read for consistency: `crates/memory-mcp/tests/agent_memory_lifecycle_release_gate.rs`

**Interfaces:**
- Consumes: the existing public-surface freeze and the decision to add one output-only CLI subcommand.
- Produces: an accepted ADR that authorizes `init` and specifies its stable target set and non-mutating behavior.

- [ ] **Step 1: Write the ADR with the exact decision.**

The ADR must contain these sections and decisions:

```markdown
# ADR-0029: Add an Output-Only `init` Command for Zero-Config Host Setup

> Status: Accepted
> Date: 2026-08-04
> Related: ADR-0016 (public surface freeze)
> Amends: ADR-0016 AD-2 and the frozen public-surface wording in `CONTEXT.md` and `docs/agent_integration/CONTRACT.md`.

## Context

A new user currently has to discover MCP host configuration, environment variables,
and database defaults before receiving a first recalled fact. Zero-config embedded
operation removes the database setup, but host registration still requires copy-paste
knowledge.

## Decision

Add one public CLI subcommand, `memory_mcp init`, with targets `vscode`,
`claude-desktop`, `codex`, `zed`, and `env`. The default target is `vscode`.
The command prints one deterministic, host-native snippet wrapped in a JSON result
object to stdout and never writes host files, changes environment variables, starts
a database, or performs network access.

This is an explicit exception to ADR-0016 AD-2: `init` is not a lifecycle verb,
does not expose a memory capability, and does not alter the eight-tool MCP surface.
The ordinary CLI surface therefore grows from the existing frozen list by exactly
one output-only onboarding command.

## Consequences

ADR-0016, `CONTEXT.md`, and `docs/agent_integration/CONTRACT.md` must say that the
ordinary CLI freeze is amended by this one exception. The command is safe to run
repeatedly and can be used in install documentation. Host configuration schemas
may evolve independently, so each target has a dedicated renderer fixture based
on the target's authoritative documentation and format.

## Non-goals

This command does not install the binary, download models, edit shell profiles,
configure remote credentials, or claim that Anno/ML startup is dependency-free.
```

- [ ] **Step 2: Verify the ADR is internally consistent.**

Check that it says `init` is public, output-only, has five exact targets, defaults to `vscode`, explicitly amends ADR-0016 AD-2 and both public-surface docs, and does not change the eight-tool MCP freeze. In `docs/agent_integration/CONTRACT.md`, list the live Clap spellings `assemble-context`, `lifecycle-capture`, and `lifecycle-recall`, then list `init` as a separate output-only onboarding command; preserve the eight MCP tools. Correct the contract’s test reference to `crates/memory-mcp/tests/agent_memory_lifecycle_release_gate.rs`. In ADR-0016 AD-2, replace the inaccurate claim that lifecycle capabilities are not CLI subcommands with the precise rule that hidden lifecycle subcommands are internal and not part of the ordinary public CLI surface. Do not claim the CLI remains at 11 subcommands after this change.

- [ ] **Step 3: Commit the ADR.**

```bash
git add docs/adr/0029-zero-config-cli-init.md docs/adr/0016-agent-memory-lifecycle-integration.md CONTEXT.md docs/agent_integration/CONTRACT.md
git commit -m "docs: approve zero-config init command"
```

### Task 5: Add `InitArgs` and the frozen CLI command variant

**Files:**
- Modify: `crates/memory-mcp/src/cli/args.rs`
- Modify: `crates/memory-mcp/src/cli.rs:33-75`
- Modify: `crates/memory-mcp/src/runner.rs:47-129,175-188`
- Modify: `crates/memory-mcp/src/cli/commands.rs`
- Create: `crates/memory-mcp/src/cli/commands/init.rs` with a compile-safe minimal handler that Task 6 replaces with the final renderers
- Modify: `crates/memory-mcp/tests/agent_memory_lifecycle_release_gate.rs`

**Interfaces:**
- Consumes: ADR-0029 and existing `clap::Args`/`clap::Subcommand` conventions.
- Produces: `Command::Init(InitArgs)` and a dispatch path that does not build `MemoryService` or touch storage.

- [ ] **Step 1: Write a live parser test and update the expected list.**

Add `use clap::{CommandFactory, Parser};` and `use memory_mcp::cli::Cli;` to `crates/memory-mcp/tests/agent_memory_lifecycle_release_gate.rs`. Add these tests before changing the CLI enum:

```rust
#[test]
fn cli_parser_exposes_init() {
    let parsed = Cli::try_parse_from(["memory_mcp", "init"]);
    assert!(parsed.is_ok(), "init must be a real clap subcommand: {parsed:?}");
}

#[test]
fn live_cli_surface_matches_snapshot() {
    let command = Cli::command();
    let actual: std::collections::HashSet<&str> = command
        .get_subcommands()
        .map(clap::Command::get_name)
        .collect();
    let expected: std::collections::HashSet<&str> = EXPECTED_CLI_SUBCOMMANDS.iter().copied().collect();
    assert_eq!(actual, expected, "frozen CLI snapshot must match live Clap commands");
}
```

Update the existing snapshot to use the actual Clap command spellings and append `init`:

```rust
const EXPECTED_CLI_SUBCOMMANDS: &[&str] = &[
    "serve",
    "watch",
    "reembed",
    "ingest",
    "extract",
    "resolve",
    "invalidate",
    "explain",
    "assemble-context",
    "lifecycle-capture",
    "lifecycle-recall",
    "init",
];
```

Replace the current underscore spellings for the three derived subcommands with Clap’s kebab-case names before comparing the snapshot with the live parser, and update the constant’s doc comment to say that `init` is the one authorized output-only exception rather than claiming the pre-change command count.

Run:

```bash
cargo test -p memory_mcp --test agent_memory_lifecycle_release_gate cli_parser_exposes_init -- --exact
cargo test -p memory_mcp --test agent_memory_lifecycle_release_gate live_cli_surface_matches_snapshot -- --exact
cargo test -p memory_mcp --test agent_memory_lifecycle_release_gate public_surface_snapshot -- --exact
```

Expected: both tests compile but fail because `Command` does not yet expose `init`; the live test also protects the snapshot from future self-referential edits. The existing constant-only `public_surface_snapshot` is not sufficient because it does not inspect the live Clap command.

- [ ] **Step 2: Add the exact argument type.**

In `crates/memory-mcp/src/cli/args.rs`, add:

```rust
#[derive(Debug, Clone, clap::Args)]
pub struct InitArgs {
    /// Host configuration target: vscode, claude-desktop, codex, zed, or env.
    #[arg(long, value_name = "TARGET", default_value = "vscode")]
    pub target: String,
}
```

The handler will validate the target and return `MemoryError::Validation` for unknown values. Do not add a dependency or use a free-form target without validation in the renderer.

- [ ] **Step 3: Add the command, module registration, and compile-safe handler.**

Add this variant to `Command`:

```rust
/// Print copy-paste configuration for an MCP host without changing files.
Init(args::InitArgs),
```

Add `pub mod init;` to `crates/memory-mcp/src/cli/commands.rs`, create `commands/init.rs`, and register the dispatch arm before the one-shot service-building arms:

```rust
Some(Command::Init(args)) => commands::init::run(args).map_err(report_cli_error),
```

For this task only, the new handler may return a deterministic `MemoryError::Validation("init renderer is implemented in the next task".to_string())`; it must not build `MemoryService`, call `SurrealConfig::from_env`, touch storage, or write files. Task 6 replaces this compile-safe stub before the feature is considered complete. Also add the exhaustive mode label:

```rust
Some(Command::Init(_)) => "cli.init",
```

- [ ] **Step 4: Run the live parser/surface checks.**

```bash
cargo test -p memory_mcp --test agent_memory_lifecycle_release_gate cli_parser_exposes_init -- --exact
cargo test -p memory_mcp --test agent_memory_lifecycle_release_gate live_cli_surface_matches_snapshot -- --exact
cargo test -p memory_mcp --test agent_memory_lifecycle_release_gate public_surface_snapshot -- --exact
cargo run -p memory_mcp -- init --help
cargo fmt --all --check
```

Expected: the live parser, live-surface test, and release gate pass; help shows `init` and `--target`; no database directory is created by invoking help.

- [ ] **Step 5: Commit the command-surface slice.**

```bash
git add crates/memory-mcp/src/cli/args.rs crates/memory-mcp/src/cli.rs crates/memory-mcp/src/cli/commands.rs crates/memory-mcp/src/cli/commands/init.rs crates/memory-mcp/src/runner.rs crates/memory-mcp/tests/agent_memory_lifecycle_release_gate.rs
git commit -m "feat: add init CLI command surface"
```

### Task 6: Implement deterministic host snippet renderers

**Files:**
- Modify: `crates/memory-mcp/src/cli/commands/init.rs` (replace the compile-safe handler from Task 5)

- Test: `crates/memory-mcp/src/cli/commands/init.rs` unit tests
- Create: `crates/memory-mcp/tests/init_non_mutating.rs` subprocess tests
- Modify: `README.md` with the exact first-run/documentation corrections after renderer output is stable

**Interfaces:**
- Consumes: `InitArgs`, the executable name `memory_mcp`, and the zero-config contract.
- Produces: `pub fn run(args: InitArgs) -> Result<(), MemoryError>` and a private `render(target: InitTarget) -> Result<serde_json::Value, MemoryError>` with deterministic output, replacing Task 5’s compile-safe stub.

- [ ] **Step 1: Write failing renderer tests for all five targets and unknown input.**

Define an internal enum:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InitTarget {
    Vscode,
    ClaudeDesktop,
    Codex,
    Zed,
    Env,
}
```

Add tests that parse the outer JSON result and validate each target’s native snippet format:

```rust
#[test]
fn parses_supported_targets() {
    assert_eq!(parse_target("vscode"), Ok(InitTarget::Vscode));
    assert_eq!(parse_target("claude-desktop"), Ok(InitTarget::ClaudeDesktop));
    assert_eq!(parse_target("codex"), Ok(InitTarget::Codex));
    assert_eq!(parse_target("zed"), Ok(InitTarget::Zed));
    assert_eq!(parse_target("env"), Ok(InitTarget::Env));
}

#[test]
fn rejects_unknown_target() {
    assert!(matches!(parse_target("cursor"), Err(MemoryError::Validation(_))));
}

#[test]
fn vscode_renderer_uses_current_mcp_json_schema() {
    let value = render(InitTarget::Vscode).expect("vscode snippet");
    let snippet: serde_json::Value = serde_json::from_str(value["snippet"].as_str().unwrap())
        .expect("VS Code snippet is JSON");
    assert_eq!(value["format"], "json");
    assert_eq!(value["path"], ".vscode/mcp.json");
    assert_eq!(snippet["servers"]["memory_mcp"]["type"], "stdio");
    assert_eq!(snippet["servers"]["memory_mcp"]["command"], "memory_mcp");
    assert_eq!(snippet["servers"]["memory_mcp"]["args"], serde_json::json!([]));
}

#[test]
fn claude_renderer_uses_mcp_servers_schema() {
    let value = render(InitTarget::ClaudeDesktop).expect("Claude snippet");
    let snippet: serde_json::Value = serde_json::from_str(value["snippet"].as_str().unwrap())
        .expect("Claude snippet is JSON");
    assert_eq!(snippet["mcpServers"]["memory_mcp"]["command"], "memory_mcp");
}

#[test]
fn codex_renderer_uses_toml_mcp_servers_table() {
    let value = render(InitTarget::Codex).expect("Codex snippet");
    let snippet = value["snippet"].as_str().unwrap();
    assert!(snippet.contains("[mcp_servers.memory_mcp]"));
    assert!(snippet.contains("command = \"memory_mcp\""));
    assert!(snippet.contains("args = []"));
}

#[test]
fn zed_renderer_uses_context_servers_schema() {
    let value = render(InitTarget::Zed).expect("Zed snippet");
    let snippet: serde_json::Value = serde_json::from_str(value["snippet"].as_str().unwrap())
        .expect("Zed snippet is JSON");
    assert_eq!(snippet["context_servers"]["memory_mcp"]["command"], "memory_mcp");
    assert_eq!(snippet["context_servers"]["memory_mcp"]["args"], serde_json::json!([]));
}

#[test]
fn env_renderer_is_shell_and_contains_no_secret() {
    let value = render(InitTarget::Env).expect("environment snippet");
    assert_eq!(value["format"], "shell");
    assert!(value["snippet"].as_str().unwrap().contains("embedded zero-config"));
    let shell = value["snippet"].as_str().unwrap();
    assert!(shell.contains("SURREALDB_PASSWORD"));
    assert!(!shell.contains("root"));
    assert!(!shell.contains("secret"));
}

#[test]
fn every_renderer_is_non_mutating() {
    let targets = [
        InitTarget::Vscode,
        InitTarget::ClaudeDesktop,
        InitTarget::Codex,
        InitTarget::Zed,
        InitTarget::Env,
    ];
    for target in targets {
        let value = render(target).expect("renderer output");
        assert_eq!(value["mutates_files"], false);
    }
}
```

- [ ] **Step 2: Run the renderer tests and verify they fail.**

```bash
cargo test -p memory_mcp init::tests
```

Expected: FAIL because the module, target parser, and renderer do not exist.

- [ ] **Step 3: Implement target parsing and renderers.**

Use lowercase exact matching and return `MemoryError::Validation(format!("unsupported init target `{raw}`; choose vscode, claude-desktop, codex, zed, or env"))` for other values.

Each command must print one JSON result object with a host-native `snippet` string:

```json
{
  "target": "vscode",
  "format": "json",
  "path": ".vscode/mcp.json",
  "mutates_files": false,
  "snippet": "{\"servers\":{\"memory_mcp\":{\"type\":\"stdio\",\"command\":\"memory_mcp\",\"args\":[]}}}",
  "next": "Copy the snippet into the indicated host configuration, start the host, then ingest and extract one source before assembling context."
}
```

Use the current authoritative host formats, not one guessed JSON shape for every host:

- `vscode`: JSON for `.vscode/mcp.json`, using `servers.memory_mcp.type = "stdio"`, `command`, and `args`; source: `https://code.visualstudio.com/docs/agents/reference/mcp-configuration`.
- `claude-desktop`: JSON for the Claude Desktop configuration file, using `mcpServers.memory_mcp.command` and `args`; source: `https://modelcontextprotocol.io/docs/develop/connect-local-servers`.
- `codex`: TOML for `~/.codex/config.toml`, using `[mcp_servers.memory_mcp]`, `command = "memory_mcp"`, and `args = []`; source: `https://developers.openai.com/codex/mcp`.
- `zed`: JSON for settings, using `context_servers.memory_mcp.command`, `args`, and optional `env`; source: `https://zed.dev/docs/ai/mcp`.
- `env`: shell text showing optional overrides, not required setup:

```sh
# Optional remote configuration; omit these for embedded zero-config mode.
# export SURREALDB_URL=ws://localhost:8000
# export SURREALDB_DB_NAME=memory
# export SURREALDB_NAMESPACES=org
# export SURREALDB_USERNAME=<your-remote-username>
# export SURREALDB_PASSWORD=<your-remote-password>
```

The `env` output must not print a real password or the embedded `root/root` defaults. It must explicitly say that local embedded mode requires no variables and must show `SURREALDB_USERNAME` and `SURREALDB_PASSWORD` as commented placeholders only. The renderer must not claim that the host will automatically edit a file; `path` and `next` explain where the user pastes the snippet.

- [ ] **Step 4: Implement stdout output using the module registration from Task 5.**

Keep the `pub mod init;` registration added in Task 5; do not add it a second time. Implement `run` as:

```rust
pub fn run(args: InitArgs) -> Result<(), MemoryError> {
    let target = parse_target(&args.target)?;
    let value = render(target)?;
    write_response(&value).map_err(|err| MemoryError::Transient(err.to_string()))
}
```

Keep `write_response` as the existing JSON writer. Do not print explanatory prose outside the JSON envelope because scripts and users need one predictable stdout document.

- [ ] **Step 5: Prove subprocess non-mutation and manually inspect each output.**

In `crates/memory-mcp/tests/init_non_mutating.rs`, run the compiled binary once for each target with isolated temporary `HOME`, `XDG_DATA_HOME`, current directory, and a sentinel file. Snapshot the directory trees before each invocation; assert success, parse the outer JSON, assert `mutates_files == false`, and assert the trees and sentinel contents are unchanged. This test must use `env_clear()` and must not start a database or rely on the developer’s existing data directory.

```bash
cargo test -p memory_mcp init::tests
cargo test -p memory_mcp --test init_non_mutating
for target in vscode claude-desktop codex zed env; do cargo run -p memory_mcp -- init --target "$target"; done
```

Expected: all tests pass; each command prints valid JSON; no command starts storage or creates a data directory.

- [ ] **Step 6: Add onboarding documentation using the actual output.**

Update `README.md` with a short first-run path using supported installation forms, and correct the existing contradictory setup text in the same change:

- Replace the root-level `cargo install --path .` command with `cargo install --path crates/memory-mcp --locked`.
- Replace `rocksdb://...` URL examples with the zero-config embedded default and document `ws`/`wss`/`http`/`https` only as remote URL schemes.
- Replace the old VS Code `mcpServers`/`memory-mcp`/explicit-environment example with the renderer’s current `servers.memory_mcp` stdio snippet or a command to run `memory_mcp init`.
- Mark `SURREALDB_DB_NAME` default `memory`, `SURREALDB_NAMESPACES` default `org`, and credentials required only for remote mode.
- Correct the output contract: tool responses go to stdout, structured logs go to stderr, and configuration failures are JSON on stderr.
- Rewrite the README’s ordinary-CLI-surface statement so `init` is the authorized output-only exception. Task 4 owns the corresponding `docs/agent_integration/CONTRACT.md` lifecycle/public-surface corrections.
- State that NER defaults to Anno and that “zero-config” means no external database/service setup, not a dependency-free binary.

```markdown
## First run

1. Download a release binary, or from a checkout run `cargo install --path crates/memory-mcp --locked`.
2. Run `memory_mcp init` for the default VS Code snippet, or pass one of the exact targets `claude-desktop`, `codex`, `zed`, or `env`.
3. Copy the printed host-native snippet into the indicated configuration file. No `SURREALDB_*` variables are required for local embedded mode.
4. Ingest one source, run `extract --episode-id <episode-id>`, then run `assemble-context` to verify a real fact is recalled.
```

Do not document `cargo install memory_mcp --locked` unless a separate crates.io publication has been verified.

State explicitly that NER defaults to Anno and may perform model/runtime work; do not describe the entire application as having zero dependencies merely because database setup is zero-config.

- [ ] **Step 7: Run the Stage 3 validation gate.**

```bash
cargo fmt --all --check
cargo test -p memory_mcp init::tests
cargo test -p memory_mcp --test agent_memory_lifecycle_release_gate
cargo test -p memory_mcp --test tools_e2e
```

Expected: PASS, with the public MCP tool count still exactly eight and the ordinary CLI snapshot containing the new `init` entry.

- [ ] **Step 8: Commit the onboarding slice.**

```bash
git add crates/memory-mcp/src/cli/commands.rs crates/memory-mcp/src/cli/commands/init.rs crates/memory-mcp/tests/init_non_mutating.rs README.md
git commit -m "feat: print host setup from init command"
```

---

# Stage 4 — Clarify the existing storage seam

### Task 7: Move storage-engine terminology into configuration without changing behavior

**Files:**
- Modify: `crates/memory-mcp/src/config.rs`
- Modify: `crates/memory-mcp/src/config/surreal.rs`
- Modify: `crates/memory-mcp/src/storage/client.rs:89-100,139-187`
- Test: existing storage client tests plus a new configuration-selection unit test

**Interfaces:**
- Consumes: the existing private `DbEngine` enum (`Local`/`Remote`) and `SurrealConfig.embedded`.
- Produces: a configuration-owned `StorageBackend` enum with `Embedded` and `Remote` variants and `pub(crate) const fn from_embedded(bool) -> StorageBackend`; `SurrealDbClient::connect` consumes it without changing the public MCP or CLI surface.

- [ ] **Step 1: Write a failing unit test for backend selection.**

Define the exact internal contract before implementing it:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StorageBackend {
    Embedded,
    Remote,
}

impl StorageBackend {
    pub(crate) const fn from_embedded(embedded: bool) -> Self {
        if embedded { Self::Embedded } else { Self::Remote }
    }
}

#[test]
fn storage_backend_selection_follows_embedded_flag() {
    assert_eq!(StorageBackend::from_embedded(true), StorageBackend::Embedded);
    assert_eq!(StorageBackend::from_embedded(false), StorageBackend::Remote);
}
```

- [ ] **Step 2: Run the focused test and verify it fails.**

```bash
cargo test -p memory_mcp storage_backend_selection_follows_embedded_flag -- --exact
```

- [ ] **Step 3: Implement the smallest internal enum and adapt the client.**

Define `StorageBackend` in `crates/memory-mcp/src/config/surreal.rs`, re-export it as `pub(crate)` from `crates/memory-mcp/src/config.rs`, and keep `DbEngine` in `storage/client.rs` as the connection-value enum. In `SurrealDbClient::connect`, replace the direct `if config.embedded` branch with `match StorageBackend::from_embedded(config.embedded)`, leaving `connect_embedded`, `connect_remote`, and all query operations unchanged. Do not add a second storage abstraction or rewrite query code.

- [ ] **Step 4: Run the exact backend-selection test and the full relevant library/integration tests.**

```bash
cargo test -p memory_mcp storage_backend_selection_follows_embedded_flag -- --exact
cargo test -p memory_mcp --lib
cargo test -p memory_mcp --test service_integration
cargo test -p memory_mcp --test tools_e2e
```

Do not use `cargo test -p memory_mcp storage` as a proxy for storage coverage: it is only a name filter and does not necessarily match the existing `with_db_retry_*` tests.

- [ ] **Step 5: Commit the internal seam clarification.**

```bash
git add crates/memory-mcp/src/config.rs crates/memory-mcp/src/config crates/memory-mcp/src/storage/client.rs
git commit -m "refactor: clarify storage backend selection"
```

This stage must not alter default behavior or add a public tool/subcommand.

---

# Stage 5 — Make installation fast and obvious

### Task 8: Extend the existing release matrix and document supported installs

**Files:**
- Modify: `.github/workflows/ci.yml:215-291` (the existing `build_binaries` release job; do not create a duplicate workflow)
- Modify: `README.md`
- Modify: `docs/agent_integration/CONTRACT.md` only where installation/setup is described

**Interfaces:**
- Consumes: the existing workspace release profile (`strip = true`, LTO enabled), the current four-target release matrix, the binary package name, and the zero-config CLI.
- Produces: release assets for the already-supported Linux x86_64, macOS x86_64, macOS arm64, and Windows x86_64 targets, with native binary smoke tests and a verified install path that does not require a local Rust toolchain when a release binary is available.

- [ ] **Step 1: Write the failing release-matrix smoke-test requirement against the existing workflow.**

Before editing, record the current matrix from `.github/workflows/ci.yml`: `x86_64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, and `x86_64-pc-windows-msvc`. Do not add `aarch64-unknown-linux-gnu` in this task; it requires a cross toolchain and a non-native validation strategy. Do not remove Windows from the advertised release set.

- [ ] **Step 2: Add native smoke tests to the existing `build_binaries` job.**

After the existing artifact-preparation steps and before checksum/upload, add a Unix step that captures and parses the complete stdout document:

```yaml
- name: Smoke test binary (Unix)
  if: matrix.platform.os != 'windows-latest'
  run: |
    target/${{ matrix.platform.target }}/release/memory_mcp --version
    target/${{ matrix.platform.target }}/release/memory_mcp init --target vscode > init-output.json
    python3 -c 'import json; from pathlib import Path; payload=json.loads(Path("init-output.json").read_text()); assert payload["mutates_files"] is False; assert payload["target"] == "vscode"'
```

Add a Windows PowerShell equivalent that validates the same fields:

```yaml
- name: Smoke test binary (Windows)
  if: matrix.platform.os == 'windows-latest'
  shell: pwsh
  run: |
    target/${{ matrix.platform.target }}/release/memory_mcp.exe --version
    target/${{ matrix.platform.target }}/release/memory_mcp.exe init --target vscode | Out-File -Encoding utf8 init-output.json
    $payload = Get-Content init-output.json -Raw | ConvertFrom-Json
    if ($payload.mutates_files -ne $false -or $payload.target -ne 'vscode') { throw 'invalid init smoke-test result' }
```

Do not upload `init-output.json`; leave it outside the `app/` artifact paths or remove it after validation. Do not launch the MCP server because the release job has no protocol client. Keep the workflow’s current `release` trigger and permissions, and add `--locked` to the existing release build command.

- [ ] **Step 3: Document install alternatives using commands that exist today.**

Document release binary installation first. For a repository checkout, document exactly:

```bash
cargo install --path crates/memory-mcp --locked
```

Do not document `cargo install memory_mcp --locked`: the repository does not establish a crates.io publication. State the expected first-run command and the five-minute goal as a measured target, not a guarantee.

- [ ] **Step 4: Run the pre-feature-gating local release validation.**

```bash
cargo build --release --locked -p memory_mcp
./target/release/memory_mcp --version
./target/release/memory_mcp init --target vscode
```

This Stage 5 command validates the artifact before optional Task 10 changes the dependency graph. After Task 10, use its separate full-provider and slim `--target-dir` commands instead of treating this featureless build as the published artifact. Record the stripped binary size. Do not enforce `< 200 MB` as a hard claim until the release artifact is measured on every advertised platform; use it as an observation target. A future Linux arm64 target requires a separate cross-build plan.

- [ ] **Step 5: Commit the release packaging slice.**

```bash
git add .github/workflows/ci.yml README.md docs/agent_integration/CONTRACT.md
git commit -m "build: smoke test zero-config release binaries"
```

---

# Stage 6 — Measure time-to-value and prevent regressions (prerequisite for Task 10)

### Task 9: Add a clean-machine TTV measurement rig

**Files:**
- Create: `scripts/measure_ttv.sh`
- Create: `scripts/test_measure_ttv.sh`
- Create: `scripts/measure_ttv_fixtures/invalid.json`
- Create: `scripts/measure_ttv_fixtures/missing-result.json`
- Create: `scripts/measure_ttv_fixtures/empty-facts.json`
- Create: `scripts/measure_ttv_fixtures/fallback-only.json`
- Modify: `README.md` with the measurement contract
- Test: shell syntax, JSON validator behavior, and fixture checks

**Interfaces:**
- Consumes: a release binary or explicit checkout path for `cargo install`, `memory_mcp init`, zero-config embedded storage, and the existing CLI `ingest`, `extract`, and `assemble-context` commands.
- Produces: a machine-readable result with timings for install, init, episode write, extraction, fact recall, and total time.

- [ ] **Step 1: Define the measurement contract.**

The script must support these three operationally distinct personas:

```bash
scripts/measure_ttv.sh --binary ./memory_mcp --persona release-binary
scripts/measure_ttv.sh --cargo-install --source . --persona rust-user
scripts/measure_ttv.sh --binary ./memory_mcp --persona host-config-user
```

For `release-binary`, resolve the supplied `--binary` path to `BIN` and count installation time as zero. For `rust-user`, run `cargo install --path "$SOURCE/crates/memory-mcp" --locked --root "$TEMP/install"`, measure installation/download/build time, and set `BIN="$TEMP/install/bin/memory_mcp"`. For `host-config-user`, run `"$BIN" init --target vscode`, parse the outer JSON, parse its `snippet` as JSON, and copy that snippet into a temporary `.vscode/mcp.json`; this persona measures host-registration preparation, not launching a real GUI host. Do not add a GUI-host claim to this metric.

For every persona, use temporary `HOME`, `XDG_DATA_HOME`, `CARGO_HOME`, and working directory so prior user state cannot make the result look faster. The script must not use the developer’s existing database or Cargo cache unless the persona explicitly measures a warm-cache run. Before launching any application command, unset every application configuration variable: `SURREALDB_URL`, `SURREALDB_EMBEDDED`, `SURREALDB_DB_NAME`, `SURREALDB_NAMESPACES`, `SURREALDB_USERNAME`, `SURREALDB_PASSWORD`, `SURREALDB_DATA_DIR`, `RUST_LOG`, `QUERY_LOGGING_ENABLED`, `QUERY_LOG_RETENTION_DAYS`, all `LIFECYCLE_*`, all `EMBEDDINGS_*`, `SURREALDB_EMBEDDING_DIMENSION`, and all `NER_*` variables. Then set only the temporary `HOME`, `XDG_DATA_HOME`, `CARGO_HOME`, `PATH`, and working directory values. Before the first measured command, create the selected new path with `mkdir -p "$XDG_DATA_HOME/memory_mcp"`; this forces the new-path preference and prevents a legacy `<BIN parent>/data/surrealdb` from contaminating a clean-machine measurement.

- [ ] **Step 2: Write shell assertions for the first-value path.**

The script must fail if any command fails or emits invalid JSON. After resolving the persona’s executable to `BIN`, the first-value path is explicitly:

```bash
INIT_JSON="$TEMP/init.json"
INGEST_JSON="$TEMP/ingest.json"
EXTRACT_JSON="$TEMP/extract.json"
CONTEXT_JSON="$TEMP/context.json"
"$BIN" init --target vscode > "$INIT_JSON"
"$BIN" ingest --source-type note --source-id ttv-fixture --content "Ada owns the memory MCP project." --t-ref 2026-08-04T00:00:00Z > "$INGEST_JSON"
episode_id="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["result"])' "$INGEST_JSON")"
"$BIN" extract --episode-id "$episode_id" > "$EXTRACT_JSON"
"$BIN" assemble-context --query "Who owns the memory MCP project?" --scope org > "$CONTEXT_JSON"
```

Capture each command’s stdout in a temporary file before parsing so stderr logs cannot be mistaken for the JSON result. Extract the response validator into an internal function used by both the live path and `scripts/test_measure_ttv.sh`; the validator must return nonzero for malformed JSON, missing top-level `result`, empty `result.facts`, and a context result containing only `episode_fallback:` items. For `host-config-user`, validate and write the VS Code `snippet` to `WORKDIR/.vscode/mcp.json` before running the same first-value path.

Parse the ingest JSON as a `ToolResponse<String>` and read the episode ID from the top-level `result` string; do not expect a nested episode-ID object. Pass that exact ID to `extract`. Parse extraction JSON as `ToolResponse<ExtractResult>` and require `result.facts` to contain at least one fact. Parse assemble-context JSON as the existing `ToolResponse<serde_json::Value>` envelope and inspect its top-level `result` array; require at least one item whose `fact_id` is present and does not start with `episode_fallback:` and whose `content` or `quote` contains both `Ada` and `memory MCP`. A raw episode fallback is not a successful fact recall. Use Python 3’s standard-library `json` module for parsing; this is a measurement-harness dependency, not an application runtime dependency.

- [ ] **Step 3: Add validator fixtures and the negative test harness.**

Create these exact fixture contents:

`scripts/measure_ttv_fixtures/invalid.json` contains:

```text
not-json
```

`scripts/measure_ttv_fixtures/missing-result.json` contains:

```json
{"status":"success"}
```

`scripts/measure_ttv_fixtures/empty-facts.json` contains:

```json
{"status":"success","result":{"facts":[]}}
```

`scripts/measure_ttv_fixtures/fallback-only.json` contains:

```json
{"status":"success","result":[{"fact_id":"episode_fallback:episode:test","content":"Ada owns the memory MCP project."}]}
```

Expose the validator through the script-only command `scripts/measure_ttv.sh --validate-fixture KIND PATH`, where `KIND` is `json`, `facts`, or `context`. `scripts/test_measure_ttv.sh` must run these exact negative cases and require every command to fail: `json invalid.json`, `json missing-result.json`, `facts empty-facts.json`, and `context fallback-only.json`. Run it with `bash scripts/test_measure_ttv.sh`; expected result: four rejected fixtures and exit 0.

- [ ] **Step 4: Implement timing output.**

Print JSON with one sample per attempted run plus explicit aggregate fields; with `--repeat 1`, the median and p90 equal that sample:

```json
{
  "persona": "release-binary",
  "runs": 1,
  "samples": [
    {
      "install_seconds": 0,
      "init_seconds": 0,
      "episode_write_seconds": 0,
      "extraction_seconds": 0,
      "fact_recall_seconds": 0,
      "total_seconds": 0,
      "success": true
    }
  ],
  "median_seconds": {
    "install_seconds": 0,
    "init_seconds": 0,
    "episode_write_seconds": 0,
    "extraction_seconds": 0,
    "fact_recall_seconds": 0,
    "total_seconds": 0
  },
  "p90_seconds": {
    "install_seconds": 0,
    "init_seconds": 0,
    "episode_write_seconds": 0,
    "extraction_seconds": 0,
    "fact_recall_seconds": 0,
    "total_seconds": 0
  },
  "success": true
}
```

Add `--repeat N` with default `1`; for every run, use monotonic timestamps from the shell’s available timing mechanism and fresh temporary state. Aggregate each timing field in Python with `statistics.median` and nearest-rank p90 at index `max(0, math.ceil(0.90 * N) - 1)` and import `math` in the validator after sorting each field. Do not report a successful aggregate if any run fails its recall assertion.

- [ ] **Step 5: Run three local personas and record the baseline.**

Run each persona with `--repeat 5` on a clean temporary state and use the script’s `median_seconds` and `p90_seconds` fields. The target is median total time `<= 300` seconds from the selected persona’s start to a real fact recall; document which component dominates when it is missed. Do not combine a warm model/cache run with a clean-machine run.

- [ ] **Step 6: Add regression guidance.**

Document that release download/build time, model download, host configuration preparation, database initialization, episode write, extraction, and fact recall are separate measurements. Do not use the existing memory evaluation harness as a TTV proxy: it evaluates memory quality, not onboarding latency. State clearly that the host-config-user persona validates a pasteable host snippet but does not measure GUI host startup.

- [ ] **Step 7: Run validation and commit the measurement rig.**

```bash
bash -n scripts/measure_ttv.sh
bash -n scripts/test_measure_ttv.sh
bash scripts/test_measure_ttv.sh
scripts/measure_ttv.sh --binary ./target/release/memory_mcp --persona release-binary --repeat 1
cargo fmt --all --check
```

```bash
git add scripts/measure_ttv.sh scripts/test_measure_ttv.sh scripts/measure_ttv_fixtures README.md
git commit -m "test: measure install to first recalled fact"
```

---

# Stage 7 — Optional slim build after evidence

### Task 10: Feature-gate the ML stack only after the TTV baseline exists

**Files:**

- Modify: `crates/memory-mcp/Cargo.toml` to add `ml` and `slim` features and make Candle, `hf-hub`, and `tokenizers` optional
- Modify: `crates/memory-mcp/src/service/embedding.rs`
- Modify: `crates/memory-mcp/src/service/embedding/local.rs`
- Modify: `crates/memory-mcp/src/service/entity_extraction.rs`
- Modify: `crates/memory-mcp/src/service/entity_extraction/gliner.rs`
- Modify: `crates/memory-mcp/src/service.rs` to gate the `model_loader` module and the public `GlinerEntityExtractor` re-export
- Modify: `crates/memory-mcp/src/service/model_loader.rs`
- Modify: `crates/memory-mcp/tests/local_model_integration.rs` to run only with the `ml` feature
- Modify: `crates/eval-harness/Cargo.toml` to add `ml = ["memory_mcp/ml"]` and mark the GLiNER `ner_cpu` benchmark `required-features = ["ml"]`
- Create: `crates/memory-mcp/tests/slim_feature_errors.rs` for ML-only provider failure behavior
- Modify: `.github/workflows/ci.yml` only; keep the existing `build_binaries` release job
- Test: feature-matrix compile, dependency-tree, startup, and slim-feature error tests

**Interfaces:**
- Consumes: the completed Task 9 baseline evidence and the existing `EmbeddingProviderKind::Disabled` and `NerProviderKind::Anno` paths.
- Produces: an explicitly named `slim` feature that compiles without ML inference dependencies while preserving embedded storage, ingest, extraction through Anno, and lexical recall. The existing full local-provider path is selected by an explicit `ml` feature; `default = []` remains unchanged.

- [ ] **Step 1: Enforce the prerequisite ordering.**

Do not implement this task based solely on dependency count. Execute all Task 9 steps first, record compile/download/startup/model-download breakdowns, and attach the measured result to the decision. The executable order is Tasks 1–8, then Task 9, then Task 10 if the baseline shows that feature-gating is worth its compatibility cost.

- [ ] **Step 2: Write feature-matrix compile tests.**

The required commands are:

```bash
cargo check -p memory_mcp --no-default-features --locked
cargo check -p memory_mcp --no-default-features --features slim --locked
cargo check -p memory_mcp --no-default-features --features ml --locked
cargo check -p memory_mcp --no-default-features --features cli-watch,mcp-apps --locked
cargo check -p eval-harness --all-targets --no-default-features --locked
cargo check -p eval-harness --all-targets --features ml --locked
```

Add this exact feature forwarding to `crates/eval-harness/Cargo.toml`:

```toml
[features]
default = []
ml = ["memory_mcp/ml"]
```

Set `required-features = ["ml"]` on the existing `[[bench]] name = "ner_cpu"` entry; the benchmark directly constructs `NerProviderKind::LocalGliner` and must not compile in the slim matrix.

The slim command must not pull Candle, `hf-hub`, or `tokenizers` into the dependency graph; the `ml` command must compile the existing full local-provider path. Use `cargo tree -p memory_mcp --locked -e normal --features slim` to verify that the slim graph contains none of those package names rather than relying on compile success.

- [ ] **Step 3: Gate imports and behavior explicitly.**

In `crates/memory-mcp/Cargo.toml`, use `default = []`, `slim = []`, and `ml = ["dep:candle-core", "dep:candle-nn", "dep:candle-transformers", "dep:hf-hub", "dep:tokenizers"]`; make the five ML dependencies optional. Make `accelerate` and `metal` depend on `ml` before enabling their Candle subfeatures. Gate `service::embedding::local`, `service::entity_extraction::gliner`, and `service::model_loader` behind `feature = "ml"`; keep the disabled embedding provider and Anno extractor available in `slim`. In `crates/memory-mcp/src/service.rs`, keep the non-ML re-exports unconditional and move `GlinerEntityExtractor` into a separate `#[cfg(feature = "ml")] pub use entity_extraction::GlinerEntityExtractor;` item. Add `#![cfg(feature = "ml")]` to `crates/memory-mcp/tests/local_model_integration.rs` so the integration tests are not compiled or run in the slim matrix. In `crates/memory-mcp/src/service/entity_extraction.rs`, put `#[cfg(feature = "ml")]` on `mod gliner`, the `pub use gliner::GlinerEntityExtractor` item, the `BACKENDS` registry entry whose `kind` is `LocalGliner`, and GLiNER-specific unit-test assertions. Replace the registry’s `.expect("unsupported NER provider configured")` with an explicit `MemoryError::ConfigInvalid` when no provider entry is available; in slim, `NerProviderKind::LocalGliner` must therefore fail descriptively rather than panic. In `crates/memory-mcp/src/service/embedding.rs`, put the local-provider import/module and local-Candle match arm behind `feature = "ml"`, and return `MemoryError::ConfigInvalid` for that provider in slim. Gate local-provider tests with the same feature. When a slim build receives `EMBEDDINGS_PROVIDER=local-candle`, `NER_PROVIDER=local-gliner`, or another ML-only request, return a descriptive `MemoryError::ConfigInvalid` rather than compiling a partial provider. Keep the release job on the current full-provider feature selection until the evidence review explicitly approves a slim artifact.

- [ ] **Step 4: Add slim-feature runtime error tests.**

In `crates/memory-mcp/tests/slim_feature_errors.rs`, invoke the compiled binary with `env_clear()`, `SURREALDB_EMBEDDED=true`, and an isolated temporary `SURREALDB_DATA_DIR`. One test must set `EMBEDDINGS_ENABLED=true` and `EMBEDDINGS_PROVIDER=local-candle`; a second must set `NER_PROVIDER=local-gliner` and run the smallest command that initializes the extractor. Both tests must require a nonzero exit and stderr containing `ConfigInvalid` plus the provider name, proving slim rejects ML-only requests instead of panicking or attempting a model download.

Run:

```bash
cargo test -p memory_mcp --no-default-features --features slim --test slim_feature_errors --locked
```

- [ ] **Step 5: Add CI coverage for full and slim builds.**

Run both `slim` and `ml` feature matrices in the existing `.github/workflows/ci.yml`, including `cargo test -p memory_mcp --features ml --test local_model_integration --locked` for the full provider path, `cargo check -p eval-harness --all-targets --features ml --locked`, and the zero-config TTV path without `ml`. Add `cargo bench -p eval-harness --bench ner_cpu --features ml` to the benchmark-only validation instructions. Keep the published full-provider artifact in the existing `target/${{ matrix.platform.target }}/release` path: build it with `cargo build -p memory_mcp --release --target ${{ matrix.platform.target }} --locked --features ml`, run the existing `init` smoke test, and upload that binary. Build the slim comparison separately with `CARGO_TARGET_DIR=target/slim cargo build -p memory_mcp --release --target ${{ matrix.platform.target }} --locked --no-default-features --features slim`; smoke-test `target/slim/${{ matrix.platform.target }}/release/memory_mcp` but do not upload it through the existing full-artifact upload step. This prevents the slim comparison build from overwriting the published full binary. Compare release sizes and TTV; publish slim only in a separately named artifact workflow after explicit evidence approval. For the Windows matrix, use a separate PowerShell step with `$env:CARGO_TARGET_DIR = target/slim` and smoke-test `target/slim/${{ matrix.platform.target }}/release/memory_mcp.exe`; the Unix step uses `CARGO_TARGET_DIR=target/slim` and the non-`.exe` path.

The Windows slim step must be explicit PowerShell rather than POSIX assignment:

The Unix slim steps use the equivalent environment assignment and non-`.exe` path:

```yaml
- name: Build slim comparison (Unix)
  if: matrix.platform.os != 'windows-latest'
  env:
    CARGO_TARGET_DIR: target/slim
  run: cargo build -p memory_mcp --release --target ${{ matrix.platform.target }} --locked --no-default-features --features slim

- name: Smoke slim comparison (Unix)
  if: matrix.platform.os != 'windows-latest'
  run: target/slim/${{ matrix.platform.target }}/release/memory_mcp init --target vscode
```

```yaml
- name: Build slim comparison (Windows)
  if: matrix.platform.os == 'windows-latest'
  shell: pwsh
  env:
    CARGO_TARGET_DIR: target/slim
  run: cargo build -p memory_mcp --release --target ${{ matrix.platform.target }} --locked --no-default-features --features slim

- name: Smoke slim comparison (Windows)
  if: matrix.platform.os == 'windows-latest'
  shell: pwsh
  run: target/slim/${{ matrix.platform.target }}/release/memory_mcp.exe init --target vscode
```

- [ ] **Step 6: Commit only after the evidence review.**

```bash
git add crates/memory-mcp/Cargo.toml crates/eval-harness/Cargo.toml crates/memory-mcp/src crates/memory-mcp/tests/local_model_integration.rs crates/memory-mcp/tests/slim_feature_errors.rs .github/workflows/ci.yml
git commit -m "build: feature gate optional ML dependencies"
```

If the evidence does not justify shipping slim, keep the feature gate opt-in and keep the release job on `--features ml`; record the measured decision instead of forcing a contradictory default.

---

# Final validation gate

After the stages selected for implementation are complete, run the narrow tests first and then the mandatory project gate:

```bash
cargo fmt --all --check
cargo test -p memory_mcp config::
cargo test -p memory_mcp init::tests
cargo test -p memory_mcp --test agent_memory_lifecycle_release_gate
cargo test -p memory_mcp --test service_integration
cargo test -p memory_mcp --test tools_e2e
cargo test --workspace --lib --bins --tests --locked
cargo test -p memory_mcp --features mcp-apps --locked
cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings
```

Also run:

```bash
cargo check --workspace --all-targets --locked
# For the first-release slices before Task 10:
cargo build --release --locked -p memory_mcp
# When Task 10 is selected, build and smoke-test both artifacts in separate target directories:
cargo build --release --locked -p memory_mcp --no-default-features --features slim --target-dir target/zero-config-slim
cargo build --release --locked -p memory_mcp --no-default-features --features ml --target-dir target/full-provider
./target/zero-config-slim/release/memory_mcp init --target vscode
./target/full-provider/release/memory_mcp init --target vscode
```

Acceptance criteria:

- Empty `SURREALDB_*` environment starts embedded configuration with `memory`, namespace `org`, and `root/root`.
- URL inference classifies only `ws`, `wss`, `http`, and `https` as remote; `mem://` is not treated as remote.
- Fresh installs default to a user-owned XDG/home path; an existing legacy executable-relative directory is selected only by the explicit compatibility rule and is surfaced by `config.legacy_data_dir_detected`.
- Default application produces structured `config.default_applied` events without exposing secrets, and the legacy-path event contains only its path.
- Config-related CLI failures include hints; unrelated errors do not.
- `memory_mcp init` supports the five exact targets and performs no file or database mutation.
- The eight MCP tools remain unchanged.
- The frozen ordinary CLI snapshot includes the authorized `init` command.
- Task 9 is executed before optional Task 10; the TTV rig proves a real ingest-extract-to-recall round trip, rejects episode fallback, and reports median/p90 rather than an unverified estimate.
- Stages 5 and 7 do not claim binary-size or cold-start improvements that were not measured.

---

# Self-review

## Spec coverage

- Zero-config embedded defaults: Task 2.
- XDG-conventional data directory, real empty-environment loader coverage, and broken executable-relative behavior: Tasks 1–2.
- URL inference with `mem://` explicitly non-remote: Tasks 1–2.
- `Anno` retained as the verified NER default: global constraints, the explicit `create_entity_extractor_defaults_to_anno` test, README update, and the slim-build non-ML path.
- Embedded `root/root` authentication verification: Task 2, Step 9.
- Friendly configuration failures: Task 3.
- Public `init` command, exact targets, subprocess non-mutation, and host-native formats: Tasks 4–6.
- ADR requirement and frozen CLI snapshot: Tasks 4–5.
- Existing storage seam rather than invented wholesale abstraction: Task 7.
- Release binaries and realistic build-size caveat: Task 8.
- Deferred Candle feature-gating, including parent-module re-exports and eval-harness feature forwarding: Task 10, explicitly gated on the Task 9 baseline and retaining `default = []`.
- Native host snippet schemas and the output-only/non-mutating `init` contract: Task 6, with VS Code, Claude Desktop, Codex TOML, Zed, and shell fixtures.
- Boxed startup configuration failures using the same hint envelope as one-shot command failures: Task 3.
- Legacy executable-relative data compatibility without automatic live-RocksDB copying: Tasks 1–2.
- Live Clap CLI-surface comparison rather than only editing the self-referential snapshot constant: Task 5.
- Separate TTV rig, real ToolResponse-shaped parsing, median/p90 aggregation, and negative fixtures rather than misusing the evaluation harness: Task 9.
- Required formatting, test, and strict clippy validation: Final validation gate.

## Placeholder scan

No unresolved placeholder markers remain. Every implementation stage names exact files, interfaces, commands, expected failures, and expected passing behavior. Task 10 is intentionally conditional, but its prerequisite and evidence requirements are explicit.

## Type consistency

- `InitArgs.target: String` is consumed by `commands::init::run(args: InitArgs)`.
- `parse_target(&str) -> Result<InitTarget, MemoryError>` feeds `render(InitTarget) -> Result<serde_json::Value, MemoryError>`.
- `StorageBackend` is internal and distinct from the existing connection-value `DbEngine`.
- `is_remote_url(Option<&str>) -> bool` and `normalize_url_scheme(&str) -> String` are consumed by `SurrealConfig::from_env()`; `storage::helpers::normalize_url` applies the same scheme normalization to builder-created remote configurations.
- `default_user_data_dir() -> String` feeds `EmbeddedDataDirResolution`, whose `path` and `legacy_path` fields are consumed by the existing `default_embedded_data_dir()` compatibility wrapper and `SurrealConfig::data_dir_or_default()`.
- `report_cli_error(MemoryError) -> ExitCode` retains its existing signature while using the exact `cli_error_json(&MemoryError) -> serde_json::Value` helper.
- `startup_config_events(&SurrealConfig) -> Vec<HashMap<String, serde_json::Value>>` is built before storage connection and emits only non-secret default/legacy metadata.
- TTV parses ingest as `ToolResponse<String>`, extraction as `ToolResponse<ExtractResult>`, and context assembly as the existing `ToolResponse<serde_json::Value>` envelope whose `result` is an array; it rejects `episode_fallback:` context items.

## Corrections from the previous draft

- The frozen test path is `crates/memory-mcp/tests/agent_memory_lifecycle_release_gate.rs`, not repository-root `tests/...`.
- The Anno-default regression is specified by the exact test name `create_entity_extractor_defaults_to_anno`; line numbers are intentionally not treated as a stable interface because the backend registry shape may change during the baseline cleanup.
- The next available ADR is `0029`, because the repository already contains ADRs through `0028`; Stage 3 explicitly depends on ADR-0029.
- `data_dir_or_default()` already exists in `config/surreal.rs`; the plan updates its helper and its existing broken expectation rather than inventing a second data-directory API.
- The plan does not claim that the current ML stack is dependency-free or that a slim binary is already available.
