# ADR-0067: The composition root starts the service; the container constructs and holds it

- Status: accepted
- Date: 2026-10-01
- Related: ADR-0058, ADR-0052, ADR-0066

## Context

`MemoryService::new_from_env_with_mode_and_progress` sat in
`src/service/core/builder.rs` and was 280 lines of startup policy: read the
configuration, connect, probe the server version, apply migrations, resolve
the embedding activation decision, construct the embedding provider, construct
the NER extractor, write the bootstrap-ready state, check connectivity,
schedule the claim backfill, spawn the lifecycle workers, and spawn the
embedding-recovery worker.

The container's own job is narrower than that. `MemoryService::new`,
`new_with_embedding_provider` and the `with_*` methods construct the type and
hold its collaborators — that is what a container does. Everything in the
startup function is *policy about when and in what order to do what*, and it
lived in the same file as construction because the file that builds the thing
was already the file everyone went to when they needed a running service.

Two costs followed.

**There was no seam for starting a service.** To test the startup sequence,
or to start a service from somewhere other than the CLI, a caller had to
reach for a `pub(crate)` method on the container. The local `serve` path in
`runner.rs` and the CLI's `build_memory_service` both reach into
`service/core/` for it, which is the dependency direction ADR-0058 gives to
bounded contexts only.

**`bootstrap/` was the composition root for one profile.** It was
`cfg(control-plane)` and held only the HTTP integration adapters, because
that is where the HTTP profile's wiring had already been put. The stdio
profile had no composition root; it had a method on a type it also defines.

## Decision

`bootstrap/` is the composition root for both profiles. The environment
reading and startup sequence move to `bootstrap/stdio.rs` as a free function:

```rust
pub async fn build_memory_service_from_env(
    mode: EmbeddingActivationMode,
    progress: Arc<dyn ModelProgressSink>,
) -> Result<MemoryService, MemoryError>;
```

`MemoryService::new`, `new_with_embedding_provider` and the `with_*` builder
methods stay in `service/core/builder.rs`. The container constructs and
holds; the composition root starts.

## Consequences

`bootstrap.rs` is unconditional — `lib.rs` already declared it so; only its
`integration` submodule was behind `control-plane`, which is correct, because
those adapters are HTTP's. Every profile compiles the composition root, so a
second caller can start a service without the CLI.

`service/core/builder.rs` drops from 810 lines to roughly 500. The remainder
is construction and the `with_*` surface, which is the container's.

The startup sequence becomes testable as a unit: `bootstrap/stdio.rs` takes
its two inputs as arguments, so a test can exercise the zero-configuration
path without the environment.

## Alternatives considered

(a) **Keep a `service/startup.rs`.** Rejected. It leaves a second place that
knows how to build a service, which is the confusion this change exists to
remove — only the directory would have moved.

(b) **Move it into `runner.rs`.** Rejected. AGENTS.md requires `main.rs` and
`runner.rs` to stay thin and hold CLI parsing and mode dispatch; 280 lines of
startup policy is neither.

(c) **Leave it where it is and make the method `pub`.** Rejected. That makes
the dependency reachable rather than correct, and leaves the container's
construction surface and its startup surface in one file.
