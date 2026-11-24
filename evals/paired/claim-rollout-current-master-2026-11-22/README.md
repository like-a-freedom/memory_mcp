# Evidence default rollout record — 2026-11-22

## Current product behavior

`ClaimRolloutStage::default()` and `ClaimConfig::default()` select `evidence`.
The local and HTTP composition roots use that default when
`MEMORY_CLAIM_ROLLOUT_STAGE` is unset. `shadow` remains an explicit comparison
and rollback control. No default switch was needed in this audit. The missing
piece was sufficient deterministic safety evidence for the existing default;
the v7 capture exposed a real false positive in episode fallback, which was
fixed and tested before the final candidate was frozen.

The current preregistration and acceptance rules are
[`claim-stage-v9-preregistration.json`](../deterministic-quality-2026-11-22/claim-stage-v9-preregistration.json)
and [`claim-stage-v9-acceptance.json`](../deterministic-quality-2026-11-22/claim-stage-v9-acceptance.json).
The final raw captures, reports and hashes are in
[`ci-gate-v9-final/`](ci-gate-v9-final/).

## v9 paired result

The gate used one frozen product binary for both arms, two separate captures
per arm, and eight preregistered synthetic history families. The candidate arm
left the rollout setting unset; the control explicitly selected `shadow`.
Each case used a fresh embedded store and the public MCP ingestion, extraction
and read path. The evaluator and scoring were deterministic and needed no model
provider, Codex runner, account or API key.

| Primary public-MCP metric | Shadow control | Product default (`evidence`) |
|---|---:|---:|
| Cases with evidence success | 8/8 | 8/8 |
| Cases with decision success | 1/8 | 5/8 |

Evidence success did not regress in any case. Decision gains were
`peer-conflict`, `prior-knowledge`, `unverified-correction`, and
`valid-boundary`; there were no decision losses, candidate safety findings or
hard failures. The equal-family gain was 50 percentage points, above the
preregistered 25-point minimum. Both default captures and both shadow captures
had identical normalized results. The deterministic acceptance report is
[`claim-rollout-default-acceptance.json`](ci-gate-v9-final/claim-rollout-default-acceptance.json)
and records `accepted: true`.

This finite cohort supports keeping the existing default. It does not prove
population-wide improvement or increased agent productivity/usability.
Those require separately powered, held-out agent confirmation with resource
and workflow measurements.

The adversarial `absent-subject` case asks about `Ivory Hangar` while storing
`Jade Hangar`. Both share the common noun `Hangar`; the oracle forbids using the
stored event. The v9 default and shadow captures both abstained correctly. The
v7 default capture had returned the wrong episode. The issue was reproduced by
the public regression
`episode_fallback_does_not_answer_a_different_named_subject_with_a_shared_noun`
and fixed by applying the original query's explicit subject check only to
records that declare a structured subject. Ordinary episode admission retains
the cleaned query and its established conversation-neighborhood behavior.

The gate also reran the registered v4 compatibility acceptance; it completed
successfully and its separate report is `acceptance.json`. That compatibility
result is not a v9 score. v6 remains historical and is not valid evidence for
a true valid-time boundary: its claims overlapped at the requested time. v7–v9
use an event exactly at the cutoff and a forbidden event one minute later.

## Reproducibility

- Frozen product binary SHA-256: `e9c15dc133e38252064e9814e35aa8af49f65f828b10c173f56857993aae5034`
- Evaluator binary SHA-256: `347c170386117c89b43478d7c219423f3eaf20792ae444ec85944aafcb3a62ea`
- v9 corpus SHA-256: `40fdef00b8201de03df680d2ad9185038ead358372d5b5b3fedac66e55044ee3`
- v9 preregistration plan SHA-256: `e0ec60b0131c656dafe72890bf8586bc7dd631131d67136b8b0dabd8374b9095`
- Acceptance report's canonical plan identity: `a5f3746ac3e8be388f0258ce66fe9bb96bce1d34439862dd0a7075ae7d9ff561`

[`ci-gate-v9-final/SHA256SUMS`](ci-gate-v9-final/SHA256SUMS) records hashes
for the raw captures, reports, registered corpus and plan, profiles, and local
binaries used at capture time. The repository preserves their identities and
the portable captures for offline scoring.
The run evaluated the authorized current working tree (`product_revision:
ci-current`), not a clean commit. The result supports only the finite
registered synthetic cohort; it is not a population guarantee, production
latency result, or measured agent productivity gain.

## Preserved failed and aborted attempts

- `ci-gate-rejected-binary-identity/`: rejected after concurrent compilation
  replaced the executable between captures.
- `ci-gate-aborted-retrieval-regression/`: stopped after a regression found
  loss of a traceable graph neighbor; the product correction is covered by
  retrieval tests.
- `ci-gate-v7-preflight-aborted/`: stopped before v7 product capture because
  source values and expected oracle strings did not agree.
- `ci-gate-v7-partial-safety-failure/`: captured the v7 absent-subject false
  positive, then stopped before acceptance. It is not a successful rollout.
- `ci-gate-v8-final/`: accepted on its frozen finite cohort, but its binary was
  built before the full product test suite exposed an ordinary
  conversation-neighborhood regression. It is retained as historical evidence;
  v9 was frozen after fixing that regression and is the final current result.
- `ci-gate-final/`: accepted v6 artifacts, retained as historical only because
  the valid-time boundary oracle was semantically wrong.

## Post-lineage no-regression replay

The explicit-import and watcher-lineage credential checks changed the product
binary after the original v9 acceptance. The unchanged v9 corpus was therefore
replayed on one newly frozen binary (`5ddf5f8db2a4ce74c8093a29b827f2c60fc5eaf2f2f470920bf2ba0be05b2218`)
with the same shadow/default arms, acceptance thresholds, item ceiling and
separate fresh stores. Preregistration and plan are
[`claim-stage-v9-post-lineage-preregistration.json`](../deterministic-quality-2026-11-22/claim-stage-v9-post-lineage-preregistration.json)
and
[`claim-stage-v9-post-lineage-acceptance.json`](../deterministic-quality-2026-11-22/claim-stage-v9-post-lineage-acceptance.json).

Both repeats matched within each arm. The final acceptance report records 8/8
evidence success in each arm, primary decision success 1/8 for shadow and 5/8
for default, four family wins, zero losses, zero hard failures and no baseline
safety findings; `accepted: true`. This is the same finite synthetic cohort
used as a regression check on the new binary, not an independent population
sample or agent-usability result. Raw reports and receipts are retained under
[`ci-gate-v9-post-lineage/`](ci-gate-v9-post-lineage/), including the accepted
`acceptance.json` and its two exact replay comparisons.

## Final verification build replay

After source-lineage validation and final logging/error-redaction cleanup, the
frozen v9 gate was run again on product binary
`4ec78618868d35e38f6832716c26973420bfb7e3976ccba141dd05738b0c141c`. This
replay was preregistered before capture in
[`claim-stage-v9-final-build-preregistration.json`](../deterministic-quality-2026-11-22/claim-stage-v9-final-build-preregistration.json)
against
[`claim-stage-v9-final-build-acceptance.json`](../deterministic-quality-2026-11-22/claim-stage-v9-final-build-acceptance.json).
The gate also reran registered v4 compatibility acceptance.

The four v9 captures used the same frozen eight-family corpus, separate fresh
stores, the same product binary, and the `shadow` and product-default
(`evidence`) settings. Both repeats within each arm matched through the
integration gate and standalone replay comparator. The pinned
`accept-memory-candidate` report is
[`claim-rollout-default-acceptance-pinned.json`](ci-gate-v9-final-post-clippy/claim-rollout-default-acceptance-pinned.json);
the per-case aggregate is
[`claim-rollout-default-acceptance.json`](ci-gate-v9-final-post-clippy/claim-rollout-default-acceptance.json).

| Primary public-MCP metric | Shadow control | Product default (`evidence`) |
|---|---:|---:|
| Cases with evidence success | 8/8 | 8/8 |
| Cases with decision success | 1/8 | 5/8 |

Acceptance passed: four decision-family wins, no losses, no evidence
regressions, no candidate safety findings, and no hard failures. This confirms
the existing default on the registered finite cohort; it is a repeated
regression check on the same v9 families, not a fresh population sample or an
agent-usability/productivity result. The run needed no Codex executable,
account, or API key.

Reproducibility identities: product binary
`4ec78618868d35e38f6832716c26973420bfb7e3976ccba141dd05738b0c141c`,
evaluator binary
`654ae3d9c186028ab7a3a63f91233a366fd315d3d7e8ff40c25a0efcab79ed43`, corpus
`40fdef00b8201de03df680d2ad9185038ead358372d5b5b3fedac66e55044ee3`,
acceptance-plan file
`32655e8fb73a9010aac1211cc2e57569ca276a20824f21900f5fb27edf830c53`, and
preregistration file
`33bcb9de49131264b77c80082a6d2825f9ebc1fa43b05ce81ccf4a8230af077a`.

Final local verification on this source state passed `cargo test -p memory_mcp
--locked` (2,755 passed, 45 ignored), the full `eval-harness` suite (648
passed, 22 ignored), strict workspace clippy, and `cargo fmt --all --check`.
The final-build gate artifacts are under
[`ci-gate-v9-final-post-clippy/`](ci-gate-v9-final-post-clippy/).
