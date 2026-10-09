# A1 scorer GREEN evidence

- Command: `rtk cargo test -p eval-harness --locked retrieval_quality`.
- Result: exit code 0; 5 scorer tests passed, 216 filtered out.
- Broader command: `rtk cargo test -p eval-harness --locked`.
- Result: exit code 0; 208 passed, 13 ignored across 12 suites.
- `rtk cargo fmt --all --check` and `rtk git diff --check` passed.
- Scope: component oracle only. No corpus-dependent retrieval evaluation or quality gate was run.
