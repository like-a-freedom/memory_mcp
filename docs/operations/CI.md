# CI and releases

## Everyday flow

1. Open a pull request. **CI** runs Linux lint, workspace tests, optional-feature
   tests and the PR evaluation gate. Native build/test jobs start after the
   quality gate passes.
2. Require **CI passed** in branch protection. The required set follows the
   event: pull requests, manual runs and releases must pass every job; a push
   to `master` must pass the quality job (the build jobs do not start there
   and must be skipped, never failed or cancelled). Windows failures are
   mandatory.
3. Merge to `master`. The quality job checks the merged commit; the native and
   image jobs do not start on push — the pull request already ran them and the
   release rebuilds with the release profile. Feature-branch pushes do not
   also trigger a duplicate push workflow.
4. A release run publishes the Linux/amd64 Streamable HTTP image to GitHub
   Container Registry as `ghcr.io/like-a-freedom/memory_mcp:latest`,
   `sha-<commit>` and the release tag (a manual run started from `master`
   refreshes `latest` and `sha-<commit>`). Pull requests build and test the
   image without publishing it.
5. Publish a GitHub Release on a tag containing these workflows. **Release**
   calls the same CI with optimized builds and the release evaluation gate,
   publishes the matching container tag, then uploads assets only after every
   job passes. Pushing a tag alone does not start another build. Re-run failed
   jobs from the original Actions run.

Manual **CI** runs build development binaries. Manual **Release** rebuilds must
select the existing release tag under **Run workflow** and supply the same tag
as input. Old tags retain their old workflows: re-running `v1.9.10` does not
retroactively use a newer workflow. Create a new version/tag after merging.

## Platforms

| OS | x64 | ARM64 |
|---|---|---|
| Linux | `ubuntu-24.04` | `ubuntu-24.04-arm` |
| macOS | — | `macos-15` |
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
process tests run on Linux and macOS during ordinary CI. Windows x64 and ARM64
run the same packaged executable smoke checks after building, which exercises
the actual shipped binaries and bundled runtime libraries without the
unreliable hosted Windows Rust test harness. Release platform jobs use those
smoke checks as well and skip compiling a second test profile.

## Why these build settings matter

- `setup-rust-toolchain` defaults to `RUSTFLAGS=-D warnings`. Our setup
  explicitly disables that override; Clippy supplies `-D warnings` itself.
  Windows and its native dependencies use the toolchain's dynamic MSVC CRT.
- Cache policy is explicit: successful dependency builds are keyed by target
  and role, and the Linux x64 `dev` cache is **shared** between the quality job
  and the native Linux x64 job (`cache-shared-key`) — the platform build
  restores the dependency tree the gate just compiled instead of rebuilding it.
  Release builds keep their own separate key. Failed builds are not saved. Pull
  requests restore caches and save their own branch-scoped cache, so later
  pushes of the same PR reuse the first build; GitHub expires these caches on
  inactivity. Rust, lockfile and Cargo config
  changes invalidate Rust caches. Bump `native-v3` if changing native compiler
  policy outside Cargo configuration.
- The first uncached build is expensive: SurrealDB, RocksDB, ML libraries and
  release LTO still need compilation. Job timeouts are ceilings, not expected
  durations. Do not infer an optimization gain until comparing completed runs.

## Extended evaluations

**Extended evaluations** runs weekly or manually. It retains the nightly
evaluation profile, response-size checks, benchmark compilation, benchmark
linting and CPU Criterion runs without blocking ordinary builds. The Criterion
targets live behind the `bench` feature of `eval-harness` (`required-features`
on each `[[bench]]`), so the ordinary quality gate never compiles them; this
workflow enables the feature for both the clippy pass over the bench targets
and `make bench-check`. NER model quality checks
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

The `unittest` line above also pins two contracts that CI would otherwise leave
to a comment: the image harness's scenario registry and the browser runner's
(`test_local_admin_image.py`), and the Dioxus CLI version the console's bundle is
built with against the `dioxus` requirement in the UI crate's manifest
(`test_ui_bundle_pin.py`). Both are stdlib-only, so they run in the quality job
without Docker, Node or a browser.
