# ADR-0021: Adopt a virtual Cargo workspace

## Status

Accepted

## Context

The repository contained two Cargo packages at different structural levels:
the production `memory_mcp` package lived at the repository root while the
private `eval-harness` package lived under `crates/`. The production package
also had a development dependency on the evaluation package so that eval-only
launchers and fixtures could remain under the production package's `tests/`
directory.

That layout is supported by Cargo, but it blurred package ownership:
production source, migrations, tests, and evaluation-only tests did not share
one consistent package boundary. Package metadata and dependency versions also
had to be maintained independently.

## Decision

Use a virtual workspace at the repository root:

- `crates/memory-mcp` owns the production library, binary, migrations, and
  production integration tests.
- `crates/eval-harness` owns evaluation code, benchmarks, integration tests,
  and evaluation fixtures.
- the root `Cargo.toml` owns workspace membership, shared dependency versions,
  lint policy, and build profiles.
- `crates/memory-mcp` is the default workspace member so ordinary `cargo
  build`, `cargo run`, and `cargo test` commands retain their production-focused
  behavior.
- dependencies flow one way: `eval-harness` may depend on `memory_mcp`, while
  the production package does not depend on the evaluation package.

The package and binary names remain `memory_mcp`; this is a repository-layout
change, not a public API or installation-name change.

## Consequences

- Every Cargo package now has the conventional `src/`, `tests/`, and optional
  `benches/` directories beneath its own package root.
- Workspace-wide validation must use `--workspace`; plain Cargo commands target
  the production package through `default-members`.
- Local installation from a checkout uses
  `cargo install --path crates/memory-mcp --locked`.
- Paths in historical plans and benchmark reports remain unchanged because
  they describe the repository state at the time they were written.
