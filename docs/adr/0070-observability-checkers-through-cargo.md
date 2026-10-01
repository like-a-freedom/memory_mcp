# ADR-0070: The observability checkers are Python, reached through cargo

- Status: accepted
- Date: 2026-10-01
- Amends: ADR-0064
- Related: ADR-0065, ADR-0069

## Context

ADR-0064 removed `scripts/ci` and recorded that "every check CI runs is a
`cargo` command". The reasoning was sound and two of its claims still hold:
the scripts that came out were mostly dead — twelve files, 3516 lines, four of
them invoked — and the two that ran failed for reasons unrelated to the tree
they inspected.

The 2026-09-30 architecture audit then found a different kind of dead weight.
`observability/` holds 31 recording rules, 15 alerts, 4 dashboards and 4
Python checkers, and nothing reached any of them. The failure mode is specific
and it is invisible: a recording rule that reads a metric the crate no longer
exports evaluates to empty and never fires, so a safety net is absent while
looking present; and a panel naming a series that no longer exists renders
empty, which Grafana cannot distinguish from a subsystem that is switched off.
The reader sees "no failures" where the truth is "this was never measured".

The checkers were written on 2026-09-30, four days after ADR-0064. They were
never in its scope, and it does not mention them.

## Decision

The checkers stay Python, and the entry point is a cargo subcommand.

`cargo run -p xtask -- check-observability` is a cargo invocation, so a
workflow that already runs cargo runs this too. That is the whole of the
claim, and it is worth being precise about what it does and does not buy: the
command is cargo, and the command runs Python. ADR-0064's prohibition is on a
*separate* toolchain in the pipeline — a second one to install, version, and
keep in step — and `python3` plus one declared package is not a second
toolchain. It is a dependency, and it is declared in `pyproject.toml` where
the rest of the Python the repository uses is declared.

The order is fixed and is not alphabetical. `build_dashboards.py` regenerates
the committed JSON and the other three read it, so a checker run against a
stale dashboard would prove nothing about the dashboard that ships. The first
failure stops the run, which a test pins by making the second script fail and
asserting the third is never reached.

## Consequences

- ADR-0064's table gains a row: the observability checkers, reached through
  `xtask`, not run directly by a workflow. The ADR's "no Python in the
  pipeline" sentence needs qualifying, and this record is the qualification.
- A runner without `python3` cannot run this one step. Every GitHub-hosted
  runner has it, and a runner that does not is a runner the rest of the
  repository's Python tooling could not use either.
- The checkers are the only Python in `ci.yml`. That is worth keeping true,
  and it is why `pyyaml` is declared rather than left to resolve by accident
  through the onnxruntime chain — an undeclared dependency that happens to
  work is a dependency that stops working on a dependency bump nobody was
  watching.

## Alternatives considered

1. **Reimplement the checkers in Rust, using a `serde_yaml` dependency.**
   Rejected. It adds a dependency to the workspace to reimplement four
   checkers that already work, and ADR-0064's objection was never to the
   language — it was to a second toolchain and to code that nothing ran.

2. **Run the scripts directly from the workflow, with no `xtask` subcommand.**
   Rejected. Then the workflow contains a Python invocation, and ADR-0064's
   sentence is false rather than qualified. The subcommand is what makes the
   cargo claim honest.

3. **Leave the checkers unwired and record the gap.** Rejected. Thirty-one
   rules and fifteen alerts that nothing checks are a monitoring setup that
   looks like one. ADR-0064's own argument — that tooling nothing runs is
   worse than no tooling, because its size reads as coverage — applies here
   with more force, since this tooling is larger than what it removed.
