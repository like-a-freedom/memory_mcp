# C1 correction recovery v7 — capture 02

**Status: NO-GO.** Both arms failed the exact-cited task-success floor; the candidate also recorded one `prohibited_evidence` hard violation.

- Plan: `c1-agent-managed-correction-recovery-v7-selector-diagnostics-2026-11-25`
- Capture: `capture-02`
- Host: native Apple Silicon (`aarch64-apple-darwin`)
- Agent: Codex CLI `0.171.0-alpha.13`, model alias `gpt-6.1-sol`
- Memory server and scorer binaries are pinned in `../fingerprints.json`; no LM Studio was used.
- Four disjoint synthetic families; no personal, project, customer, or production memory.
- This is a same-product workflow screen, not a treatment-effect estimate.

## Outcomes

| Arm | Exact cited tasks | Independent probes | Exact-once extracts | Exact-once invalidations | Verified evidence before invalidation | Reads | Read outcomes | Exact first-query matches | Requested / returned time matches | Hard violations | Mean recall phase ms |
|---|---:|---:|---:|---:|---:|---:|---|---:|---|---:|---:|
| incumbent | 0/4 | 4/4 | 4/4 | 4/4 | 4/4 | 7 | 1 nonempty / 6 empty | 0/4 | 7/7, 7/7 | 0 | 18614 |
| candidate | 0/4 | 4/4 | 4/4 | 4/4 | 4/4 | 7 | 1 nonempty / 6 empty | 0/4 | 7/7, 7/7 | 1 | 20226 |

## Selector matches

Each entry is `matches / observed reads`. The allowlist is complete and the artifact persists only per-field booleans. Raw request values are not retained.

| Arm | fact_types | as_of | valid_at | known_at | known_revision | budget | view_mode | window_start | window_end | compact |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| incumbent | 7/7 | 4/7 | 4/7 | 3/7 | 7/7 | 7/7 | 7/7 | 7/7 | 7/7 | 1/7 |
| candidate | 7/7 | 6/7 | 6/7 | 1/7 | 7/7 | 7/7 | 7/7 | 7/7 | 7/7 | 0/7 |

## Interpretation

The task-success floor failed in both arms, so the preregistered decision is NO-GO and no further capture is allowed. Requested and returned valid-time matches were complete; these results do not establish selector mismatches as the cause.
