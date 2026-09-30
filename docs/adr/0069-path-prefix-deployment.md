# ADR-0069: Path-prefix deployment, and why the base path cannot be discovered at runtime

- Status: accepted
- Date: 2026-09-30
- Supersedes: nothing
- Related: ADR-0052, ADR-0056, ADR-0058
- Spec: [path-prefix deployment design](../superpowers/specs/2026-09-23-path-prefix-deployment.md)

## Context

`memory_mcp_http` is deployed behind a shared-host reverse proxy alongside
other MCP services, distinguished by path prefix — the server is reachable at
`https://host/memory`, not at the origin root. The code assumed it owned the
root: routes were root-absolute (`/mcp`, `/api/v1/*`, `/auth/oidc/*`,
`/health/*`, `/metrics`), the SPA fallback answered on any unclaimed path, the
embedded bundle hardcoded root-absolute asset URLs, and session cookies were
`__Host-` prefixed with `Path=/`.

Three constraints collided, and the shape of the solution follows from all
three rather than from any one of them.

**The base path had to come from configuration the operator already sets.**
The design spec's zero-config discipline forbids adding environment
variables, and an operator who has configured a public URL should not then
have to configure the mount point separately. The two can drift, and a drifted
pair produces a server that generates wrong links while every setting looks
correct.

**The UI bundle is built once and embedded in the binary.** Its
prefix-dependent URLs are literals inside compiled WASM and JS. A value
discovered at runtime cannot rewrite them: by the time the server knows its
base path, the bundle is already a byte array in the executable. This is the
constraint that rules out the obvious design.

**`__Host-` requires `Path=/`.** That is the cookie specification's contract,
not a choice. On a shared host, `Path=/` sends the session credential to every
sibling service on the origin, so keeping the prefix under a base path would
mean leaking it.

## Decision

1. **The public base URL is the single source of truth, and the base path is
   derived from its path component.** `MEMORY_MCP_HTTP_PUBLIC_BASE_URL` is
   required; `derive_base_path` (`http/config/parse.rs:218`) parses its path,
   and `HttpConfig::base_path` (`http/config/types.rs:117`) carries the
   result. An empty path yields `""`, which means origin root and preserves
   the previous behaviour exactly.

   The parser's grammar is strict — one optional trailing slash, and every
   segment non-empty, not `.` or `..`, and limited to unreserved URL
   characters. This is not defensive parsing for its own sake: `Router::nest`
   panics on a malformed mount path, so a bad public URL must fail at config
   load, where the error names the setting, rather than at route assembly,
   where it is a panic with no context.

2. **The bundle is built relocatable and stamped at startup.** The build bakes
   the placeholder `BASE_PATH_SENTINEL` = `/__memory_mcp_base__` wherever a
   prefix-dependent URL appears, and the server replaces it with the deployed
   base when it assembles assets (`ui/assets.rs:171`). The literal exists in
   five places that must change together — `ui/assets.rs`,
   `crates/ui/src/base.rs`, `crates/ui/index.html`, the `Dockerfile`'s
   `dx bundle` invocation, and `xtask/src/bundle.rs` — and `xtask` refuses a
   bundle whose `index.html` lacks it, so a mis-built bundle fails the build
   rather than serving root-absolute URLs from a prefixed mount.

   Stamping a non-empty base into a document with no sentinel is a
   `ConfigInvalid` error, not a silent pass-through: a legacy bundle that
   cannot be relocated must not be served as though it had been.

3. **Under a base path the session cookie is `__Secure-` and scoped to
   `Path={base}/`.** At the root it stays `__Host-` with `Path=/`.
   `control/session.rs:77-79` states the reason: the `__Host-` contract
   *requires* `Path=/`, and on a shared host that hands the credential to
   every sibling service. The same rule applies to the local-admin session and
   pre-auth cookies (`control/local_admin/csrf.rs:21-27`).

   Parsing accepts both names and prefers the configured one
   (`control/session.rs:115-117`), because a browser sends both during a
   migration and header order is not a contract.

4. **A request outside the prefix gets a 404 envelope, never the SPA.** The
   fallback 301-redirects `{base}/` to `{base}` and answers everything else
   with JSON. The host allowlist is checked in the gap
   (`http/router.rs:347-366`), so one service on a shared host cannot serve
   another service's pages.

## Consequences

- A host that has depended on a base path cannot be moved to the origin root
  without changing `MEMORY_MCP_HTTP_PUBLIC_BASE_URL`, and the cookie name will
  change with it. Existing sessions do not survive that move; the dual-name
  parsing exists to make the *deployment* move survivable, not the
  un-deployment.
- Renaming or re-valuing `BASE_PATH_SENTINEL` is a breaking change to the
  build: the five literals must move together, and a bundle built with the old
  value is rejected at startup. This is deliberate. A sentinel that could
  change value would make every cached bundle ambiguous.
- `derive_base_path` is the only place the mount base is computed. Adding a
  second source — a `BASE_PATH` variable, say — reintroduces exactly the drift
  the single source exists to prevent.
- Coverage is unit-level: the derivation, the router nesting, the stamping, the
  cookie naming and the build gate each have tests
  (`http/config/parse.rs`, `http/router.rs`, `ui/assets.rs`,
  `control/session.rs`, `control/local_admin/csrf.rs`, `xtask/src/bundle.rs`).
  There is no end-to-end test that boots the server behind a proxy under a
  prefix. That is a real gap, and it is the one thing here that a reader
  cannot verify from the code.

## Alternatives considered

1. **A separate `BASE_PATH` environment variable.** Rejected. Two settings
   describing one fact is a drift waiting to happen, and the zero-config
   discipline forbids it. The public URL is something the operator already
   configures, and its path is the mount point by construction.

2. **Serve the UI from a reverse proxy that rewrites asset URLs.** Rejected.
   It moves a correctness burden onto infrastructure the operator may not
   control, and the bundle's WASM reads its own root from a meta tag anyway, so
   the rewrite would have to be exact.

3. **Serve the bundle from a per-deployment file path** — write the stamped
   assets to disk at startup and point the browser at them. Rejected: it makes
   the server's behaviour depend on filesystem state it created, and it
   replaces an in-memory substitution with a write that can fail.

4. **Keep `__Host-` and accept the `Path=/` leak.** Rejected. The prefix
   exists precisely to share a host, and the cookie is the credential. Scoping
   the cookie and downgrading the prefix is the only option that keeps both
   properties; the security cost of `__Secure-` is a `Secure` attribute, which
   the deployment already requires.
