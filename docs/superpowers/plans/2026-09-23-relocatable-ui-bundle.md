# Relocatable UI Bundle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the control-plane UI bundle prefix-free at build time and stamp the mount base into it at startup, so one `memory_mcp_http` image serves any path prefix and the build/runtime base mismatch (image 1.11.0 incident) becomes structurally impossible.

**Architecture:** The UI bundle is always built with a sentinel base (`dx bundle --base-path /__memory_mcp_base__`), so every prefix-dependent URL in the served shell and the `DIOXUS_ASSET_ROOT` meta carry the sentinel. At router assembly `memory_mcp_http` replaces the sentinel with the path of `MEMORY_MCP_HTTP_PUBLIC_BASE_URL` (the existing, already-validated mount base) and serves those stamped bytes for `index.html`. The WASM resolves its base at runtime from the `DIOXUS_ASSET_ROOT` meta (which the server stamps) instead of the compile-time `option_env!` the release build of `dioxus-cli-config` bakes, and the router gets the same base through an explicit `WebHistory` prefix.

**Tech Stack:** Rust 1.97.1, axum 0.8.8, Dioxus 0.7.10 (`dioxus-web` `Config::history` + `WebHistory::new(Some(prefix), …)`), `dioxus-cli-config` 0.7.10 (`get_meta_contents`, `ASSET_ROOT_ENV`), `dx bundle` (dioxus-cli 0.7.10), Playwright (`scripts/ci/local_admin_browser.mjs`).

**Spec:** `docs/superpowers/specs/2026-09-23-path-prefix-deployment.md` — this plan **amends** its Decision 3 and removes the matching non-goal (Task 1 is that amendment; implementers read the spec as amended).

## Global Constraints

- **Zero new environment variables.** The one knob stays the path of `MEMORY_MCP_HTTP_PUBLIC_BASE_URL`. The Docker build arg `MEMORY_MCP_UI_BASE_PATH` is **deleted**, not extended.
- The sentinel literal is `"/__memory_mcp_base__"` (unreserved URL characters only). It lives in exactly five code sites and they must change together: `crates/memory-mcp/src/control/static_assets.rs` (`BASE_PATH_SENTINEL`), `crates/control-plane-ui/src/base.rs` (`BASE_PATH_SENTINEL`), `crates/control-plane-ui/index.html` (favicon href), the `Dockerfile` (`dx bundle --base-path` and its `grep` assertion), and `scripts/ci/local_admin_browser.mjs` (the sentinel check). README/spec prose follows automatically.
- Public-URL base grammar (unchanged): empty (origin root) or `/`-prefixed, no trailing `/`, no empty/`.`/`..` segments, unreserved characters only. Violations fail config load with `MemoryError::ConfigInvalid`.
- Public URL without a path ⇒ **zero behavior change** at root: after stamping with `base == ""` the shell's URLs are byte-equivalent to today's root-absolute URLs. The existing test suite must stay green (only the mechanical `build_router` signature updates of Task 4 touch test call sites).
- No `unwrap()`/`expect()` in production code paths (tests may use them).
- `MemoryError::ConfigInvalid(String)` is the error for every stamping contract violation (repo convention: thiserror-based `MemoryError`, descriptive messages).
- Dependency changes need operator approval before merge (AGENTS.md). Task 7 adds exactly one pinned dependency already resolved in `Cargo.lock` through `dioxus`: `dioxus-web = "=0.7.10"`.
- Gates before every commit: `cargo fmt --all`, then `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings`, `cargo test -p memory_mcp`. Tasks 3 and 4 additionally run `cargo test -p memory_mcp --features streamable-http` — the `control` and `http` modules are feature-gated (`#[cfg(feature = "control-plane")]` / `"streamable-http"`), so the stamping code and its tests compile only with that feature and the bare `cargo test -p memory_mcp` run does not cover them. UI tasks additionally run `cargo test -p control-plane-ui` and `cargo check -p control-plane-ui --target wasm32-unknown-unknown`.
- `/metrics` exposure, more than one mount prefix per process, and `MEMORY_MCP_CONTROL_PLANE_UI_DIST` semantics (optional bundle, embedded at compile time) are out of scope and unchanged.

---

### Task 1: Amend the path-prefix spec (build-time prefix → relocatable bundle)

**Files:**
- Modify: `docs/superpowers/specs/2026-09-23-path-prefix-deployment.md`

**Interfaces:**
- Produces: the amended Decision 3 and amended Non-goals that Tasks 2–11 implement. Consumed by: every later task's implementer.

- [ ] **Step 1: Record the amendment in the header**

Directly under the line `**Status:** Approved for planning` insert:

```markdown
**Amended:** 2026-09-23 — Decision 3 is replaced: the UI bundle no longer bakes
the mount prefix at build time. It is built relocatable (sentinel base) and the
server stamps the deployed base into the served shell at startup. The non-goal
"Runtime-variable base for the UI bundle" is removed accordingly.
Implementation: `docs/superpowers/plans/2026-09-23-relocatable-ui-bundle.md`.
```

- [ ] **Step 2: Replace Decision 3**

Replace the whole paragraph beginning `**3. The UI bundle bakes the same prefix at build time**` with:

```markdown
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
```

- [ ] **Step 3: Remove the reversed non-goal**

Delete the line:

```markdown
- Runtime-variable base for the UI bundle (Dioxus bakes it at build time).
```

- [ ] **Step 4: Update the Configuration surface and Compatibility text**

In the Configuration surface section replace:

```markdown
Build-time: `dx bundle --base-path /memory` (Docker: `--build-arg
MEMORY_MCP_UI_BASE_PATH=/memory`, default empty). **Zero new env vars.**
```

with:

```markdown
Build-time: `dx bundle --base-path /__memory_mcp_base__` — the relocatable-bundle
sentinel, never a deployment prefix (Docker: hardcoded in the `Dockerfile`, no
build arg). **Zero new env vars.**
```

In the Compatibility section, after the sentence ending `(one-time re-login, same class as the documented \`browser_policy_epoch\` break)`, add:

```markdown
The UI bundle is prefix-free from this amendment on: the same image serves the
origin root and any prefix, stamped at startup. Prefix changes no longer
invalidate browser caches of hashed assets for a rebuilt bundle, because no
rebuild happens.
```

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/specs/2026-09-23-path-prefix-deployment.md
git commit -m "docs: amend path-prefix spec — relocatable UI bundle, runtime-stamped base"
```

---

### Task 2: Spike — verify the `dx bundle` sentinel contract

Throwaway verification (brainstorming spike discipline: nothing here is committed). Run before writing code that assumes the answers. `dx` is the pinned dioxus-cli 0.7.10 (`cargo install dioxus-cli --version 0.7.10 --locked` if absent — same pin as the `Dockerfile`).

**Files:** none (probe output under `/tmp`).

**Interfaces:**
- Produces: the factual basis for Task 5 (which favicon variant applies) and for the sentinel constant in Tasks 3, 6, 8.

- [ ] **Step 1: Build a probe bundle with the sentinel base**

```bash
cd /Users/solovey/Documents/dev/memory_mcp
rm -rf /tmp/mmcp-ui-probe
dx bundle --platform web --release --package control-plane-ui \
  --base-path /__memory_mcp_base__ \
  --out-dir /tmp/mmcp-ui-probe
test -s /tmp/mmcp-ui-probe/public/index.html
```

Expected: exit 0. **STOP condition A:** if `dx` rejects or normalizes the sentinel value (validation error, or the output contains a percent-encoded/normalized variant instead of the literal `/__memory_mcp_base__`), stop and report to the operator — the sentinel constant must change before any other task proceeds.

- [ ] **Step 2: Inspect what `dx` wrote into the shell**

```bash
grep -o 'href="[^"]*"\|src="[^"]*"\|name="DIOXUS_ASSET_ROOT" content="[^"]*"' \
  /tmp/mmcp-ui-probe/public/index.html
```

Expected (all three must hold):
1. The injected css/js refs contain `/__memory_mcp_base__/` (the `/./` join artifact, e.g. `href="/__memory_mcp_base__/./assets/main-….css"`, is fine — the browser normalizes `.` segments).
2. `name="DIOXUS_ASSET_ROOT" content="/__memory_mcp_base__"` is present (`dx` emits the meta via `dioxus_cli_config::format_base_path_meta_element`).
3. The favicon is exactly one of two outcomes, recorded for Task 5:
   - **Variant A (expected):** `href="/assets/favicon.svg"` — `dx` does **not** rewrite hand-written literal hrefs in `index.html`.
   - **Variant B:** `href="/__memory_mcp_base__/assets/favicon.svg"` — `dx` prefixes literal asset hrefs too.

- [ ] **Step 3: Confirm the fallback value baked into the WASM is the sentinel (informational)**

```bash
grep -a -c '__memory_mcp_base__' /tmp/mmcp-ui-probe/public/*.js /tmp/mmcp-ui-probe/public/*.wasm || true
```

Expected: at least one hit (the `option_env!("DIOXUS_ASSET_ROOT")` constant compiled into the release WASM carries the sentinel). This is why Task 6 filters the sentinel out of every fallback value; the stamped meta is the only live source.

---

### Task 3: `stamp_index_html` — the pure stamping function

**Files:**
- Modify: `crates/memory-mcp/src/control/static_assets.rs` (new consts + functions, extend the existing `#[cfg(test)]` module)

**Interfaces:**
- Produces: `pub(crate) const BASE_PATH_SENTINEL: &str = "/__memory_mcp_base__";`
- Produces: `pub(crate) fn stamp_index_html(raw: &[u8], base: &str) -> Result<Vec<u8>, MemoryError>`. Consumed by Task 4's `build_stamped_index`.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)]` module of `static_assets.rs`:

```rust
    const PROBE_INDEX: &str = r#"<!DOCTYPE html><html><head><meta name="DIOXUS_ASSET_ROOT" content="/__memory_mcp_base__"><link rel="icon" type="image/svg+xml" href="/__memory_mcp_base__/assets/favicon.svg"></head><body><script src="/__memory_mcp_base__/./assets/app-1.js"></script></body></html>"#;

    #[test]
    fn stamps_every_sentinel_occurrence_with_the_mount_base() {
        let out = stamp_index_html(PROBE_INDEX.as_bytes(), "/memory").expect("stamps");
        let out = String::from_utf8(out).expect("utf-8");
        assert!(!out.contains(BASE_PATH_SENTINEL));
        assert!(out.contains(r#"href="/memory/assets/favicon.svg""#));
        assert!(out.contains(r#"src="/memory/./assets/app-1.js""#));
        assert!(out.contains(r#"<meta name="DIOXUS_ASSET_ROOT" content="/memory">"#));
    }

    #[test]
    fn an_empty_base_reproduces_root_absolute_urls() {
        let out = stamp_index_html(PROBE_INDEX.as_bytes(), "").expect("stamps");
        let out = String::from_utf8(out).expect("utf-8");
        assert!(out.contains(r#"href="/assets/favicon.svg""#));
        assert!(out.contains(r#"src="/./assets/app-1.js""#));
        assert!(out.contains(r#"<meta name="DIOXUS_ASSET_ROOT" content="">"#));
    }

    #[test]
    fn a_root_base_accepts_a_legacy_bundle_without_the_sentinel() {
        let raw = r#"<!DOCTYPE html><html><head></head><body></body></html>"#;
        assert!(stamp_index_html(raw.as_bytes(), "").is_ok());
    }

    #[test]
    fn a_prefixed_base_rejects_a_bundle_without_the_sentinel() {
        let raw = r#"<!DOCTYPE html><html><head></head><body></body></html>"#;
        let err = stamp_index_html(raw.as_bytes(), "/memory").expect_err("must reject");
        assert!(matches!(err, MemoryError::ConfigInvalid(_)), "{err:?}");
    }

    #[test]
    fn missing_meta_is_injected_so_the_wasm_reads_the_deployed_base() {
        let raw = r#"<!DOCTYPE html><html><head><title>t</title></head><body><script src="/__memory_mcp_base__/./assets/app-1.js"></script></body></html>"#;
        let out = stamp_index_html(raw.as_bytes(), "/memory").expect("stamps");
        let out = String::from_utf8(out).expect("utf-8");
        assert!(out.contains(r#"<head><meta name="DIOXUS_ASSET_ROOT" content="/memory">"#));
    }

    #[test]
    fn a_stale_meta_content_is_rewritten_to_the_deployed_base() {
        let raw = r#"<!DOCTYPE html><html><head><meta name="DIOXUS_ASSET_ROOT" content=""></head><body><script src="/__memory_mcp_base__/./assets/app-1.js"></script></body></html>"#;
        let out = stamp_index_html(raw.as_bytes(), "/memory").expect("stamps");
        let out = String::from_utf8(out).expect("utf-8");
        assert!(out.contains(r#"<meta name="DIOXUS_ASSET_ROOT" content="/memory">"#));
    }

    #[test]
    fn non_utf8_input_is_a_config_error() {
        let err = stamp_index_html(&[0xff, 0xfe], "/memory").expect_err("must reject");
        assert!(matches!(err, MemoryError::ConfigInvalid(_)), "{err:?}");
    }

    #[test]
    fn a_base_that_itself_contains_the_sentinel_is_rejected() {
        // The public-URL grammar would allow such a base segment; stamping it
        // would leave sentinel residue in the served shell, so it must fail
        // loudly at router assembly instead.
        let err = stamp_index_html(PROBE_INDEX.as_bytes(), BASE_PATH_SENTINEL)
            .expect_err("must reject");
        assert!(matches!(err, MemoryError::ConfigInvalid(_)), "{err:?}");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p memory_mcp --features streamable-http --lib static_assets`
Expected: FAIL — `cannot find function stamp_index_html` / `cannot find value BASE_PATH_SENTINEL`.

(The `--features streamable-http` is required throughout Tasks 3–4: `control` is `#[cfg(feature = "control-plane")]`, which `streamable-http` implies.)

- [ ] **Step 3: Write the minimal implementation**

Add near the top of `static_assets.rs` (next to `INDEX_PATH`). The module needs `use crate::error::MemoryError;`:

```rust
/// Build-time marker baked into the UI bundle by
/// `dx bundle --base-path /__memory_mcp_base__`. The compiled `index.html`
/// carries it in every prefix-dependent URL and in the `DIOXUS_ASSET_ROOT`
/// meta; [`stamp_index_html`] replaces it with the deployed mount base at
/// startup. The same literal lives in `crates/control-plane-ui/src/base.rs`,
/// `crates/control-plane-ui/index.html`, the `Dockerfile`'s `dx bundle`
/// invocation and `scripts/ci/local_admin_browser.mjs` — all five must change
/// together.
pub(crate) const BASE_PATH_SENTINEL: &str = "/__memory_mcp_base__";

const META_PREFIX: &str = r#"<meta name="DIOXUS_ASSET_ROOT" content=""#;
const HEAD_TAG: &str = "<head>";

/// Replaces every sentinel occurrence in the compiled `index.html` with
/// `base` and guarantees the `DIOXUS_ASSET_ROOT` meta carries exactly `base`,
/// so the WASM's runtime base resolution reads the deployed base. `base` is
/// the validated mount base (`""` = origin root).
pub(crate) fn stamp_index_html(raw: &[u8], base: &str) -> Result<Vec<u8>, MemoryError> {
    let html = std::str::from_utf8(raw).map_err(|_| {
        MemoryError::ConfigInvalid(
            "the embedded control-plane index.html is not valid UTF-8".to_string(),
        )
    })?;
    if base.contains(BASE_PATH_SENTINEL) {
        return Err(MemoryError::ConfigInvalid(
            "the mount base itself must not contain the bundle sentinel".to_string(),
        ));
    }
    if !base.is_empty() && !html.contains(BASE_PATH_SENTINEL) {
        return Err(MemoryError::ConfigInvalid(format!(
            "the embedded control-plane bundle was not built with \
             `dx bundle --base-path {BASE_PATH_SENTINEL}`; rebuild the UI bundle \
             (see README \"Control-plane UI asset packaging\")"
        )));
    }
    let stamped = html.replace(BASE_PATH_SENTINEL, base);
    let stamped = with_meta(stamped, base);
    debug_assert!(!stamped.contains(BASE_PATH_SENTINEL));
    Ok(stamped.into_bytes())
}

/// Rewrites the `content` of the `DIOXUS_ASSET_ROOT` meta to `base`, or
/// inserts the element at the start of `<head>` when the bundle has none.
fn with_meta(html: String, base: &str) -> String {
    if let Some(start) = html.find(META_PREFIX) {
        let value_start = start + META_PREFIX.len();
        let width = html[value_start..].find('"').unwrap_or(0);
        let mut out = String::with_capacity(html.len() + base.len());
        out.push_str(&html[..value_start]);
        out.push_str(base);
        out.push_str(&html[value_start + width..]);
        return out;
    }
    let insert_at = html.find(HEAD_TAG).map_or(0, |pos| pos + HEAD_TAG.len());
    let mut out = String::with_capacity(html.len() + base.len() + META_PREFIX.len() + 2);
    out.push_str(&html[..insert_at]);
    out.push_str(META_PREFIX);
    out.push_str(base);
    out.push('"');
    out.push('>');
    out.push_str(&html[insert_at..]);
    out
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p memory_mcp --features streamable-http --lib static_assets`
Expected: PASS (all eight new tests).

- [ ] **Step 5: Run the gates and commit**

```bash
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
cargo test -p memory_mcp
cargo test -p memory_mcp --features streamable-http
git add crates/memory-mcp/src/control/static_assets.rs
git commit -m "feat: pure index.html base-stamping for the relocatable UI bundle"
```

---

### Task 4: Wire stamping into `build_router` and `serve_asset`

**Files:**
- Modify: `crates/memory-mcp/src/control/static_assets.rs` (`build_stamped_index`, `serve_asset`, `serve_asset_from`, `index_response`; fixture call sites in tests)
- Modify: `crates/memory-mcp/src/http/router.rs:16-18` (signature), `:288-306` (fallback closure)
- Modify: `crates/memory-mcp/src/bin/memory_mcp_http.rs:108-112` (call site)
- Modify: `crates/memory-mcp/src/http/middleware/auth.rs:255-306` (two test call sites), `crates/memory-mcp/src/http/health.rs:62`, `crates/memory-mcp/src/http/metrics.rs:84` (one test call site each)

**Interfaces:**
- Consumes: `stamp_index_html` from Task 3; `HttpConfig.base_path: String` (existing).
- Produces: `pub fn build_stamped_index(base: &str) -> Result<Option<Arc<[u8]>>, MemoryError>` — `None` when no UI is compiled into the binary.
- Produces: `pub fn serve_asset(path: &str, stamped_index: Option<&[u8]>) -> Response`.
- Produces: `pub fn build_router(state: Arc<HttpState>, control_plane_injector: Option<Arc<dyn FaultInjector>>) -> Result<Router, MemoryError>`.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)]` module of `static_assets.rs` (existing `response_body` helper is reused):

```rust
    #[tokio::test]
    async fn a_stamped_index_replaces_the_compiled_document_on_spa_routes() {
        let response = serve_asset_from(
            "/admin/clients",
            FIXTURE_ASSETS,
            Some(b"stamped-fixture".as_slice()),
        );
        assert_eq!(response_body(response).await, b"stamped-fixture");
    }

    #[tokio::test]
    async fn the_index_path_also_serves_the_stamped_document() {
        let response = serve_asset_from(
            "/index.html",
            FIXTURE_ASSETS,
            Some(b"stamped-fixture".as_slice()),
        );
        assert_eq!(response_body(response).await, b"stamped-fixture");
    }
```

(The second test documents that `/index.html` — the exact-path match on the index entry — also serves the stamped bytes; see Step 2. The tests follow the module's existing `#[tokio::test]` + `response_body` helper pattern.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p memory_mcp --features streamable-http --lib static_assets`
Expected: FAIL — `this function takes 2 arguments but 3 arguments were supplied`.

- [ ] **Step 3: Implement in `static_assets.rs`**

Add the import `use std::sync::Arc;` and:

```rust
/// Stamps the compiled `index.html` with the deployed mount base once, at
/// router assembly. `None` when no UI bundle is compiled into the binary.
pub fn build_stamped_index(base: &str) -> Result<Option<Arc<[u8]>>, MemoryError> {
    let Some(index) = ASSETS.iter().find(|asset| asset.path == INDEX_PATH) else {
        return Ok(None);
    };
    Ok(Some(Arc::from(
        stamp_index_html(index.body, base)?.into_boxed_slice(),
    )))
}
```

Change the two signatures and the index branch (exact old → new):

```rust
pub fn serve_asset(path: &str) -> Response {
    serve_asset_from(path, ASSETS)
}
```
→
```rust
/// Serve a compiled Dioxus asset by path. `stamped_index` is the
/// mount-base-stamped `index.html` from [`build_stamped_index`]; SPA routes
/// and `/index.html` serve those bytes instead of the raw compiled document.
pub fn serve_asset(path: &str, stamped_index: Option<&[u8]>) -> Response {
    serve_asset_from(path, ASSETS, stamped_index)
}
```

```rust
fn serve_asset_from(path: &str, assets: &[Asset]) -> Response {
```
→
```rust
fn serve_asset_from(path: &str, assets: &[Asset], stamped_index: Option<&[u8]>) -> Response {
```

Inside `serve_asset_from`, the exact-path match and the index branch become:

```rust
    if let Some(asset) = assets.iter().find(|asset| asset.path == path) {
        return if asset.path == INDEX_PATH {
            index_response(asset, stamped_index)
        } else {
            asset_response(asset)
        };
    }

    if is_spa_route(path)
        && let Some(index) = assets.iter().find(|asset| asset.path == INDEX_PATH)
    {
        return index_response(index, stamped_index);
    }
```

Add `index_response` (reusing `asset_response`'s header rules via a small shared helper to keep the two from drifting):

```rust
fn asset_response(asset: &Asset) -> Response {
    asset_response_with_body(asset, Body::from(asset.body))
}

/// The index document is `no-cache` and served from the stamped buffer when
/// one exists: the compiled bytes still carry the build sentinel and must
/// never reach a client on a prefixed deployment.
fn index_response(asset: &Asset, stamped_index: Option<&[u8]>) -> Response {
    match stamped_index {
        Some(stamped) => asset_response_with_body(asset, Body::from(stamped.to_vec())),
        None => asset_response(asset),
    }
}

fn asset_response_with_body(asset: &Asset, body: Body) -> Response {
    let mut resp = Response::new(body);
    resp.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(asset.content_type));
    // Content-addressed bundle assets never change once published, so they can
    // be cached immutably. The index document and any stable-path asset (the
    // favicon) are revalidated, so a new deploy is picked up promptly.
    resp.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static(if asset.immutable {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        }),
    );
    attach_security_headers(resp)
}
```

(Delete the old one-argument `fn asset_response(asset: &Asset) -> Response` body — it is replaced by the thin wrapper above. Both constructors use `Body::from` forms already present in this file: `&'static [u8]` keeps hashed assets zero-copy, and only the ~2 KB index document is copied per request.)

Update the five existing test calls shaped `serve_asset_from(path, FIXTURE_ASSETS)` (in this module's test list at the `fixture_asset_…`, `root_and_extensionless_…`, `stable_unhashed_…` and not-found tests) to pass `None` as the third argument: `serve_asset_from(path, FIXTURE_ASSETS, None)`.

- [ ] **Step 4: Make `build_router` fallible and capture the stamped index**

In `crates/memory-mcp/src/http/router.rs` change the signature (old → new):

```rust
pub fn build_router(
    state: Arc<HttpState>,
    #[allow(unused_variables)] control_plane_injector: Option<Arc<dyn FaultInjector>>,
) -> Router {
```
→
```rust
pub fn build_router(
    state: Arc<HttpState>,
    #[allow(unused_variables)] control_plane_injector: Option<Arc<dyn FaultInjector>>,
) -> Result<Router, crate::error::MemoryError> {
```

(`router.rs` does not import `MemoryError`; the fully-qualified path in the signature above avoids adding an import.)

Insert directly above the line `let router = router.fallback(move |uri: axum::http::Uri| async move {` (the `let ui_enabled = { … };` block is computed immediately above it):

```rust
    // The SPA shell is stamped with the mount base once, here: the fallback
    // below serves these bytes for every SPA route so a prefixed deployment
    // never ships the build-time sentinel to a client. Stamping is gated on
    // `ui_enabled` so a deployment that turns the UI off never fails on a
    // bundle contract it does not serve; the `#[cfg]` mirrors the gate on
    // the `serve_asset` call inside the fallback.
    #[cfg(feature = "control-plane-ui")]
    let ui_index = if ui_enabled {
        crate::control::static_assets::build_stamped_index(&state.config.base_path)?
    } else {
        None
    };
    #[cfg(not(feature = "control-plane-ui"))]
    let ui_index: Option<std::sync::Arc<[u8]>> = None;
```

Change the closure header (old → new):

```rust
    let router = router.fallback(move |uri: axum::http::Uri| async move {
```
→
```rust
    let router = router.fallback(move |uri: axum::http::Uri| {
        let ui_index = ui_index.clone();
        async move {
```

Change the asset return and the capture sink (old → new — the `#[cfg]` attribute and the `ui_enabled` gate are the existing code, unchanged):

```rust
        #[cfg(feature = "control-plane-ui")]
        if ui_enabled {
            return crate::control::static_assets::serve_asset(path);
        }
        let _ = ui_enabled;
```
→
```rust
        #[cfg(feature = "control-plane-ui")]
        if ui_enabled {
            return crate::control::static_assets::serve_asset(path, ui_index.as_deref());
        }
        let _ = (ui_enabled, &ui_index);
```

(`let _ = (ui_enabled, &ui_index);` keeps both bindings consumed in builds without `control-plane-ui`, where the `#[cfg]` block above compiles out — the existing `let _ = ui_enabled;` line served exactly that purpose. `Option<Arc<[u8]>>::as_deref()` yields `Option<&[u8]>` directly.)

Close the extra brace the new closure header opened: the fallback's closing line is exactly `    });` and becomes `    }});` (the `async move` block's `}` is now followed by the closure body's `}`). Finally, wrap the function's final `Router` expression — the `app.layer(…)` chain ending `build_router` — in `Ok( … )` to match the new `Result` return. If the function has more than one tail return site, wrap each of them.

- [ ] **Step 5: Update the production call site**

In `crates/memory-mcp/src/bin/memory_mcp_http.rs` replace:

```rust
    bootstrap::emit_startup_log(&logger, &cfg);
    let server_result = server::serve(
        cfg,
        router::build_router(state.clone(), Some(runtime.fault_injector.clone())),
        state.shutdown.clone(),
    )
    .await;
```
with:
```rust
    let router = match router::build_router(state.clone(), Some(runtime.fault_injector.clone())) {
        Ok(router) => router,
        Err(err) => {
            eprintln!("router config error: {err}");
            return ExitCode::from(2);
        }
    };
    bootstrap::emit_startup_log(&logger, &cfg);
    let server_result = server::serve(cfg, router, state.shutdown.clone()).await;
```

(The `ExitCode::from(2)` + `eprintln!` shape matches the scheduler-config error handling directly above.)

- [ ] **Step 6: Update the four test call sites**

Old → new (same change in each file):

```rust
build_router(state, None)
```
→
```rust
build_router(state, None).expect("router builds in tests")
```

at `crates/memory-mcp/src/http/middleware/auth.rs:257` and `:306` (variables `router`, `router2`), `crates/memory-mcp/src/http/health.rs:62`, `crates/memory-mcp/src/http/metrics.rs:84`.

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p memory_mcp --features streamable-http --lib static_assets` then `cargo test -p memory_mcp --features streamable-http`
Expected: PASS (new tests green, full suite green).

- [ ] **Step 8: Run the gates and commit**

```bash
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
cargo test -p memory_mcp
cargo test -p memory_mcp --features streamable-http
git add crates/memory-mcp/src/control/static_assets.rs crates/memory-mcp/src/http/router.rs crates/memory-mcp/src/bin/memory_mcp_http.rs crates/memory-mcp/src/http/middleware/auth.rs crates/memory-mcp/src/http/health.rs crates/memory-mcp/src/http/metrics.rs
git commit -m "feat: stamp the mount base into the served UI shell at router assembly"
```

---

### Task 5: UI shell — the favicon literal carries the sentinel

Apply **exactly the variant Task 2 recorded**. Variant A is the expected outcome of `dx bundle` 0.7.10.

**Files:**
- Modify: `crates/control-plane-ui/index.html` (favicon `<link>`)
- Modify: `crates/control-plane-ui/src/base.rs` (guard test in the existing `#[cfg(test)]` module)

**Interfaces:**
- Consumes: `BASE_PATH_SENTINEL` semantics from Task 3 (documented here as a literal; the UI crate does not depend on `memory-mcp`).
- Produces: a shell whose literal (hand-written) asset URLs carry `/__memory_mcp_base__`, so Task 3's stamping covers them.

- [ ] **Step 1: Write the failing guard test (Variant A)**

Add to the `#[cfg(test)] mod tests` of `crates/control-plane-ui/src/base.rs`:

```rust
    #[test]
    fn the_document_shell_carries_the_base_sentinel_in_literal_asset_urls() {
        // `dx bundle` prefixes only the tags it injects (css/js). Hand-written
        // hrefs in index.html survive verbatim, so they must carry the
        // sentinel themselves or the favicon escapes the server-side stamp.
        let shell = include_str!("../index.html");
        assert!(
            shell.contains(r#"href="/__memory_mcp_base__/assets/favicon.svg""#),
            "index.html literal asset URLs must carry /__memory_mcp_base__ so \
             memory_mcp_http can stamp them (Variant A of the dx sentinel spike)"
        );
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p control-plane-ui the_document_shell`
Expected: FAIL (index.html still contains `href="/assets/favicon.svg"`).

- [ ] **Step 3: Edit the favicon link**

Variant A (expected — `dx` did not rewrite the literal in Task 2), old → new:

```html
        <link rel="icon" type="image/svg+xml" href="/assets/favicon.svg">
```
→
```html
        <link rel="icon" type="image/svg+xml" href="/__memory_mcp_base__/assets/favicon.svg">
```

Update the comment above it to end with: `The /__memory_mcp_base__ sentinel is replaced with the deployed mount base by memory_mcp_http at startup (README, "Deploying under a path prefix").`

**Variant B only** (Task 2 showed `dx` prefixes the literal itself): leave the source literal `href="/assets/favicon.svg"` unchanged, and change the guard test's expected string to assert the *absence* of a double prefix instead:

```rust
    #[test]
    fn the_document_shell_favicon_is_not_double_prefixed() {
        // Variant B: `dx bundle --base-path` prefixes hand-written asset hrefs
        // itself, so the source literal must stay root-absolute.
        let shell = include_str!("../index.html");
        assert!(shell.contains(r#"href="/assets/favicon.svg""#));
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p control-plane-ui`
Expected: PASS (guard test green; whole crate green).

- [ ] **Step 5: Rebuild the probe and confirm the built shell stamps cleanly**

```bash
rm -rf /tmp/mmcp-ui-probe
dx bundle --platform web --release --package control-plane-ui \
  --base-path /__memory_mcp_base__ \
  --out-dir /tmp/mmcp-ui-probe
grep -c '__memory_mcp_base__' /tmp/mmcp-ui-probe/public/index.html
```

Expected: count ≥ 3 occurrences (css, js, favicon and/or meta). **STOP condition:** the favicon line contains `__memory_mcp_base__` twice (double prefix) — report to the operator.

- [ ] **Step 6: Run the gates and commit**

```bash
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
cargo test -p control-plane-ui
cargo check -p control-plane-ui --target wasm32-unknown-unknown
git add crates/control-plane-ui/index.html crates/control-plane-ui/src/base.rs
git commit -m "fix: carry the base sentinel in the UI shell's literal asset URLs"
```

---

### Task 6: UI `base.rs` — runtime base resolution (meta-first, sentinel-filtered)

**Files:**
- Modify: `crates/control-plane-ui/src/base.rs`

**Interfaces:**
- Consumes: `dioxus_cli_config::get_meta_contents` + `dioxus_cli_config::ASSET_ROOT_ENV` (both public in the pinned 0.7.10 with the `web` feature, which the UI crate already enables), `dioxus_cli_config::base_path` (existing call).
- Produces: `pub(crate) const BASE_PATH_SENTINEL: &str = "/__memory_mcp_base__";`
- Produces: `pub(crate) fn resolve_base(meta_content: Option<String>, baked: Option<String>) -> String`
- Produces: `pub fn base_path() -> String` (signature unchanged; now meta-first and cached). Consumed by Task 7's `main.rs`.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` of `base.rs` (alongside the existing `join` tests):

```rust
    use super::{BASE_PATH_SENTINEL, resolve_base};

    #[test]
    fn the_stamped_meta_wins_over_the_baked_value() {
        assert_eq!(
            resolve_base(Some("/memory".to_string()), Some("/other".to_string())),
            "/memory"
        );
    }

    #[test]
    fn a_missing_meta_falls_back_to_the_baked_value() {
        assert_eq!(
            resolve_base(None, Some("/memory".to_string())),
            "/memory"
        );
    }

    #[test]
    fn the_unstamped_sentinel_never_leaks_into_a_url() {
        // The release WASM bakes `option_env!("DIOXUS_ASSET_ROOT")` — the
        // sentinel itself when the bundle is relocatable. It must lose to
        // "no base at all", never reach a request URL.
        assert_eq!(
            resolve_base(Some(BASE_PATH_SENTINEL.to_string()), None),
            ""
        );
        assert_eq!(
            resolve_base(None, Some(BASE_PATH_SENTINEL.to_string())),
            ""
        );
    }

    #[test]
    fn root_deployments_resolve_to_the_empty_base() {
        assert_eq!(resolve_base(Some(String::new()), None), "");
        assert_eq!(resolve_base(None, None), "");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p control-plane-ui resolve_base`
Expected: FAIL — `cannot find function resolve_base` / `cannot find value BASE_PATH_SENTINEL`.

- [ ] **Step 3: Implement**

Replace the current `base_path` body of `crates/control-plane-ui/src/base.rs` and add the new items (the `join` function and its tests stay unchanged):

```rust
/// The sentinel `dx bundle --base-path` bakes into this bundle instead of a
/// deployment prefix. `memory_mcp_http` replaces it in the served shell at
/// startup; values still carrying it mean "unstamped" and must never be used.
pub(crate) const BASE_PATH_SENTINEL: &str = "/__memory_mcp_base__";

/// The base path this bundle is served under (e.g. `/memory`), empty at root.
///
/// Resolved at runtime from the `DIOXUS_ASSET_ROOT` meta the server stamps,
/// with the compile-time `DIOXUS_ASSET_ROOT` as fallback — the exact source
/// `dioxus_web::WebHistory` would read on its own, filtered through
/// [`resolve_base`] so the unstamped sentinel can never leak into a request.
pub fn base_path() -> String {
    thread_local! {
        static BASE_PATH: std::cell::OnceCell<String> = const { std::cell::OnceCell::new() };
    }
    BASE_PATH.with(|cell| {
        cell.get_or_init(|| resolve_base(read_meta_base(), baked_base()))
            .clone()
    })
}

/// Pure precedence: the stamped meta wins, then the baked value, then the
/// origin root. Any value still carrying [`BASE_PATH_SENTINEL`] is skipped.
pub(crate) fn resolve_base(meta_content: Option<String>, baked: Option<String>) -> String {
    [meta_content, baked]
        .into_iter()
        .flatten()
        .find(|value| !value.contains(BASE_PATH_SENTINEL))
        .unwrap_or_default()
}

/// The stamped meta element (`<meta name="DIOXUS_ASSET_ROOT" content="…">`).
/// JS interop exists only on the wasm target; native tests resolve `None`.
fn read_meta_base() -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        dioxus_cli_config::get_meta_contents(dioxus_cli_config::ASSET_ROOT_ENV)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

/// `dioxus_cli_config`'s own resolution (the `option_env!` compile-time
/// constant in release builds).
fn baked_base() -> Option<String> {
    dioxus_cli_config::base_path()
}
```

Also update the module doc comment: the console is served under the base **stamped by `memory_mcp_http` at startup** (the same value is handed to `WebHistory` explicitly in `main.rs`); drop the phrase "baked by `dx bundle --base-path`".

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p control-plane-ui`
Expected: PASS (new tests plus the untouched `join` tests).

- [ ] **Step 5: Run the gates and commit**

```bash
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
cargo test -p control-plane-ui
cargo check -p control-plane-ui --target wasm32-unknown-unknown
git add crates/control-plane-ui/src/base.rs
git commit -m "feat: resolve the console base at runtime from the stamped meta"
```

---

### Task 7: UI `main.rs` — explicit runtime `WebHistory` prefix (+ pinned `dioxus-web`)

**Files:**
- Modify: `crates/control-plane-ui/Cargo.toml` (one pinned dependency)
- Modify: `crates/control-plane-ui/src/main.rs`

**Interfaces:**
- Consumes: `crate::base::base_path() -> String` (Task 6), `dioxus_web::Config::history(Rc<dyn dioxus_history::History>)` and `dioxus_web::WebHistory::new(Option<String>, bool)` (public API of the pinned 0.7.10: `cfg.rs` `Config::history`, `history.rs` `WebHistory::new`).
- Produces: a launched console whose router prefix equals the stamped base. Consumed by: the browser checks of Tasks 9 and 11.

- [ ] **Step 1: Verify the resolved `dioxus-web` version, then pin it**

Run: `grep -A 1 'name = "dioxus-web"' Cargo.lock`
Expected: `version = "0.7.10"` (the exact version `dioxus = "=0.7.10"` already resolved — the pin policy documented in `crates/control-plane-ui/Cargo.toml`). If the lock resolves anything else, stop and align the pin with the lock first.

Add to `[dependencies]` of `crates/control-plane-ui/Cargo.toml`, after the `dioxus-router` line:

```toml
# The renderer `dioxus` already resolves to; named here only so `main` can
# hand `WebHistory` an explicit runtime prefix via `Config::history` — the
# one native seam that keeps the router's URL space in step with the
# server-stamped base. Pin is exact and must keep matching what `dioxus`
# resolves to in `Cargo.lock` (same policy as the list below).
dioxus-web = "=0.7.10"
```

Run: `cargo check -p control-plane-ui --target wasm32-unknown-unknown`
Expected: PASS (dependency graph unchanged apart from naming — `cargo tree -p control-plane-ui -i dioxus-web` shows one copy before and after).

- [ ] **Step 2: Wire the runtime prefix at launch**

Replace the body of `crates/control-plane-ui/src/main.rs` (module declarations stay exactly as they are):

```rust
fn main() {
    dioxus::launch(App);
}
```
→
```rust
fn main() {
    // The router prefix is the same runtime value the fetch layer uses
    // (`crate::base::base_path`) — the base `memory_mcp_http` stamped into
    // this document. `do_scroll_restoration: true` matches `WebHistory`'s own
    // default (`Default` is `new(None, true)`), so root deployments keep
    // today's behavior byte for byte.
    let history = WebHistory::new(Some(crate::base::base_path()), true);
    dioxus::LaunchBuilder::new()
        .with_cfg(Config::new().history(Rc::new(history)))
        .launch(App);
}
```

Add the imports above `mod admin_api;`:

```rust
use std::rc::Rc;

use dioxus_web::{Config, WebHistory};
```

(keep the existing `use crate::layouts::app::App;`).

- [ ] **Step 3: Verify the router seam resolves the provided history**

Run: `cargo check -p control-plane-ui --target wasm32-unknown-unknown`
Expected: PASS. (`Config::new().history(…)` stores `Rc<dyn dioxus_history::History>` and `dioxus_web::run` provides it at root scope — `dioxus-web/src/lib.rs`; `dioxus-history`'s `history()` consumes that context and only falls back to `MemoryHistory` when none is provided, so the explicit prefix reaches `dioxus-router`. `LaunchBuilder` is re-exported at the dioxus crate root (`pub use crate::launch::*` in dioxus 0.7.10); `dioxus_web::launch::launch` downcasts the `with_cfg` value to `dioxus_web::Config`, which is exactly what is passed here; `dioxus-web`'s default features include `document`, which `Config::history` requires.)

- [ ] **Step 4: Run the tests and the gates, commit**

```bash
cargo test -p control-plane-ui
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
git add crates/control-plane-ui/Cargo.toml crates/control-plane-ui/src/main.rs Cargo.lock
git commit -m "feat: hand the console router its runtime base via WebHistory"
```

---

### Task 8: Dockerfile — always build a relocatable bundle

**Files:**
- Modify: `Dockerfile` (ui-builder stage: the `ARG MEMORY_MCP_UI_BASE_PATH` block and the `base_args` RUN fragment)

**Interfaces:**
- Consumes: the sentinel contract of Task 3.
- Produces: an image whose embedded bundle always carries `/__memory_mcp_base__`; the build fails loudly if `dx` ever stops writing it.

- [ ] **Step 1: Remove the build-arg plumbing**

Delete these lines from the `ui-builder` stage:

```dockerfile
# Mount prefix for the control-plane bundle (empty = origin root). Must
# equal the path of the deployed MEMORY_MCP_HTTP_PUBLIC_BASE_URL.
ARG MEMORY_MCP_UI_BASE_PATH=
```

- [ ] **Step 2: Build with the sentinel and assert it landed**

Replace the RUN fragment (old → new):

```dockerfile
    rm -rf /src/target/dx/control-plane-ui/release/web/public /src/control-plane-ui-dist; \
    base_args=""; \
    if [ -n "${MEMORY_MCP_UI_BASE_PATH}" ]; then \
        base_args="--base-path ${MEMORY_MCP_UI_BASE_PATH}"; \
    fi; \
    dx bundle --platform web --release --package control-plane-ui --out-dir /src/control-plane-ui-dist ${base_args}; \
    test -s /src/control-plane-ui-dist/public/index.html; \
```
→
```dockerfile
    rm -rf /src/target/dx/control-plane-ui/release/web/public /src/control-plane-ui-dist; \
    dx bundle --platform web --release --package control-plane-ui --out-dir /src/control-plane-ui-dist --base-path /__memory_mcp_base__; \
    test -s /src/control-plane-ui-dist/public/index.html; \
    grep -q '__memory_mcp_base__' /src/control-plane-ui-dist/public/index.html; \
```

The `grep` pins the relocatable-bundle contract at build time: the sentinel must be in the shell (the server refuses an unstamped bundle under a prefix — see `stamp_index_html`). Keep the existing `*.js`/`*.wasm`/`*.css` count assertions unchanged.

Add above the RUN a short comment:

```dockerfile
# The bundle is relocatable: every prefix-dependent URL in its index.html and
# its DIOXUS_ASSET_ROOT meta carry the sentinel /__memory_mcp_base__, which
# memory_mcp_http replaces with the deployed MEMORY_MCP_HTTP_PUBLIC_BASE_URL
# path at startup. Never pass a deployment prefix here. The literal must match
# BASE_PATH_SENTINEL in crates/memory-mcp/src/control/static_assets.rs and the
# favicon href in crates/control-plane-ui/index.html.
```

- [ ] **Step 3: Build the image and verify the embedded contract**

```bash
docker build -t memory-mcp-http:relocatable .
```

Expected: exit 0; both the sentinel `grep` and the asset-count assertions pass.

- [ ] **Step 4: Commit**

```bash
git add Dockerfile
git commit -m "build: always ship a relocatable UI bundle (sentinel base, no build arg)"
```

---

### Task 9: Browser scenario — asset URLs must stay under the mount base

**Files:**
- Modify: `scripts/ci/local_admin_browser.mjs` (`scenarioUi`, after the stylesheet checks)

**Interfaces:**
- Consumes: `BASE_URL` (the script's existing `--base-url`; its pathname *is* the mount base for the check).
- Produces: a `ui` scenario that fails on the image-1.11.0 failure class (any asset request escaping the prefix) on root **and** prefixed deployments.

- [ ] **Step 1: Write the check (browser-scenario scripts assert via the existing `check` helper; this step is both failing-test and implementation — see Step 2 for the one run that matters)**

Insert at the end of `scenarioUi`, before its closing brace:

```js
  // Every asset URL in the served shell must live under the mount base. This
  // is the exact failure class of image 1.11.0: a bundle whose prefix was
  // rebuilt at root while the server ran under /memory — the page shell
  // returns 200 and every asset request escapes into the host root.
  const assetRefs = await page.evaluate(() => [
    ...[...document.querySelectorAll('link[href]')].map((el) => el.getAttribute('href')),
    ...[...document.querySelectorAll('script[src]')].map((el) => el.getAttribute('src')),
  ]);
  const basePath = new URL(BASE_URL).pathname.replace(/\/+$/, '');
  const escaped = assetRefs.filter(
    (ref) => ref && !ref.startsWith('data:') && !ref.startsWith(`${basePath}/`),
  );
  check('every asset url stays under the mount base', escaped.length === 0, escaped);
  check(
    'no unstamped base sentinel is served',
    assetRefs.every((ref) => !(ref ?? '').includes('__memory_mcp_base__')),
    assetRefs,
  );
```

- [ ] **Step 2: Run the scenario against a root deployment**

```bash
node scripts/ci/local_admin_browser.mjs --base-url https://localhost:8443 --scenario ui
```

Expected: all checks green (root base path is `""`, so every root-absolute asset URL passes `startsWith('/')`).

- [ ] **Step 3: Commit**

```bash
git add scripts/ci/local_admin_browser.mjs
git commit -m "ci: assert served asset URLs stay under the mount base"
```

---

### Task 10: Documentation — one value, stamped

**Files:**
- Modify: `README.md` (sections *Deploying under a path prefix* and *Control-plane UI asset packaging*, plus the `MEMORY_MCP_HTTP_PUBLIC_BASE_URL` row of the environment table)

**Interfaces:**
- Consumes: the amended spec (Task 1) and the implementation of Tasks 3–8.

- [ ] **Step 1: Rewrite the build/deploy contract paragraph**

In *Deploying under a path prefix*, replace the block beginning `Two values must agree:` and ending `rebuild the bundle when the prefix changes.` with:

```markdown
One value configures the prefix:

- runtime: `MEMORY_MCP_HTTP_PUBLIC_BASE_URL=https://mcp.example/memory`
  (the server answers only under `/memory`, `308`-canonicalizes
  `/memory/` → `/memory`, `404`s every root path, and stamps the mount base
  into the served UI shell at startup).

The UI bundle is **relocatable**: it is always built with the placeholder
`dx bundle --base-path /__memory_mcp_base__`, and `memory_mcp_http` replaces
that sentinel with the path of `MEMORY_MCP_HTTP_PUBLIC_BASE_URL` in the
embedded `index.html` (asset URLs and the `DIOXUS_ASSET_ROOT` meta) when the
router is assembled. One image therefore serves the origin root and any
prefix; changing the prefix is a configuration change and needs no rebuild. A
bundle without the sentinel is rejected at startup unless the deployment is at
the origin root.
```

- [ ] **Step 2: Rewrite the bundle build contract**

In *Control-plane UI asset packaging*, replace the command's `--base-path /memory` with `--base-path /__memory_mcp_base__` and replace the paragraph beginning `` `--base-path` may be omitted `` with:

```markdown
`--base-path` must always be the sentinel `/__memory_mcp_base__` — never a
deployment prefix. The bundle carries the sentinel in every prefix-dependent
URL and in its `DIOXUS_ASSET_ROOT` meta; `memory_mcp_http` replaces it with
the path of `MEMORY_MCP_HTTP_PUBLIC_BASE_URL` at startup (see *Deploying under
a path prefix*). The same literal lives in `crates/control-plane-ui/index.html`
and in `BASE_PATH_SENTINEL` (`crates/memory-mcp/src/control/static_assets.rs`);
changing it means changing all five code sites together (the two `BASE_PATH_SENTINEL` consts, the shell's favicon href, the `Dockerfile`, and the browser CI check).
```

- [ ] **Step 3: Update the environment table row**

In the `MEMORY_MCP_HTTP_PUBLIC_BASE_URL` row, after `Its **path** is also the mount base of the server`, replace `(e.g. https://mcp.example/memory); no path means the origin root` with `(e.g. https://mcp.example/memory); no path means the origin root. The path is stamped into the served UI shell at startup, so no UI rebuild is needed when it changes`.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: document the relocatable UI bundle and the runtime-stamped base"
```

---

### Task 11: Acceptance — one image, two mounts

**Files:** none (verification; fixes go to the owning task's files and that task's commit is amended or a follow-up commit is made there).

**Interfaces:**
- Consumes: everything above.

- [ ] **Step 1: Run the full gates**

```bash
cargo fmt --all && cargo fmt --all --check
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
cargo test -p memory_mcp
cargo test -p memory_mcp --features streamable-http
cargo test -p control-plane-ui
cargo check -p control-plane-ui --target wasm32-unknown-unknown
```

Expected: zero warnings, all suites green.

- [ ] **Step 2: Boot the compose stack under `/memory` with the freshly built image**

Build the tagged image and start the stack under the prefix (the compose file requires the operator secrets as usual; `ALLOWED_HOSTS` must cover the probe host — the host allowlist wraps the whole deployment boundary and answers before routing otherwise):

```bash
MEMORY_MCP_IMAGE=memory-mcp-http:relocatable docker compose build memory_mcp
MEMORY_MCP_IMAGE=memory-mcp-http:relocatable \
  MEMORY_MCP_HTTP_PUBLIC_BASE_URL=https://mcp.like-a-freedom.ru/memory \
  ALLOWED_HOSTS=127.0.0.1:8080 \
  ALLOWED_ORIGINS=https://mcp.like-a-freedom.ru \
  docker compose up -d
```

then run the incident's probe matrix:

```bash
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/memory/health/live        # 200
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/memory/api/v1/auth/config # 200
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/memory/mcp                # 405 (POST-only route — the GET probes the method guard, exactly as the incident matrix did)
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/memory/                    # 308
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/health/live                # 404
```

- [ ] **Step 3: Assert the stamped shell**

```bash
curl -s http://127.0.0.1:8080/memory | grep -o 'href="[^"]*"\|src="[^"]*"\|name="DIOXUS_ASSET_ROOT" content="[^"]*"'
```

Expected: every `href`/`src` starts with `/memory/`; the meta reads `content="/memory"`; the string `__memory_mcp_base__` appears nowhere. Then verify assets resolve:

```bash
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/memory/assets/favicon.svg   # 200
```

- [ ] **Step 4: Boot the same image at the origin root**

Restart the stack with `MEMORY_MCP_HTTP_PUBLIC_BASE_URL=https://mcp.like-a-freedom.ru` (no path). Expected: the root matrix is unchanged (`/health/live` 200, `/` serves the shell, `/api/...` unmatched = 404), the shell URLs are root-absolute exactly as today, and `curl -s http://127.0.0.1:8080/ | grep -c __memory_mcp_base__` prints `0`.

- [ ] **Step 5: Real browser under the prefix**

With the stack under `/memory` (TLS as configured for the host), run:

```bash
node scripts/ci/local_admin_browser.mjs --base-url https://mcp.like-a-freedom.ru/memory --scenario ui
```

Expected: all checks green — in particular `wasm app boots under the shipped csp`, `every asset url stays under the mount base`, `no unstamped base sentinel is served`, and the stylesheet checks.

- [ ] **Step 6: Report**

Summarize for the operator: the probe matrix, the stamped-shell output, and confirmation that both mounts ran the same image (`memory-mcp-http:relocatable`).

---

## Self-review

**Spec coverage (against the amended spec):** Decision 3' (relocatable bundle + stamping) → Tasks 3, 4, 5, 6, 7, 8; config surface (build arg deleted, sentinel build contract) → Tasks 8, 10; compatibility (root byte-compatible, one image any prefix) → Tasks 4 (root no-op path), 11 Steps 2–4; non-goal removal recorded → Task 1. Nothing outside the amended Decision 3 is touched: `Router::nest` mounting, cookies, OIDC redirect, canonicalization and the `derive_base_path` grammar are existing behavior and stay as-is.

**Placeholder scan:** every code step carries the actual code or an exact old→new splice; the Task 2 outcome that selects Task 5's variant is resolved by a recorded probe (Variant A and Variant B are both written out); the two STOP conditions say exactly what to report, not "handle it".

**Type consistency:** `stamp_index_html(raw: &[u8], base: &str) -> Result<Vec<u8>, MemoryError>` (Task 3 = Task 4 consume); `build_stamped_index(base: &str) -> Result<Option<Arc<[u8]>>, MemoryError>` and `serve_asset(path: &str, stamped_index: Option<&[u8]>)` (Task 4, `Arc<[u8]>::as_deref()` feeds `Option<&[u8]>`); `resolve_base(meta_content: Option<String>, baked: Option<String>) -> String` and `base_path() -> String` (Task 6 = Task 7 consume). `BASE_PATH_SENTINEL` is spelled `/__memory_mcp_base__` in all five code sites (server const, UI const, UI index.html, Dockerfile `dx` invocation + `grep`, browser-script check).
