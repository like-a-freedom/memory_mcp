# C1 correction recovery v7 — capture 01

**Status: PASS.** Both arms passed every frozen gate.

- Plan: `c1-agent-managed-correction-recovery-v7-selector-diagnostics-2026-11-25`
- Capture: `capture-01`
- Host: native Apple Silicon (`aarch64-apple-darwin`)
- Agent: Codex CLI `0.171.0-alpha.13`, model alias `gpt-6.1-sol`
- Memory server and scorer binaries are pinned in `../fingerprints.json`; no LM Studio was used.
- Four disjoint synthetic families; no personal, project, customer, or production memory.
- This is a same-product workflow screen, not a treatment-effect estimate.

## Outcomes

| Arm | Exact cited tasks | Independent probes | Exact-once extracts | Exact-once invalidations | Verified evidence before invalidation | Reads | Read outcomes | Exact first-query matches | Requested / returned time matches | Hard violations | Mean recall phase ms |
|---|---:|---:|---:|---:|---:|---:|---|---:|---|---:|---:|
| incumbent | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4 | 4 nonempty / 0 empty | 0/4 | 4/4, 4/4 | 0 | 14186 |
| candidate | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4 | 4 nonempty / 0 empty | 0/4 | 4/4, 4/4 | 0 | 15341 |

## Selector matches

Each entry is `matches / observed reads`. The allowlist is complete and the artifact persists only per-field booleans. Raw request values are not retained.

| Arm | fact_types | as_of | valid_at | known_at | known_revision | budget | view_mode | window_start | window_end | compact |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| incumbent | 4/4 | 4/4 | 4/4 | 0/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 1/4 |
| candidate | 4/4 | 3/4 | 3/4 | 1/4 | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 1/4 |

## Interpretation

All frozen gates passed in both arms. This permitted capture 02 under the preregistered rule; it does not establish a memory improvement or stable qualification by itself.
