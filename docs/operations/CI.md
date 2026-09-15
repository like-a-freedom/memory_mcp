# CI and releases

## Everyday flow

1. Open a pull request. **CI** runs Linux lint, workspace tests, optional-feature
   tests and the PR evaluation gate, alongside six native build/test jobs.
2. Require **CI passed** in branch protection. This check fails if any required
   job fails, is cancelled or is skipped. Windows failures are mandatory.
3. Merge to `master`. CI checks the merged commit. Feature-branch pushes do not
   also trigger a duplicate push workflow.
4. Publish a GitHub Release on a tag containing these workflows. **Release**
   calls the same CI with optimized builds and the release evaluation gate,
   then uploads assets only after every job passes. Pushing a tag alone does
   not start another build. Re-run failed jobs from the original Actions run.

Manual **CI** runs build development binaries. Manual **Release** rebuilds must
select the existing release tag under **Run workflow** and supply the same tag
as input. Old tags retain their old workflows: re-running `v1.9.10` does not
retroactively use a newer workflow. Create a new version/tag after merging.

## Platforms

| OS | x64 | ARM64 |
|---|---|---|
| Linux | `ubuntu-24.04` | `ubuntu-24.04-arm` |
| macOS | `macos-15-intel` | `macos-15` |
| Windows | `windows-2025` | `windows-11-arm` |

All jobs use native hosts and Rust **1.97.1**, matching `rust-version` and
`rust-toolchain.toml`. There are no 32-bit targets or emulated Linux builds.
Runner labels come from the [GitHub runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).

Each archive contains `memory_mcp` and `memory_mcp_http` with `fs-watch`,
`mcp-apps`, `streamable-http` and `control-plane`. It excludes the browser UI
and test fixtures. macOS Metal is linted separately; it is not enabled in the
portable release binaries. Windows uses ZIP; Unix uses tar.gz. Each download
has a SHA-256 sidecar. Standalone CLI filenames are retained when no adjacent
runtime library is required.

Before upload, the actual packaged executables must pass CLI version/init,
both ordinary NER selector ingestion paths, MCP initialization with an inbox,
and bounded shutdown. HTTP must load and reject invalid configuration with
its expected error code; this is a loader check, not a live HTTP deployment
test. HTTP integration tests run separately on Linux. Native unit and watcher
process tests run on all six platforms during ordinary CI; release platform
jobs use the executable smoke checks to avoid compiling a second test profile.

## Why these build settings matter

- `setup-rust-toolchain` defaults to `RUSTFLAGS=-D warnings`. That overrides
  `.cargo/config.toml` and drops Windows `+crt-static`. Our setup explicitly
  disables that override; Clippy supplies `-D warnings` itself. C/C++ and Rust
  therefore use the same static Windows CRT.
- Cache policy is explicit: successful dependency builds are cached separately
  by target and dev/release/quality role. Failed builds are not saved. Pull
  requests restore caches but do not save them. Rust, lockfile and Cargo config
  changes invalidate Rust caches. Bump `native-v3` if changing native compiler
  policy outside Cargo configuration.
- `ort-sys 2.0.0-rc.12` has no Intel macOS prebuilt. Changing runner labels cannot
  fix that. The Intel job builds ONNX Runtime 1.24.2 from pinned commit
  `058787ceead760166e3c50a0a4cba8a833a6f53f`, caches its libraries, and ships them
  with notices beside the binaries. It tests the bundle without a development
  library search path. Intel macOS users must keep the archive contents together.
- The first uncached build is expensive: SurrealDB, RocksDB, ML libraries and
  release LTO still need compilation. Job timeouts are ceilings, not expected
  durations. Do not infer an optimization gain until comparing completed runs.

## Extended evaluations

**Extended evaluations** runs weekly or manually. It retains the nightly
evaluation profile, response-size checks, benchmark compilation and CPU
Criterion runs without blocking ordinary builds. NER model quality checks
still require provisioned fixtures; missing fixtures are reported explicitly.
The lightweight PR/release evaluation baselines remain mandatory in CI.

## Local checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http,control-plane --locked -- -D warnings
cargo test --workspace --lib --bins --tests --locked
python3 -m unittest discover -s scripts/ci -p 'test_*.py'
actionlint
```

The shared setup is `.github/actions/setup/action.yml`; packaging and smoke
checks are in `scripts/ci/package.py`. No generated workflow files or custom
CI framework are involved.
