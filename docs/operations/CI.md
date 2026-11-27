# CI and releases

## Everyday flow

1. Open a pull request. **CI** runs Linux lint, workspace tests, optional-feature
   tests and the PR evaluation gate. Native and Docker build/smoke jobs run
   alongside quality checks. Their prospective release artifacts use
   `candidate-*` names; these jobs have no release or registry publication
   permission.
2. Require **CI passed** in branch protection. It fails on a failed, cancelled
   or unexpectedly skipped prerequisite, including Windows builds and the
   SurrealDB remote compatibility tests. From 2.0 the remote matrix tests only
   SurrealDB 4.0.1; older servers are outside the supported CI contract.
3. Push-triggered CI is currently disabled. Pull requests and manual CI runs
   validate changes; a plain branch or tag push publishes nothing.
4. For a release, bump and push the release commit on `master`, then dispatch
   **CI** on that exact commit with `release=true`. This runs all gates and
   optimized builds, records compiler timings, and seals `qualified-binaries`
   only after **CI passed** succeeds. A normal manual CI run uses development
   builds and cannot qualify as a reusable release candidate.
5. Tag that exact commit and create a **draft** GitHub Release with reviewed,
   nonempty notes. Dispatch **Release** on the existing tag, supply the same
   `tag` and the successful `candidate_run_id`. The wrapper checks the run's
   repository, workflow, event, branch, source SHA and successful completion;
   it also requires the release-only qualification receipt. Without a
   candidate run ID, the wrapper runs the full release-mode CI on the tag.
6. The publisher verifies every binary/checksum and the smoke-tested image
   archive against their source SHA/run ID, loads that image without rebuilding,
   and checks its OCI version/revision. It stages the registry image and refuses
   to replace a version or SHA alias with a different manifest digest. It then
   uploads assets to the draft, downloads them again and verifies every byte
   against the qualified bundle before publishing the draft.
7. Only after publication may a stable SemVer release advance the shared Docker
   `latest` and GitHub Latest marker. Publication jobs share a serialized queue;
   an older version cannot replace a newer one, prereleases cannot become
   stable `latest`, and the same version with a different digest is refused.
   New images carry `org.opencontainers.image.version` and `.revision` labels.
   For an old image without a version label, the publisher reads its CLI version
   by exact digest in a bounded container with no network, mounts or credentials.
   Unknown version/registry errors stop publication instead of guessing.

The build-once candidate expires after seven days. Use the tag rebuild path if
it has expired. Rerun failed jobs from the same Release run to finish a partial
publication, including a draft already published before a later step failed.
This reuses that run's artifacts and verifies existing public bytes. For a new
dispatch recovering an already public release, use **Release** with
`allow_published_recovery=true`; existing public asset bytes must match the
qualified bundle, only missing assets may be filled. Reuse the original
candidate run ID to avoid a rebuild changing already public bytes.
For the tag-build path without a master candidate, prefer rerunning failed
jobs of the original Release run: its run ID is not a master CI candidate ID.

GitHub and GHCR have no shared transaction. A failure can leave a public release
without an advanced `latest`, or staged/version image references while the
release stays draft; retries verify existing bytes before continuing. The
monotonic update protects publishers using this workflow, not external manual
tag overwrites. Do not bypass its lock and policy. Pushing a tag or publishing
a draft by hand does not dispatch **Release** automatically.

Manual **Release** must select the existing release tag under **Run workflow**
and supply the same tag as input. Old tags retain their old workflows: rerunning
an old release does not retroactively use this design. Create a new version/tag
after merging these workflow changes.

## CI improvement completion record — 2026-11-27

Local implementation is complete; hosted release acceptance remains pending.
No commit, push, workflow dispatch or registry/release mutation was performed
as part of this CI-only change.

| Work item | Local status and evidence |
|---|---|
| Overlap native/Docker builds with quality | Implemented; publication still requires every gate. Executed gate script rejects 54 failure/cancellation/skip controls and accepts three event controls. |
| Remove pre-4 SurrealDB CI support | Matrix is 4.0.1 only; removed the 3.x conversion/rollback row. Preserved 35 other check/build/smoke command bodies (native build adds timings). |
| Reuse caches without competing writers | Existing native key namespace retained; quality owns the shared Linux dev key. Docker exports installer layers; source copies exclude unrelated repository material. |
| Build once and qualify exact bytes | Source/run-bound image handoff and release bundle; explicit candidate-run provenance; no rebuild in the publisher. |
| Publish only verified release assets | Draft upload/download/hash verification precedes publication. Version/SHA image aliases reject different digests. |
| Protect stable channels | Independent GitHub/GHCR version checks, serialized publishers, no downgrade/prerelease promotion; both promoted pointers are read back. |
| Recover partial publication | Same-run retry reuses its artifacts; a fresh dispatch needs explicit recovery. Existing public bytes cannot be replaced; missing assets can be repaired. |
| Keep runbook and release skill consistent | Both describe draft/manual dispatch, seven-day artifact expiry, recovery and hosted verification. |

Verification: `cargo test -p xtask --locked` passed 78 existing/policy tests and
22 publisher process-integration scenarios. They exercise real qualification
policies through controlled GitHub/Docker transports, without external writes.
The new final-readback regression failed before the fix (a false-success GitHub
update was accepted), then passed with remote-state verification. Required
workspace Clippy, formatting, Bash syntax and workflow validation also pass.
The exact actionlint compatibility exception is documented below.

**Remaining acceptance on the next authorized release:** use the build-once
path above, verify every hosted gate and published binary/image, then compare
completed critical-path durations, compiler timings and cache hit/export logs
with the preceding comparable release. Record cold/warm cache and runner
differences; no measured speedup is claimed from local tests or DAG changes.
Unmeasured LTO/profile changes are outside this change.

## Platforms

| OS | x64 | ARM64 |
|---|---|---|
| Linux | `ubuntu-24.04` | `ubuntu-24.04-arm` — disabled (runner SIGTERM'd mid-build; see the commented matrix row in `ci.yml`) |
| macOS | — | `macos-15` |
| Windows | `windows-2025` | `windows-11-arm` |

Every enabled job runs on a native host using the rolling `stable` channel,
matching `rust-toolchain.toml`. The workspace `rust-version = 1.100.0` is the
minimum supported compiler, not a pinned toolchain. There are no 32-bit targets
or emulated Linux builds. Runner labels come from the
[GitHub runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).

The Dockerfile uses the official rolling `rust:slim-trixie` tags, and the CI
setup action selects `stable` as well. `cargo run -p xtask -- check-toolchain-pin`
requires those three selectors to agree and runs in the `quality` job; it does
not freeze the stable compiler version, which can advance upstream. The same job
runs `cargo run -p xtask -- check-observability` to keep the recording rules,
alerts and dashboards in step with the exported metrics.

Each archive contains `memory_mcp` and `memory_mcp_http` with `fs-watch`,
`mcp-apps`, `streamable-http` and `control-plane`. It excludes the browser UI
and test fixtures. macOS Metal is linted separately; it is not enabled in the
portable release binaries. Windows uses ZIP; Unix uses tar.gz. Each download
has a SHA-256 sidecar. Standalone CLI filenames are retained when no adjacent
runtime library is required.

Before upload, the actual packaged executables must pass CLI version/init,
both ordinary NER selector ingestion paths, MCP initialization with an inbox,
and bounded shutdown. HTTP must load and reject invalid configuration with
its expected error code and structured `http.serve_failed` event with
`error=[redacted]`. Error prose and variable names are private under the logging
contract; HTTP configuration tests separately verify invalid bind handling.
This is a loader check, not a live HTTP deployment test. HTTP integration tests
run separately on Linux. Native unit and watcher
process tests run on Linux and macOS during ordinary CI. Windows x64 and ARM64
run the same packaged executable smoke checks after building, which exercises
the actual shipped binaries and bundled runtime libraries without the
unreliable hosted Windows Rust test harness. Release platform jobs use those
smoke checks as well and skip compiling a second test profile.

## Why these build settings matter

- `setup-rust-toolchain` defaults to `RUSTFLAGS=-D warnings`. Our setup
  explicitly disables that override; Clippy supplies `-D warnings` itself.
  Windows and its native dependencies use the toolchain's dynamic MSVC CRT.
- Cache policy is explicit: the Linux x64 `dev` cache is shared by quality,
  evaluations, remote compatibility and native dev builds. Quality is its sole
  writer; concurrent readers restore an existing cache, not the current run's
  unfinished build. Other targets and release profiles own separate keys. The
  `native-v3` namespace is retained to reuse existing compatible caches. A
  failed owning job still saves what it
  compiled (`cache-on-failure: true`), so the next run reuses the dependency
  tree and rebuilds only what changed. This covers a job that fails but runs
  its post steps; a runner lost mid-build skips post steps and loses its cache,
  and needs a manual re-run — GitHub has no job-retry key, and `max-attempts`
  is not one (it makes GitHub reject the workflow file outright). Pull
  requests restore caches and save their own branch-scoped cache, so later
  pushes of the same PR reuse the first build; GitHub expires these caches on
  inactivity. Rust, lockfile and Cargo config
  changes invalidate Rust caches. Bump `native-v3` if changing native compiler
  policy outside Cargo configuration.
- Docker exports/imports a BuildKit GHA cache for `docker-linux-amd64`, including
  the expensive Dioxus installer layer. Cache export failure is nonfatal and
  bounded to five minutes. External layer caching does not export mutable Cargo
  cache mounts; changes to source still require compilation. Narrow source
  copies keep unrelated documentation/eval changes out of these build layers.
- The first uncached build is expensive: SurrealDB, SurrealKV, ML libraries
  and release LTO still need compilation. Job timeouts are ceilings, not expected
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
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http,test-fixtures --locked -- -D warnings
cargo test --workspace --lib --bins --tests --locked
cargo test -p xtask --locked
actionlint -ignore '^unexpected key "queue" for "concurrency" section\. expected one of "cancel-in-progress", "group"$'
```

As of actionlint 1.7.12, its concurrency parser lacks GitHub's documented
[`queue: max`](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/control-workflow-concurrency)
field. The command ignores exactly that obsolete diagnostic; all other lint
checks remain enabled. Validate the full workflow with the current
[workflow JSON schema](https://json.schemastore.org/github-workflow.json) as
well. Keep `cancel-in-progress: false` with `queue: max`: the default single
pending slot can replace older pending publishers.

The shared setup is `.github/actions/setup/action.yml`. Packaging and the
console-bundle contract live in `crates/xtask` (`cargo run -p xtask -- package`
and `cargo run -p xtask -- check-ui-bundle`). No generated workflow files or
custom CI framework are involved.

Policy checks are cargo commands. Release orchestration uses `gh`, Docker and
the tested `.github/scripts/publish-release.sh`; it invokes the `xtask`
artifact and channel policies. Its filesystem/process integration tests use
controlled transports and real qualification checks, without registry writes.
The Python tooling that
previously lived under `scripts/ci` has been removed: `package.py` became
`xtask package`, the Dioxus CLI pin and the bundle's output layout became
`xtask check-ui-bundle`, the embedded-UI checker became the `ui_assets`
integration test plus the `docker` job's own walk of the served document, and
the rest guarded code that either no longer exists or is covered by cargo
already. A guard that a job never executed was not protecting anything, and two
of those scripts had in fact failed CI for reasons unrelated to the tree they
were inspecting.
