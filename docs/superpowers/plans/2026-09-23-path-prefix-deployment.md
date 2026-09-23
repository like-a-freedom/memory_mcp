# Path-Prefix Deployment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Serve `memory_mcp_http` entirely under a path prefix (e.g. `https://mcp.like-a-freedom.ru/memory/*`) so the shared host root stays free for other services.

**Architecture:** The base path is *derived* from the path of the already-required `MEMORY_MCP_HTTP_PUBLIC_BASE_URL` — zero new configuration. The router is mounted with axum's native `Router::nest`, which registers routes and the fallback under the prefix and strips it before any path inspection; requests outside the prefix answer `404`. The UI bundle bakes the same prefix via `dx bundle --base-path` (native Dioxus base-path support) and UI fetches prepend it through `dioxus_cli_config::base_path()`. A public URL without a path keeps today's behavior byte-identical.

**Tech Stack:** Rust 1.97.1, axum 0.8.8 (`Router::nest`, `Redirect`), Dioxus 0.7.10 + dioxus-router (`WebHistory` prefix), `dioxus-cli-config` 0.7.10, gloo-net.

**Spec:** `docs/superpowers/specs/2026-09-23-path-prefix-deployment.md`

## Global Constraints

- **Zero new environment variables** (zero-config discipline). The one knob is the path of the existing `MEMORY_MCP_HTTP_PUBLIC_BASE_URL`. The one new build arg (`MEMORY_MCP_UI_BASE_PATH`) defaults to empty.
- Public-URL base grammar: empty (origin root) or `/`-prefixed, no trailing `/`, no empty/`.`/`..` segments, unreserved characters only (`A-Za-z0-9-._~`). Violations fail config load with `MemoryError::ConfigInvalid` naming `MEMORY_MCP_HTTP_PUBLIC_BASE_URL`.
- Public URL without a path ⇒ **zero behavior change**; the existing test suite must stay green unmodified.
- No `unwrap()`/`expect()` in production code paths (tests may use them).
- Native framework mechanisms over bespoke ones: `Router::nest` for mounting, `dx bundle --base-path` + `dioxus_cli_config::base_path()` for the UI. No custom strip middleware, no runtime asset rewriting.
- Dependency changes need operator approval before merge (AGENTS.md). Task 3 adds exactly one pinned dependency already resolved in `Cargo.lock` through `dioxus-web`: `dioxus-cli-config = { version = "=0.7.10", features = ["web"] }`.
- Gates before every commit: `cargo fmt --all`, then `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings`, `cargo test -p memory_mcp`. Tasks adding `#[cfg(feature = ...)]`-gated tests additionally run `cargo test -p memory_mcp --features streamable-http`. UI tasks additionally run `cargo test -p control-plane-ui` and `cargo check -p control-plane-ui --target wasm32-unknown-unknown`.
- Build/deploy contract: the `dx bundle --base-path` value must equal the path of `MEMORY_MCP_HTTP_PUBLIC_BASE_URL` (documented, not runtime-validated).
- `/metrics` exposure is out of scope (operator: reverse proxy / identity-aware proxy owns it).

---

### Task 1: Derive the base path from `MEMORY_MCP_HTTP_PUBLIC_BASE_URL`

**Files:**
- Modify: `crates/memory-mcp/src/http/config/parse.rs` (add `derive_base_path`)
- Modify: `crates/memory-mcp/src/http/config/types.rs` (field ~line 159, `Debug` impl ~line 220, `from_env` ~lines 417/647, `default_for_test` ~line 719)

**Interfaces:**
- Produces: `HttpConfig.base_path: String` (`""` = origin root), derived at config load. Consumed by Tasks 2 and 6.
- Produces: `pub(super) fn derive_base_path(public_base_url: &str) -> Result<String, MemoryError>`.

- [ ] **Step 1: Write the failing tests**

Add to `parse.rs` (extend its existing `#[cfg(test)]` module, or create one):

```rust
    #[test]
    fn derives_the_base_path_from_the_public_url() {
        for (url, expected) in [
            ("http://localhost", ""),
            ("http://localhost:8080", ""),
            ("https://mcp.example/", ""),
            ("https://mcp.example/memory", "/memory"),
            ("https://mcp.example/memory/", "/memory"),
            ("https://mcp.example/tools/memory", "/tools/memory"),
        ] {
            assert_eq!(derive_base_path(url).expect(url), expected, "{url}");
        }
    }

    #[test]
    fn rejects_public_urls_that_cannot_name_a_mount_base() {
        for url in [
            "mcp.example/memory", // no scheme
            "https://", // no host
            "https:///memory", // no host
            "https://mcp.example//memory", // empty segment
            "https://mcp.example/mem/../x", // dot-dot segment
            "https://mcp.example/.", // dot segment
            "https://mcp.example/mem ory", // space
            "https://mcp.example/мемори", // non-ASCII
            "https://mcp.example/memory?q=1", // query
        ] {
            assert!(derive_base_path(url).is_err(), "{url}");
        }
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p memory_mcp derive_base_path -- --nocapture 2>&1 | tail -5` (or the module filter that matches)
Expected: compile FAIL — `cannot find function \`derive_base_path\``.

- [ ] **Step 3: Implement**

In `parse.rs`:

```rust
/// Derive the mount base path from `MEMORY_MCP_HTTP_PUBLIC_BASE_URL`.
///
/// The public URL is the single source of truth for where this server is
/// mounted (12-factor config): its path *is* the base path, so no second
/// variable can drift from it. No path means the origin root — the
/// pre-base-path behavior. The grammar is strict on purpose: `Router::nest`
/// panics on malformed mount paths, so a bad public URL must fail config
/// load instead.
pub(super) fn derive_base_path(public_base_url: &str) -> Result<String, MemoryError> {
    let invalid = |detail: &str| {
        MemoryError::ConfigInvalid(format!(
            "MEMORY_MCP_HTTP_PUBLIC_BASE_URL must be scheme://host[:port][/base] where the \
             base is '/'-prefixed unreserved-character segments \
             (e.g. https://mcp.example/memory): {detail}"
        ))
    };
    let Some((_, rest)) = public_base_url.split_once("://") else {
        return Err(invalid("no scheme"));
    };
    if rest.is_empty() || rest.starts_with('/') {
        return Err(invalid("no host"));
    }
    let path = rest
        .split_once('/')
        .map(|(_, path)| format!("/{path}"))
        .unwrap_or_default();
    let base = path.trim_end_matches('/').to_owned();
    if base.is_empty() {
        return Ok(String::new());
    }
    for segment in base[1..].split('/') {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || !segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~'))
        {
            return Err(invalid("bad path segment"));
        }
    }
    Ok(base)
}
```

In `types.rs`: add the derived field next to `public_base_url` in `HttpConfig`:

```rust
    pub bind: SocketAddr,
    pub public_base_url: String,
    /// Mount base path derived from the path of `public_base_url`
    /// (`""` = origin root). See `derive_base_path`.
    pub base_path: String,
    pub trusted_proxy_cidrs: Vec<TrustedCidr>,
```

In `from_env`, right after `public_base_url` is read:

```rust
        let public_base_url = require_env("MEMORY_MCP_HTTP_PUBLIC_BASE_URL")?;
        let base_path = derive_base_path(&public_base_url)?;
```

plus `base_path,` in the `HttpConfig { … }` construction and `.field("base_path", &self.base_path)` in the hand-written `Debug` impl. In `default_for_test()` add `base_path: String::new(),`. Import `derive_base_path` alongside the existing `optional_env`/`require_env` imports in `types.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p memory_mcp`
Expected: PASS — new tests green, entire existing suite unchanged.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
git add crates/memory-mcp/src/http/config/parse.rs crates/memory-mcp/src/http/config/types.rs
git commit -m "feat(config): derive the mount base path from MEMORY_MCP_HTTP_PUBLIC_BASE_URL"
```

---

### Task 2: Mount the router under the base path with `Router::nest`

**Files:**
- Modify: `crates/memory-mcp/src/http/router.rs` (final return block, ~lines 325-340; tests module)

**Interfaces:**
- Consumes: `HttpConfig.base_path` (Task 1).
- Produces: `build_router` behavior only (no signature change): with an empty base the router is exactly today's; with a base the whole router (routes + fallback) answers under `{base}/*` and nothing at the root. `{base}/` canonicalizes to `{base}` via `308`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `router.rs` (reuse its `request`/`request_with_host` helpers):

```rust
    async fn prefixed_state(base: &str) -> std::sync::Arc<crate::http::HttpState> {
        let mut config = crate::http::config::HttpConfig::default_for_test();
        config.base_path = base.to_owned();
        config.enable_control_plane_ui = true;
        HttpStateTestBuilder::new()
            .await
            .with_config(config)
            .build()
            .await
            .expect("prefixed HTTP state")
    }

    async fn body_text(response: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// The point of the whole feature: `memory_mcp` must not answer at the
    /// origin root of a shared host. Without the base, `GET /mcp` is a 405
    /// and `GET /health/live` is a 200 — both must become the 404 envelope.
    #[tokio::test]
    async fn with_a_base_path_root_paths_answer_the_404_envelope() {
        let mut router = build_router(prefixed_state("/memory").await, None);
        for (method, uri) in [
            (Method::GET, "/health/live"),
            (Method::GET, "/mcp"),
            (Method::POST, "/mcp"),
        ] {
            let response = router.call(request(method, uri)).await.expect("dispatch");
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
            assert!(body_text(response).await.contains("\"not_found\""), "{uri}");
        }
    }

    #[tokio::test]
    async fn with_a_base_path_routes_reach_their_handlers() {
        let mut router = build_router(prefixed_state("/memory").await, None);

        let response = router
            .call(request(Method::GET, "/memory/health/live"))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::OK);

        // 405 proves `reject_non_post_mcp` saw the *stripped* path: it
        // guards on `path == "/mcp"`, and `/memory/mcp` carried in the URI
        // would fall through to the fallback instead.
        let response = router
            .call(request(Method::GET, "/memory/mcp"))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

        // POST passes the method guard and dies at MCP prevalidation with
        // the 400 JSON-RPC envelope — the whole route stack ran.
        let response = router
            .call(request(Method::POST, "/memory/mcp"))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_query_string_survives_the_mount() {
        let mut router = build_router(prefixed_state("/memory").await, None);
        let response = router
            .call(request(Method::GET, "/memory/health/live?probe=1"))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn the_trailing_slash_form_canonicalizes_to_the_bare_prefix() {
        let mut router = build_router(prefixed_state("/memory").await, None);
        let response = router
            .call(request(Method::GET, "/memory/"))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(
            response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok()),
            Some("/memory")
        );
    }

    /// `serve_asset`'s not-found is plain `not found` with security headers;
    /// the gap fallback's is the JSON envelope. The body proves the bare
    /// prefix routed to the *inner* SPA fallback (the bundle is absent in
    /// tests, so the SPA route itself 404s — through `serve_asset`).
    #[cfg(feature = "control-plane-ui")]
    #[tokio::test]
    async fn the_bare_prefix_reaches_the_inner_spa_fallback() {
        let mut router = build_router(prefixed_state("/memory").await, None);
        let response = router
            .call(request(Method::GET, "/memory"))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_text(response).await, "not found");
        assert!(response.headers().contains_key("content-security-policy"));
    }

    #[cfg(feature = "control-plane-ui")]
    #[tokio::test]
    async fn deep_spa_paths_reach_the_inner_spa_fallback() {
        let mut router = build_router(prefixed_state("/memory").await, None);
        let response = router
            .call(request(Method::GET, "/memory/admin/login"))
            .await
            .expect("dispatch");
        assert_eq!(body_text(response).await, "not found");
    }

    /// The deployment boundary must cover the gap fallback too — it is the
    /// one surface that does not sit inside the nested router.
    #[tokio::test]
    async fn the_host_allowlist_covers_the_gap_fallback() {
        let mut router = build_router(prefixed_state("/memory").await, None);
        let response = router
            .call(request_with_host(Method::GET, "/memory/", "evil.example"))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    /// An empty base must keep the exact pre-feature surface.
    #[tokio::test]
    async fn an_empty_base_path_keeps_root_serving() {
        let mut router = build_router(prefixed_state("").await, None);
        let response = router
            .call(request(Method::GET, "/health/live"))
            .await
            .expect("dispatch");
        assert_eq!(response.status(), StatusCode::OK);
        let response = router.call(request(Method::GET, "/mcp")).await.expect("dispatch");
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p memory_mcp router:: && cargo test -p memory_mcp --features streamable-http router::`
Expected: FAIL — root tests see `200`/`405` instead of `404`; prefixed tests see `404` instead of `200`/`405`/`400`.

- [ ] **Step 3: Implement the nested mount**

Replace the final return block of `build_router` (the three-layer chain) with — keeping the existing boundary comment intact above it:

```rust
    // The deployment boundary goes on last, and last is what makes it a
    // boundary … (keep the existing comment) …
    let base_path = state.config.base_path.clone();
    let app = router.with_state(state.clone());
    // Under a mount base the whole router is nested: axum registers every
    // route *and the fallback* under the prefix and strips it from the
    // request URI (`StripPrefix`) before anything below inspects
    // `uri().path()`, so `reject_non_post_mcp`, the reserved-surface
    // checks and `serve_asset` keep seeing canonical paths. Requests
    // outside the prefix match nothing. The boundary layers below wrap
    // this whole shape, gap fallback included.
    let app = if base_path.is_empty() {
        app
    } else {
        let base_for_fallback = base_path.clone();
        axum::Router::new()
            .nest(&base_path, app)
            .fallback(move |uri: axum::http::Uri| {
                let base = base_for_fallback.clone();
                async move {
                    // matchit does not match an empty catch-all tail, so
                    // `{base}/` lands here while `{base}` is the nested SPA
                    // root. Canonicalize the one URL humans and proxies
                    // produce routinely instead of 404-ing it; everything
                    // else outside the nested surface is a plain 404.
                    if uri.path() == format!("{base}/") {
                        return axum::response::IntoResponse::into_response(
                            axum::redirect::Redirect::permanent(&base),
                        );
                    }
                    axum::response::IntoResponse::into_response((
                        axum::http::StatusCode::NOT_FOUND,
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        "{\"error\":{\"code\":\"not_found\",\"message\":\"not found\"}}",
                    ))
                }
            })
    };
    app.layer(axum::middleware::from_fn_with_state(
        state,
        super::middleware::host_origin,
    ))
    .layer(axum::middleware::from_fn(
        super::middleware::inject_sse_headers,
    ))
    .layer(axum::middleware::from_fn(super::logging::request_log))
}
```

(The three `.layer(...)` calls stay in the exact order and shape of the block being replaced, so the empty-base composition is byte-identical.)

- [ ] **Step 4: Run the full suites to verify they pass**

Run: `cargo test -p memory_mcp && cargo test -p memory_mcp --features streamable-http`
Expected: PASS — new tests green **and** every pre-existing test unchanged (root parity).

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
git add crates/memory-mcp/src/http/router.rs
git commit -m "feat(http): mount the whole router under a path prefix with Router::nest"
```

---

### Task 3: UI — base-aware same-origin URLs

> **Depends on operator approval for the one-line dependency add** (Global Constraints). The crate is already in `Cargo.lock` through `dioxus-web`; this only names it.

**Files:**
- Modify: `crates/control-plane-ui/Cargo.toml`
- Create: `crates/control-plane-ui/src/base.rs`
- Modify: `crates/control-plane-ui/src/main.rs` (add `mod base;`)
- Modify: `crates/control-plane-ui/src/api.rs` (`endpoint` body, new `same_origin()`, 7 `ApiClient::new("/"…)` call sites in `state/account_session.rs`, `pages/keys.rs` ×3, `pages/delete.rs` ×2, `pages/status.rs`)
- Modify: `crates/control-plane-ui/src/admin_api.rs` (request-construction sites: ~1153, ~1219, ~1258, ~1269, ~1285, ~1298, ~1314, ~1329, ~1352, ~1369)
- Modify: `crates/control-plane-ui/src/pages/login.rs:51` (`href`)

**Interfaces:**
- Consumes: `dioxus_cli_config::base_path()` — the native Dioxus base-path source (the same value `dioxus_web::WebHistory` uses for router navigation), baked by `dx bundle --base-path` (Task 4).
- Produces: `crate::base::base_path() -> String`, `crate::base::url(path: &str) -> String`, `crate::base::join(base: &str, path: &str) -> String`, `ApiClient::same_origin() -> ApiClient`. `PATH_*` constants stay root-absolute forever.

- [ ] **Step 1: Write the failing tests**

Create `crates/control-plane-ui/src/base.rs`:

```rust
//! Base-path aware same-origin URL construction.
//!
//! The console is served under the path prefix baked by
//! `dx bundle --base-path` — the same native source `dioxus_web::WebHistory`
//! reads for router navigation. Every fetch and raw `<a href>` goes through
//! [`url`] so a prefixed deployment never issues a root-absolute URL.
//! Router `Link`s need nothing: `WebHistory` adds the prefix itself.

/// The base path this bundle was built for (e.g. `/memory`), empty at root.
pub fn base_path() -> String {
    dioxus_cli_config::base_path().unwrap_or_default()
}

/// Join the bundle base and a root-absolute `path` into one same-origin URL.
pub fn url(path: &str) -> String {
    join(&base_path(), path)
}

/// Pure join: exactly one slash between base and path.
///
/// A leading `//` is not an absolute path — the URL spec reads it as a
/// scheme-relative reference (`scheme://api/…`), so the browser would look
/// up a host named `api` and reject the fetch before a byte is sent. The
/// same rule is pinned from the `ApiClient` side by its own tests.
pub fn join(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    format!("{base}/{}", path.trim_start_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::join;

    #[test]
    fn empty_base_and_root_base_agree() {
        assert_eq!(join("", "/api/v1/account"), "/api/v1/account");
        assert_eq!(join("/", "/api/v1/account"), "/api/v1/account");
    }

    #[test]
    fn a_prefixed_base_prepends_exactly_one_slash() {
        assert_eq!(join("/memory", "/api/v1/account"), "/memory/api/v1/account");
        assert_eq!(join("/memory/", "api/v1/account"), "/memory/api/v1/account");
    }

    #[test]
    fn a_prefixed_join_is_never_scheme_relative() {
        assert!(!join("/memory/", "/api/v1/account").starts_with("//"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p control-plane-ui`
Expected: compile FAIL — `mod base` not declared.

- [ ] **Step 3: Implement**

1. `Cargo.toml` dependencies (exact-pin convention of the file; matches what `dioxus-web` already resolves):

```toml
dioxus-cli-config = { version = "=0.7.10", features = ["web"] }
```

2. `main.rs`: add `mod base;` next to `mod admin_api;`.

3. `api.rs`: replace the body of `ApiClient::endpoint` with `crate::base::join(&self.base, path)` (keep the function and its doc; the join contract stays pinned by its tests) and add:

```rust
    /// A client for this bundle's own origin, under its base path.
    pub fn same_origin() -> Self {
        Self::new(crate::base::base_path())
    }
```

Replace all `ApiClient::new("/".to_owned())` call sites with `ApiClient::same_origin()` (`state/account_session.rs:35`, `pages/keys.rs:31`, `pages/keys.rs:46`, `pages/keys.rs:74`, `pages/delete.rs:34`, `pages/delete.rs:59`, `pages/status.rs:24`).

4. `admin_api.rs`: wrap every request-construction path in `crate::base::url(...)` — `Request::get(path)` → `Request::get(&crate::base::url(path))`, `Request::post(PATH_CHALLENGE)` → `Request::post(&crate::base::url(PATH_CHALLENGE))`, and likewise for `PATH_LOGIN`, `PATH_REAUTH`, `PATH_LOGOUT`, `PATH_CLIENTS` (get/post), `keys_path(...)`, `state_path(...)`. The `PATH_*` constants and `client_path`/`keys_path`/`state_path` keep returning root-absolute strings.

5. `pages/login.rs:51`: `href: crate::base::url(PATH_OIDC_AUTHORIZE)`.

- [ ] **Step 4: Run tests and checks to verify they pass**

```bash
cargo test -p control-plane-ui
cargo check -p control-plane-ui --target wasm32-unknown-unknown
cargo tree -p control-plane-ui -i dioxus-cli-config
```
Expected: tests PASS (all pre-existing `api.rs` join tests unchanged — semantics preserved); wasm check clean; `cargo tree` shows a **single** `dioxus-cli-config v0.7.10` shared with `dioxus-web`.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
git add crates/control-plane-ui
git commit -m "feat(ui): build every fetch and link under the native Dioxus base path"
```

---

### Task 4: Build pipeline — `dx bundle --base-path`

**Files:**
- Modify: `Dockerfile` (build-arg + `dx bundle` invocation, ~lines 50-72)
- Modify: `scripts/ci/test_ui_bundle_pin.py` (pin the new plumbing)
- Modify: `README.md` (build snippet, ~lines 1159-1175)

**Interfaces:**
- Consumes: `dx bundle --base-path` (Dioxus CLI 0.7.10, already pinned: `ARG DIOXUS_CLI_VERSION=0.7.10`).
- Produces: Docker build arg `MEMORY_MCP_UI_BASE_PATH` (default **empty** = root bundle). Its value must equal the path of the runtime `MEMORY_MCP_HTTP_PUBLIC_BASE_URL`.

- [ ] **Step 1: Write the failing pin test**

In `scripts/ci/test_ui_bundle_pin.py`, next to the existing `dx bundle` assertion:

```python
    def test_base_path_plumbing(self):
        # The UI bundle bakes its mount prefix at build time; the runtime
        # twin is the path of MEMORY_MCP_HTTP_PUBLIC_BASE_URL (see
        # docs/superpowers/specs/2026-09-23-path-prefix-deployment.md).
        # The arg must default to empty so root builds stay zero-config.
        self.assertIn("ARG MEMORY_MCP_UI_BASE_PATH=", self.text)
        self.assertIn("--base-path", self.text)
        self.assertIn("${MEMORY_MCP_UI_BASE_PATH}", self.text)
```

Run: `python3 scripts/ci/test_ui_bundle_pin.py`
Expected: FAIL — `AssertionError: ARG MEMORY_MCP_UI_BASE_PATH= not found`.

- [ ] **Step 2: Implement the Dockerfile plumbing**

In the build stage next to `ARG DIOXUS_CLI_VERSION=0.7.10` add:

```dockerfile
# Mount prefix for the control-plane bundle (empty = origin root). Must
# equal the path of the deployed MEMORY_MCP_HTTP_PUBLIC_BASE_URL.
ARG MEMORY_MCP_UI_BASE_PATH=
```

and extend the `dx bundle` `RUN` (keep its existing shell flags and comments):

```dockerfile
    base_args=""; \
    if [ -n "${MEMORY_MCP_UI_BASE_PATH}" ]; then \
        base_args="--base-path ${MEMORY_MCP_UI_BASE_PATH}"; \
    fi; \
    dx bundle --platform web --release --package control-plane-ui --out-dir /src/control-plane-ui-dist ${base_args}; \
```

Update the README build snippet (~lines 1159-1175): show the flag and add one sentence — the `--base-path` value must equal the path of `MEMORY_MCP_HTTP_PUBLIC_BASE_URL` (e.g. `/memory`).

- [ ] **Step 3: Run the pin test to verify it passes**

Run: `python3 scripts/ci/test_ui_bundle_pin.py`
Expected: PASS.

- [ ] **Step 4: Verify a real prefixed bundle**

```bash
cd crates/control-plane-ui && dx bundle --platform web --release --base-path /memory \
  --out-dir "$PWD/../../target/ui-prefix-check" && cd ../..
grep -o 'name="dioxus-asset-root" content="/memory"' target/ui-prefix-check/public/index.html
grep -o '/memory/assets/[A-Za-z0-9._-]*' target/ui-prefix-check/public/index.html | head -5
grep -rl '/memory' target/ui-prefix-check/public/ | grep '\.wasm$'
```
Expected: the meta element matches exactly once; prefixed asset URLs are printed (or relative URLs resolved through the meta — record which shape `dx` produced in the commit message); the last line proves the prefix is baked into the WebAssembly too (`WebHistory`'s release build reads it from the compiled env), so router navigation will be prefix-aware. If the wasm grep misses while the meta is present, stop and verify router navigation in a browser before proceeding — do not assume.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
git add Dockerfile scripts/ci/test_ui_bundle_pin.py README.md
git commit -m "build: plumb MEMORY_MCP_UI_BASE_PATH into dx bundle --base-path"
```

---

### Task 5: Docs — deployment contract (README)

**Files:**
- Modify: `README.md` (HTTP SaaS configuration docs)

**Interfaces:**
- Consumes: everything from Tasks 1-4. Produces: operator-facing runbook (no code).

- [ ] **Step 1: Sharpen the `MEMORY_MCP_HTTP_PUBLIC_BASE_URL` documentation**

Wherever the variable is listed, add: its **path** is the mount base of the server (`https://mcp.example/memory`); no path means the origin root.

- [ ] **Step 2: Write the "Deploying under a path prefix" section**

```markdown
## Deploying under a path prefix

`memory_mcp_http` can live entirely under one path prefix so the host root
stays free for other services (see
`docs/superpowers/specs/2026-09-23-path-prefix-deployment.md`). There is no
separate knob: the path of `MEMORY_MCP_HTTP_PUBLIC_BASE_URL` *is* the prefix.

Two values must agree:

- runtime: `MEMORY_MCP_HTTP_PUBLIC_BASE_URL=https://mcp.example/memory`
  (the server answers only under `/memory`, `308`-canonicalizes
  `/memory/` → `/memory`, and `404`s every root path);
- build time: `dx bundle --base-path /memory` (Docker:
  `--build-arg MEMORY_MCP_UI_BASE_PATH=/memory`) — bakes the same prefix
  into the SPA bundle. **A mismatch between the two breaks the UI** (asset
  requests land outside the prefix); rebuild the bundle when the prefix
  changes.

The OIDC redirect URI keeps its canonical path under the prefix
(`https://mcp.example/memory/auth/oidc/callback`) — register exactly that at
the identity provider.

Reverse-proxy contract (Pangolin): route the prefix **without rewriting** —
`/memory` → `http://127.0.0.1:8080` forwards `/memory/mcp`,
`/memory/api/v1/...` etc. unchanged. The MCP endpoint is
`https://mcp.example/memory/mcp` (give clients the full URL; if a client
appends `/mcp` itself, give it `https://mcp.example/memory`).

With a prefix set, session cookies are `__Secure-` scoped to `Path={base}/`
so sibling services on the same host never receive them; at the origin root
the `__Host-` + `Path=/` contract is kept.
```

(If Task 6 is skipped, replace the cookie paragraph with: "Session cookies remain `__Host-` with `Path=/` and are therefore sent to every service on the shared host.")

- [ ] **Step 3: Verify and commit**

```bash
cargo fmt --all --check
git add README.md
git commit -m "docs: runbook for path-prefix deployment behind a reverse proxy"
```

---

### Task 6 (optional, separable): Scope session cookies to the base path

**Skip condition:** the operator accepts `Path=/` cookies being sent to sibling
services on the shared host. Nothing breaks functionally without this task —
`__Host-` + `Path=/` works at any base. Do it if the shared host may ever host
a service that must not receive `memory_mcp` session credentials.

**Files:**
- Modify: `crates/memory-mcp/src/control/session.rs` (`build_session_cookie`; add name/path/clear/parse helpers)
- Modify: `crates/memory-mcp/src/control/oidc/handlers.rs:112` (clear-cookie literal)
- Modify: `crates/memory-mcp/src/http/middleware/auth.rs:93` (cookie parse)
- Modify: `crates/memory-mcp/src/control/local_admin/csrf.rs:34,37` (name constants → functions) and `:275` (parse)
- Modify: `crates/memory-mcp/src/control/local_admin.rs:118` (parse)
- Modify: `crates/memory-mcp/src/control/local_admin/handlers.rs:821-839` (`session_cookie`, `clear_session_cookie`, `clear_preauth_cookie`)
- Modify: `crates/memory-mcp/src/service/local_admin/auth.rs:474,552` (`AdminLogin.cookie` → `AdminLogin.cookie_value` carrying the bare hex)
- Modify: `crates/memory-mcp/src/http/registry/surreal_store/local_admin_remote.rs:325-329` (decode accepts either name)
- Test updates: `crates/memory-mcp/src/control/oidc.rs` (logout test), `crates/memory-mcp/src/http/registry/surreal_store/local_admin.rs:2055,2444` (fixtures)

**Interfaces:**
- Produces in `control/session.rs`: `session_cookie_name(base_path: &str) -> &'static str`, `session_cookie_path(base_path: &str) -> String`, `clear_session_cookie(cfg: &HttpConfig) -> String`, `parse_session_cookie(header_value: &str, base_path: &str) -> Option<&str>`.
- Produces in `control/local_admin/csrf.rs`: `session_cookie_name(base_path)`, `preauth_cookie_name(base_path)` (same `__Host-`/`__Secure-` switch, `_preauth` suffix).
- Produces: `AdminLogin.cookie_value: String` (bare hex verifier).

- [ ] **Step 1: Write the failing tests**

In `control/session.rs` (create the test module):

```rust
#[cfg(test)]
mod cookie_tests {
    use super::*;

    #[test]
    fn root_deployments_keep_the_host_prefixed_cookie() {
        let cfg = crate::http::config::HttpConfig::default_for_test();
        assert_eq!(build_session_cookie("v".into(), &cfg),
            "__Host-memory_mcp_session=v; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=86400");
    }

    #[test]
    fn a_base_path_scopes_the_cookie_and_drops_the_host_prefix() {
        let mut cfg = crate::http::config::HttpConfig::default_for_test();
        cfg.base_path = "/memory".into();
        let cookie = build_session_cookie("v".into(), &cfg);
        assert!(cookie.starts_with("__Secure-memory_mcp_session=v; Path=/memory/;"),
                "got: {cookie}");
        assert!(!cookie.contains("__Host-"), "got: {cookie}");
    }

    #[test]
    fn clearing_uses_the_same_name_and_path_as_setting() {
        let mut cfg = crate::http::config::HttpConfig::default_for_test();
        cfg.base_path = "/memory".into();
        let clear = clear_session_cookie(&cfg);
        assert!(clear.starts_with("__Secure-memory_mcp_session=; Path=/memory/;"), "got: {clear}");
        assert!(clear.contains("Max-Age=0"), "got: {clear}");
    }

    #[test]
    fn parsing_prefers_the_configured_name_and_accepts_the_other() {
        let hdr = "other=1; __Host-memory_mcp_session=abc; __Secure-memory_mcp_session=def";
        // Preferred name wins even though the other one appears first.
        assert_eq!(parse_session_cookie(hdr, ""), Some("abc"));
        assert_eq!(parse_session_cookie(hdr, "/memory"), Some("def"));
        // The non-configured name still parses (config changes never wedge).
        assert_eq!(parse_session_cookie("__Host-memory_mcp_session=abc", "/memory"), Some("abc"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p memory_mcp cookie_tests`
Expected: compile FAIL (`clear_session_cookie`, `parse_session_cookie` missing).

- [ ] **Step 3: Implement**

`control/session.rs` — replace `build_session_cookie` and add helpers:

```rust
/// `__Host-` requires `Path=/` (cookie spec), which on a shared host sends
/// the credential to every sibling service. Under a base path we scope the
/// cookie instead and must downgrade to `__Secure-`.
pub fn session_cookie_name(base_path: &str) -> &'static str {
    if base_path.is_empty() {
        "__Host-memory_mcp_session"
    } else {
        "__Secure-memory_mcp_session"
    }
}

pub fn session_cookie_path(base_path: &str) -> String {
    if base_path.is_empty() {
        "/".to_owned()
    } else {
        format!("{base_path}/")
    }
}

pub fn build_session_cookie(cookie_value: String, cfg: &crate::http::config::HttpConfig) -> String {
    format!(
        "{}={cookie_value}; Path={}; Secure; HttpOnly; SameSite=Lax; Max-Age=86400",
        session_cookie_name(&cfg.base_path),
        session_cookie_path(&cfg.base_path),
    )
}

pub fn clear_session_cookie(cfg: &crate::http::config::HttpConfig) -> String {
    format!(
        "{}=; Path={}; Secure; HttpOnly; SameSite=Lax; Max-Age=0",
        session_cookie_name(&cfg.base_path),
        session_cookie_path(&cfg.base_path),
    )
}

/// Accepts both names deliberately: the configured one is preferred, the
/// other keeps a session readable across a config change. Preference must
/// win over header order — a browser sends both during a migration.
pub fn parse_session_cookie<'a>(header_value: &'a str, base_path: &str) -> Option<&'a str> {
    let find = |name: &str| {
        header_value.split(';').find_map(|cookie| {
            let (cookie_name, value) = cookie.trim().split_once('=')?;
            (cookie_name == name).then_some(value)
        })
    };
    find(session_cookie_name(base_path))
        .or_else(|| find("__Host-memory_mcp_session"))
        .or_else(|| find("__Secure-memory_mcp_session"))
}
```

Then:
- `oidc/handlers.rs:112`: replace the literal with `crate::control::session::clear_session_cookie(&state.config)`.
- `middleware/auth.rs:93`: replace the inline `find_map` with `session::parse_session_cookie(header_str, &state.config.base_path)` (keep the surrounding `to_str()` plumbing).
- `control/local_admin/csrf.rs`: turn `SESSION_COOKIE`/`PREAUTH_COOKIE` into `pub fn session_cookie_name(base_path: &str)` / `pub fn preauth_cookie_name(base_path: &str)` with the same `__Host-`/`__Secure-` switch and the `_preauth` suffix; make `csrf.rs:275` and `control/local_admin.rs:118` accept either name using the same prefer-configured-two-pass shape.
- `control/local_admin/handlers.rs`: `session_cookie(name: &str, path: &str, value: &str, max_age: Option<i64>) -> String` keeping today's attribute string (`Secure; HttpOnly; SameSite=Strict`); `clear_session_cookie`/`clear_preauth_cookie` take `&HttpConfig` and use the helpers.
- `service/local_admin/auth.rs`: `AdminLogin` gains `cookie_value: String` (bare hex; `cookie` field removed); `login`/`reauthenticate` fill `hex::encode(cookie_verifier)` / `hex::encode(new_cookie_verifier)`. Update the two `surreal_store/local_admin.rs` test fixtures to `hex::decode(&login.cookie_value)` and make `local_admin_remote.rs`'s decode accept either cookie name.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p memory_mcp`
Expected: PASS — `cookie_tests` green, OIDC logout test asserts via `clear_session_cookie`, all local-admin tests green.

- [ ] **Step 5: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
git add -A crates/memory-mcp
git commit -m "feat(auth): scope session cookies to the mount base path on shared hosts"
```

---

## Self-review (run at planning time)

- **Spec coverage:** base derivation + grammar (Task 1); nested mount, root refusal, `{base}/` canonicalization, boundary invariant (Task 2); UI fetch/link base (Task 3); bundle base baking (Task 4); proxy contract + operator docs (Task 5); cookie scoping (Task 6). `/metrics` correctly absent (spec non-goal). Redirect/OIDC URLs correctly absent from code tasks (config-only, documented in Task 5).
- **Placeholder scan:** every step carries real code or exact commands. No TBD/TODO.
- **Type/consistency check:** `derive_base_path(&str) -> Result<String, MemoryError>` used identically in Task 1 tests and implementation; `HttpConfig.base_path: String` consumed by Tasks 2 and 6; `join(base, path)` signature matches `ApiClient::endpoint` delegation and `base::url` in Task 3; `AdminLogin.cookie_value` named consistently across Task 6; test discriminators match the actual response bodies of the implementation (`"not found"` from `serve_asset` vs the `not_found` JSON envelope vs the 400 JSON-RPC envelope).
- **Empirical grounding (verified against axum 0.8.8 / matchit 0.8.4 during review):** nested fallback receives the stripped path (`/memory/x/y` → `/x/y`); `GET {base}` reaches the nested fallback as `/`; `GET {base}/` matches nothing (empty catch-all tail) and needs the explicit canonicalization; `GET {base}/mcp` yields 405 through the stripped path; root paths 404; boundary layers applied after `nest` wrap nested routes and the gap fallback alike.
- **Zero-config check:** no new env vars; the single build arg defaults to empty; public URLs without a path behave exactly as before.
