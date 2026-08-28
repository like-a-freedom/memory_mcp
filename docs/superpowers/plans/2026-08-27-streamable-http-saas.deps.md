# Cargo.toml change proposal — Streamable HTTP SaaS

**Status: approved and applied 2026-08-28; see commits `2e430f3b`, `4330a652`, `96bd9e2c` on `streamable-http-mcp`.**

This proposal required explicit user approval before any change was applied;
approval was given during the Phase 2 implementation session. The verbatim
block below is the proposal as originally submitted; the section after it
documents the three intentional deviations that were applied.

## Applied deviations from the verbatim block below

The implementation in `crates/memory-mcp/Cargo.toml` and `Cargo.toml` differs
from this proposal in three places, applied during code review:

1. **`chacha20poly1305` deferred.** The proposal placed `chacha20poly1305` in
   `control-plane` for "authenticated encryption of short-lived OIDC flow
   material". Review found that no Phase 1+2 code uses authenticated
   encryption — the only crypto consumers are HMAC (API-key pepper) and
   keyed blind index (OIDC subject). Pulling `aead` + `chacha20` +
   `poly1305` + `cipher` for an HMAC-only use case is a wrong-tool dep
   (AGENTS.md forbids speculative generality). Dropped; will be re-added
   in the Phase that introduces the first AEAD use site.

2. **`tower` and `tower-http` not declared.** The proposal listed
   `tower`/`tower-http` as required by `StreamableHttpService`. The
   verified rmcp 3.1.2 source shows the service impls
   `tower_service::Service<Request<RequestBody>>` (fully qualified) and
   exposes `Clone`; no `tower`/`tower-http` symbol is needed at a use
   site. `tower-service` is kept because the call site must name the
   `Service` trait to invoke `<StreamableHttpService as Service>::call`.

3. **`control-plane-ui` and `test-fixtures` features re-added.** An
   earlier review pass dropped these as "no use sites". The plan's
   Global Constraints requires them as part of the feature surface
   (they are documented in Phase 10 / Task 10.9 for the UI crate and
   Phase 4+ for fixtures). Restored.

4. **`rand` and `uuid` restored after being dropped in the first review
   pass.** The proposal lists `rand` and `uuid` for `streamable-http`
   (request IDs, nonce material). Round 1 dropped them as speculative;
   round 2 restored them because `streamable-http` is the home for the
   API-key auth and request-correlation code in Phase 4+ Tasks 4.3 and
   5.6, which require both. Kept.

5. **`src/bin/memory_mcp_http.rs` placeholder stub.** The proposal's
   `[[bin]] required-features = ["streamable-http"]` block requires the
   binary's source file to exist when the feature is enabled. The Phase
   3 Task 3.10 composition root has not been written yet, so the file
   is a fail-loud placeholder (`eprintln!` + `std::process::exit(1)`)
   that does not run a server. This is a Phase 2 file change made
   unavoidable by the proposal's own `[[bin]]` declaration.

## Workspace `Cargo.toml`

Add (or update) these workspace dependencies:

| Crate | Version | Default features | New features | Purpose |
|---|---|---|---|---|
| `axum` | `0.8` | none | `["http1", "tokio"]` | HTTP router |
| `tower` | `0.5` | none | `["util"]` | Service combinators |
| `tower-http` | `0.6` | none | `["trace", "request-id", "set-header", "limit"]` | HTTP middleware |
| `http` | `1` | none | `[]` | already transitive via rmcp; pin for explicit use |
| `http-body` | `1` | none | `[]` | already transitive via rmcp |
| `http-body-util` | `0.1` | none | `[]` | already transitive via rmcp |
| `bytes` | `1` | none | `[]` | already transitive via rmcp |
| `uuid` | `1` | none | `["v4"]` | session/request IDs (already transitive via rmcp) |
| `rand` | `0.10` | none | `[]` | already transitive via rmcp |
| `tower-service` | `0.3` | none | `[]` | `Service` trait for calling `StreamableHttpService` from axum; NOT re-exported by rmcp |
| `hmac` | `0.12` | none | `[]` | keyed verifiers for API keys / session cookies / CSRF |
| `subtle` | `2` | none | `[]` | constant-time comparison for secrets |
| `oauth2` | `5` | none | `[]` | optional control-plane Authorization Code + PKCE client |
| `jsonwebtoken` | `11` | none | `[]` | optional OIDC ID/access-token signature and claim validation |
| `chacha20poly1305` | `0.10` | none | `[]` | authenticated encryption for short-lived OIDC flow material — *see deviation 1 above; not in the applied `Cargo.toml`* |
| `base64` | `0.22` | none | `[]` | URL-safe PKCE verifier/challenge encoding |

## `crates/memory-mcp/Cargo.toml`

Add (or update) these features and optional deps:

```toml
[features]
streamable-http = [
    "rmcp/transport-streamable-http-server",
    "dep:axum",
    "dep:tower",
    "dep:tower-http",
    "dep:tower-service",
    "dep:http",
    "dep:http-body",
    "dep:http-body-util",
    "dep:bytes",

    "dep:uuid",
    "dep:rand",
    "dep:hmac",
    "dep:subtle",
]
control-plane = ["streamable-http", "dep:oauth2", "dep:jsonwebtoken", "dep:chacha20poly1305", "dep:base64"]
control-plane-ui = ["control-plane"]
# Test-only: exposes #[cfg(any(test, feature = "test-fixtures"))] fixtures to
# integration tests in tests/. Declared so cfg references pass unexpected_cfgs.
test-fixtures = []
```

The OAuth deps are intentionally added only when the control-plane feature is
enabled. They remain optional and never appear in the default build. Add
`chacha20poly1305 = "0.10"` to the optional control-plane dependencies: OIDC
state/nonce/PKCE verifier material is authenticated-encrypted at rest, while
API-key and cookie values remain keyed-hash-only.

## Workspace member

Add `crates/control-plane-ui` (gated on `control-plane-ui`) — added in Phase 10,
not now.

In `crates/memory-mcp/Cargo.toml`, declare the HTTP binary explicitly so the
normal default build never tries to compile it:

```toml
[[bin]]
name = "memory_mcp_http"
path = "src/bin/memory_mcp_http.rs"
required-features = ["streamable-http"]
```

The existing stdio binary configuration is left unchanged.

## Rollback

If the proposal is rejected, no files outside this proposal document are
 touched and Phase 2 is marked complete with the proposal retained for audit.
