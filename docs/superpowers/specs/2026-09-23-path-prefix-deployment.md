# Path-Prefix Deployment — design spec

**Date:** 2026-09-23
**Status:** Approved for planning
**Amended:** 2026-09-23 — Decision 3 is replaced: the UI bundle no longer bakes
the mount prefix at build time. It is built relocatable (sentinel base) and the
server stamps the deployed base into the served shell at startup. The non-goal
"Runtime-variable base for the UI bundle" is removed accordingly.
Implementation: `docs/superpowers/plans/2026-09-23-relocatable-ui-bundle.md`.
**Plan:** `docs/superpowers/plans/2026-09-23-path-prefix-deployment.md`

## Context

`memory_mcp_http` deploys behind the fosrl Pangolin reverse proxy on a shared
host `mcp.like-a-freedom.ru`, alongside other MCP services routed by path
prefixes (e.g. `/intervals`). Today the server assumes it owns the origin
root: all routes are root-absolute (`/mcp`, `/api/v1/*`, `/auth/oidc/*`,
`/health/*`, `/metrics`), the SPA fallback answers on any unclaimed path, the
bundle hardcodes root-absolute asset URLs, and session cookies are `__Host-`
prefixed with `Path=/` (the `__Host-` contract *requires* `Path=/`).

Operator decisions already made:

- `/metrics` exposure is **not** a code concern — the reverse proxy /
  identity-aware proxy owns it. No change.
- The origin root must be **freed**: `memory_mcp` lives entirely under its own
  prefix (deployment target: `/memory`).
- Zero-config discipline: **no new environment variables**. Prefer native
  framework mechanisms over bespoke ones. KISS/YAGNI.

## Problem

With root ownership removed, three root couplings break:

1. **Routing:** axum routes are registered at `/mcp` etc.; requests arrive as
   `/memory/mcp`. Path introspection (`reject_non_post_mcp` checks
   `path == "/mcp"`), the reserved-surface fallback (`/api/`, `/auth/`), and
   `serve_asset(path)` all read the raw request path.
2. **UI bundle:** `dx bundle` bakes root-absolute asset URLs; the Dioxus
   router matches `location.pathname` against root-absolute routes; UI fetch
   constants are root-absolute.
3. **Cookies:** `__Host-memory_mcp_session`, `__Host-memory_mcp_admin`,
   `__Host-memory_mcp_admin_preauth` use `Path=/` and are therefore sent to
   every service on the shared host.

## Decision

**1. The mount base path is derived from `MEMORY_MCP_HTTP_PUBLIC_BASE_URL`.**
Its path component *is* the prefix (`https://mcp.example/memory` → `/memory`;
no path → `""` = origin root, today's behavior). No new configuration exists,
and the derived base is automatically consistent with the URLs the server
publishes (activation/reset links, OAuth `resource`). The derivation enforces
a strict grammar (`/`-prefixed, no trailing `/`, no empty/`.`/`..` segments,
unreserved characters only) and fails config load with `ConfigInvalid` —
before the router is built, because `Router::nest` panics on malformed paths.

**2. The backend mounts itself with axum's native `Router::nest`.** `nest`
registers every route *and the fallback* under the prefix and strips the
prefix from the request URI (`StripPrefix`) before inner middleware and
handlers inspect it — verified against axum 0.8.8: a nested fallback receives
`/x/y` for a request to `/memory/x/y`. All internal path logic therefore
stays canonical and unchanged. Requests outside the prefix match nothing and
answer `404`. The deployment-boundary layers (`request_log`,
`inject_sse_headers`, `host_origin`) wrap the outermost router, preserving
the invariant that the boundary covers everything, including the gap fallback
below.

One matchit edge is canonicalized explicitly: an empty catch-all tail does
not match, so `{base}/` (e.g. `/memory/`) would 404 while `{base}` works. The
outer fallback answers `{base}/` with a `308` redirect to `{base}`, and
everything else with the standard `not_found` JSON envelope.

**3. The UI bundle is relocatable; the server stamps the base at startup.**
`dx bundle --base-path /__memory_mcp_base__` bakes a *sentinel*, never a
deployment prefix: every prefix-dependent URL in the bundle's `index.html` and
its `DIOXUS_ASSET_ROOT` meta element carry `/__memory_mcp_base__`. At router
assembly `memory_mcp_http` replaces the sentinel with the derived base
(`""` or `/memory`, …) in the embedded `index.html` and serves those stamped
bytes for the SPA shell; asset requests already work at any base because
`Router::nest` strips the prefix before `serve_asset` sees the path. The WASM
resolves its base at runtime from the `DIOXUS_ASSET_ROOT` meta (stamped by the
server) with a sentinel-filtered fallback, and `dioxus_web::WebHistory` receives
the same base explicitly. A bundle missing the sentinel is rejected at startup
(`MemoryError::ConfigInvalid`) unless the deployment is at the origin root. The
result: **one image serves any prefix**; changing the prefix is a runtime
configuration change and needs no rebuild. The only build/deploy contract left
is "the bundle was built with the sentinel", which startup enforces.

**4. Cookies switch to `__Secure-` + `Path={base}/` when a base is set**; with
an empty base the existing `__Host-` + `Path=/` contract is unchanged. On a
shared host a `Path=/` cookie is sent to every sibling service and the raw
value is the session credential; `__Host-` is incompatible with a scoped
`Path`, so the prefix downgrades to `__Secure-`. Parsers accept either name
(configured name preferred) so a config change never wedges parsing.
*Separable: see plan Task 6.*

**Redirect/absolute URLs need no code change:** `{public_base_url}/admin/activate`
concatenation, the OIDC `redirect_uri` (absolute config:
`https://mcp.example/memory/auth/oidc/callback`) and OAuth `resource` all
follow from the public URL automatically.

## Configuration surface

| Variable | Change | Notes |
|---|---|---|
| `MEMORY_MCP_HTTP_PUBLIC_BASE_URL` | semantics sharpened | Its **path** is now the mount base (`https://mcp.example/memory`). No path = origin root = unchanged behavior. |
| `MEMORY_MCP_HTTP_OIDC_REDIRECT_URI` | none | Register `https://mcp.example/memory/auth/oidc/callback` at the IdP. |

Build-time: `dx bundle --base-path /__memory_mcp_base__` — the relocatable-bundle
sentinel, never a deployment prefix (Docker: hardcoded in the `Dockerfile`, no
build arg). **Zero new env vars.**

## Pangolin contract

| Path (prefix match) | Rewrite | Target |
|---|---|---|
| `/memory` | **none** (forward as-is) | `localhost:8080` |
| `/intervals`, other services | per-service | per-service |

Anything unmatched falls through to whatever holds `/`; `memory_mcp` no longer
participates there and answers `404` on root paths itself.

## Non-goals

- `/metrics` exposure (operator: proxy owns it).
- More than one mount prefix per process.

## Compatibility

- `MEMORY_MCP_HTTP_PUBLIC_BASE_URL` without a path: zero behavior change
  (routes, cookies, assets, fallback identical; existing suite passes
  unmodified).
- With a path: browser sessions are invalidated by the cookie name/Path
  change (one-time re-login, same class as the documented
  `browser_policy_epoch` break). `{base}/` answers `308 → {base}`. The UI
  bundle is prefix-free from this amendment on: the same image serves the
  origin root and any prefix, stamped at startup. Prefix changes no longer
  invalidate browser caches of hashed assets for a rebuilt bundle, because no
  rebuild happens.
