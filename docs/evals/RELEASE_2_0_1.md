# Release 2.0.1: recovery from the 2.0.0 pipeline failure

The [initial v2.0.0 release pipeline](https://github.com/like-a-freedom/memory_mcp/actions/runs/45898634886)
failed. Its native release profile passed 154/154 and response-size passed 62/62;
remote compatibility passed against SurrealDB 3.3.5 and 4.0.1. However, the public
binary gate could not find the executable and the embedded conversion target
failed six tests. Packaging and publication were correctly skipped; no binary
assets were published for v2.0.0. Its tag is preserved.

## Causes and repairs

- CI configures `CARGO_BUILD_TARGET`, but the public gate used `target/debug`.
  The executable path now includes the configured target triple.
- The logging privacy boundary redacts dependency messages. Migration tests now
  observe the dependency's structured numeric IDs under its exact migration target,
  rather than search message text. The pinned 3.3.5 fixture has already applied
  migrations 1–3; interruption occurs at outstanding IDs 4 and 6. The actual
  interruption, resume, restart, record and old-SDK backup checks remain intact.
- HTTP maintenance failures lost operator instructions with the redacted error.
  A static guidance field now preserves offline, backup, opt-in and restore
  instructions, while arbitrary backend errors remain redacted. The instruction
  fits the existing 200-byte human-log budget; no privacy rule was relaxed.

The new public-process regression failed before the guidance repair and passed
afterward. The full pinned conversion suite passed 12/12 locally on Apple Silicon,
including interrupted conversion on both storage engines and both composition roots.

The manual recovery path had a separate ref contradiction: `release.yml` requires
its exact tag while the called CI workflow required master for every dispatch.
CI now accepts the exact dispatch tag when supplied by the release wrapper; direct
CI preflight remains master-only. Four routing controls executed the actual YAML
shell block, including rejection of mismatched tags and feature branches. These
configuration controls are not counted as production scenario coverage.

## Verification boundaries

The patch changes HTTP maintenance diagnostics and CI/test wiring. It does not
change memory retrieval, model inputs, source labels or acceptance thresholds.
Local release preflight passed: default workspace tests (4193 passed, 0 failed,
84 ignored), optional-feature tests (4225 passed, 0 failed, 75 ignored),
workspace clippy with the CI feature set, UI WASM clippy, formatting and
observability/toolchain/Dioxus checks. Totals include existing architecture lints
and are not counts of independent production scenarios. Ignored migration tests
were separately executed (12/12). See the [machine-readable preflight](../../evals/results/release-2.0.1/local-preflight.json).

Local release preflight and hosted publication are separate observations. The
post-publication artifact verification record is added after all hosted gates and
uploads finish; a published release page alone does not establish asset availability.

The [post-release memory-quality plan](../superpowers/plans/2026-11-26-post-release-memory-quality.md)
remains in effect. These repairs do not establish independent agent benefit or
close its outstanding quality work.
