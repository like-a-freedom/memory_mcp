# ADR-0056: Two coarse build profiles for the memory_mcp package

## Status

Accepted

## Context

The `memory_mcp` package had grown a large, interlocking Cargo feature matrix
that made it hard to reason about how the binary is built and deployed. The
SaaS surface alone was decomposed into four incremental features —
`streamable-http` (HTTP data plane), `control-plane` (OIDC/local-admin auth +
account API), `control-plane-ui` (embedded Dioxus SPA), and `prometheus`
(/metrics) — plus an app-session axis (`mcp-apps`). In practice the Docker image
always enabled `streamable-http,control-plane,control-plane-ui` together, and
`control-plane` and `control-plane-ui` were pure decompositions of
`streamable-http` (each implied it and were never built standalone).

The operator and developer story was confusing: which flags are needed for the
local personal tool vs. the hosted SaaS? The goal was cognitive simplicity
(KISS/YAGNI): a small, predictable build surface.

## Decision

Collapse the runtime surface into **two coarse build profiles** plus a few
truly orthogonal opt-in axes.

| Feature | Meaning | Default |
| --- | --- | --- |
| `default = ["fs-watch"]` | **Local personal profile**: stdio MCP server + CLI, embedded SurrealDB, filesystem ingestion. | yes |
| `streamable-http` | **SaaS profile**: the single coarse switch for the whole Streamable HTTP server — data plane + control plane (OIDC/local-admin) + embedded web UI + Prometheus. | no |
| `mcp-apps` | App-session surface (inspector / diff / graph / lifecycle MCP resources and tools). **Orthogonal** to the profile axis: in-memory sessions in local/stdio, durable sessions in HTTP. | no |
| `metal`, `accelerate`, `mimalloc`, `eval-support`, `test-fixtures` | Unchanged orthogonal axes (platform / allocator / eval / test). | no |

`streamable-http` implies the internal names `control-plane`, `control-plane-ui`
and `prometheus`. These names remain declared in `Cargo.toml` **only** so the
existing `cfg(feature = "...")` sites in `http/` and `control/` keep compiling —
they are never written by users. `control-plane` keeps a back-link to
`streamable-http` so pre-existing test and operator invocations that name it
without `streamable-http` still compile the `http` module they need (a benign,
Cargo-permitted feature cycle).

### Why the fine-grained SaaS flags were collapsed

- The Docker image always built them together; there is exactly one HTTP
  product, so the decomposition bought compile-time flexibility no deploy
  target used.
- `control-plane` and `control-plane-ui` were pure decompositions of
  `streamable-http` (each implied it, never built standalone), so they were
  YAGNI as user-facing switches.

### Why `mcp-apps` was kept orthogonal rather than folded in

Unlike `control-plane`/`control-plane-ui`/`prometheus`, `mcp-apps` gates
code in the **stdio/local** path too: `MemoryMcp` carries an in-memory
`SessionManager` under `mcp-apps` and the HTTP durable backend under
`mcp-apps` + `streamable-http`. Folding it into `streamable-http` would have
silently removed the local app-session surface. It is a genuine independent
axis, usable in both profiles.

### Why the Compose deployments were reduced to two modes

Compose merged the shared `docker-compose.yml` with exactly one of three
overlays: `off` (data plane only, no browser surface), `local`, and `oidc`. The
`off` overlay is removed. The server state it described is unchanged and still
reachable — the base file already sets
`MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE=false` — so the overlay was not a
capability, only a pre-baked flavour with no operator. What it actually
contributed was the four values a control-plane-disabled deployment must still
supply: `MEMORY_MCP_HTTP_SIGNUP_MODE` plus the three 32-byte
`MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY` / `..._OIDC_STATE_KEY` / `..._OIDC_NONCE_KEY`
keys. `config/types.rs` demands all four whenever the deployment is not
local-admin mode and forbids zero-filling the keys, so an operator who wants a
data-plane-only deployment must now supply them. That is the accepted cost of the
narrower Compose surface, and the base file states it at the point of use.

`local` and `oidc` stay separate files rather than becoming one overlay with a
`MODE` variable because the two modes own **disjoint** secrets and each fails
startup when it receives the other's values. Naming the file keeps every
`${VAR:?}` demand unconditional, so a wrong-mode deployment fails while Compose
interpolates rather than after the container is running.

### Why the UI bundle is optional under Q6в (rather than Q6a)

Folding the UI into `streamable-http` means `build.rs` runs for every HTTP
build. To keep every HTTP build and test runnable without a heavy Dioxus WASM
build, `build.rs` now emits an **empty asset catalog** when
`MEMORY_MCP_CONTROL_PLANE_UI_DIST` is absent, so the binary compiles and simply
serves no UI. A **present-but-malformed** bundle still fails fast. The UI is
served only when a real bundle was embedded at build time **and**
`MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI=true` at runtime. Only the UI-serving
tests set the variable explicitly.

## Consequences

- Users build the local tool with `cargo build --release` (no flags) and the
  SaaS with `--features streamable-http`. `control-plane`, `control-plane-ui`
  and `prometheus` are no longer documented as independent user features.
- All existing test/operator commands that name `control-plane` (with or
  without `streamable-http`) keep compiling via the cycle.
- Every HTTP build and test now compiles without a Dioxus WASM bundle; UI
  embedding is opt-in at build time via the environment variable.
- Dockerfile, `ci.yml`, `Makefile`, `AGENTS.md`, `README.md` and the Compose
  files were updated to the two-profile model.
- Compose ships two browser overlays, `docker-compose.local.yml` and
  `docker-compose.oidc.yml`. A data-plane-only deployment is an
  operator-composed environment rather than a third pre-baked overlay.
- The trade-off of Q8b over Q8a: the internal feature names remain in
  `Cargo.toml` (marked internal) rather than being removed and having all
  ~80 `cfg` sites rewritten. This avoids a large, security-sensitive
  mechanical diff at the cost of a small residue of internal names.

## Alternatives considered

- **Q8a — remove the collapsed feature names and rewrite every `cfg` site** to
  `streamable-http`. Cleaner `Cargo.toml`, but ~80 edits across the
  auth/control-plane subsystem with real regression risk and no user-visible
  difference. Rejected in favor of Q8b.
- **Q6a — fold the UI into `streamable-http` and require the bundle on every
  HTTP build.** Made ~15 documented HTTP test commands fail without a Dioxus
  WASM build. Rejected in favor of Q6в (optional bundle).
- **Fold `mcp-apps` into `streamable-http`.** Removed the local app-session
  surface. Rejected because `mcp-apps` is genuinely usable in both profiles.
- **Keep `docker-compose.off.yml`.** It described a real server state, but no
  deployment used it, it needed its own validation step in CI, and an operator
  can compose the same state from four documented variables. Rejected as
  maintenance weight with no consumer.
- **Collapse `local` and `oidc` into one overlay selected by a `MODE`
  variable.** The mode-exclusive secret sets are what make a wrong-mode
  deployment fail before the container starts; a conditional overlay would have
  to give that up and could no longer demand each mode's secrets
  unconditionally. Rejected.