# Agent-managed maintenance v5 — evidence response diagnostic

Status: no-go for changing `extract.include_fact_evidence` to default-on. The
bounded response implementation passes its deterministic contract tests, but
the preregistered repeated agent gate did not complete successfully.

## Frozen cohort and execution

The eight synthetic families and all task, prompt, schema, product, runner and
protocol identities are frozen in [the v5 preregistration](preregistration.md)
and [checksum manifest](freeze-manifest.sha256). The full pinned values are in
`plan.json`.

The registered sandboxed `run-01` produced 16 `model_failure` receipts with
zero model turns, tokens, memory-tool calls or writes. This is an execution
failure, not evidence about the treatment. The first CLI preflight with
relative binary paths also stopped before any agent invocation and is
separately recorded under `preflight-setup-failure-01/`.

With the user's explicit authorization, one direct-host diagnostic used the
same frozen plan, tasks and binaries with the existing local Codex login. It
completed all eight tasks per arm. It is retained as
[`run-host-diagnostic-01`](run-host-diagnostic-01/), but it is not a second
preregistered repeat and cannot satisfy the v5 promotion gate.

## Diagnostic result

| Measure | Compact incumbent | Bounded evidence candidate |
|---|---:|---:|
| Maintenance phases completed within 65,536 input-token cap | 0/8 | 7/8 |
| Maintenance phases over token cap | 8/8 | 1/8 |
| Exact extract and invalidation writes | 8/8 each | 8/8 each |
| Evidence request, response, and arrival before invalidation | 0/8 | 8/8 |
| Independent post-write probes | 8/8 | 8/8 |
| Final task success | 7/8 | 7/8 |
| Formation tool calls | 32 | 24 |
| Hard violations / unsupported or prohibited sources | 0 / 0 | 0 / 0 |
| Serialized extract response bytes, total / maximum | 19,692 / 2,467 | 24,020 / 3,023 |

All candidate evidence responses stayed below the 4,096-byte evidence budget;
the deterministic public-tool contract test separately checks the actual
serialized evidence delta. The candidate reduced one formation call per case
and completed more formation phases within the registered token cap. It did
not improve final task success in this single run (7/8 in each arm), and one
candidate phase still exceeded budget. The candidate's paired task-success
estimate is 0.0 with a family-clustered 95% interval of [-0.375, 0.375]. This
does not establish a quality gain or repeatable agent benefit.

The offline comparison replayed byte-for-byte; both report hashes were
`cfbaea436977f040335cda7333d5698be8d83a01c82f59ed546e2522407959e9`.

## Decision and remaining evidence

Keep the flag opt-in. V5 fails its preregistered requirements for two complete
repeats and all candidate phases within the context budget. The diagnostic
supports the narrow observation that this bounded evidence path can save one
verification call and reduce cap failures in this workflow; it does not show a
final-answer gain, broad agent benefit, or improved stored-memory/retrieval
quality. The separate v9 memory-quality gate tests claim reconciliation, not
this response flag.

Before revisiting a default change, run a new preregistered direct-host cohort
on disjoint families with two complete repeats, all-correct candidate tasks,
all candidate phases within budget, 8/8 exact evidence checks, and no recall or
independent-probe regressions. Retain and test explicit `false` compatibility,
the omitted-parameter behavior, the size cap, and oversized all-or-none
omission. Do not count the sandbox failure as a quality result or silently
replace it with a successful attempt.
