# Local Administrator Authentication — Operations Runbook

Covers the local (non-OIDC) browser control plane: administrator accounts,
password lifecycle, durable sessions, client provisioning, client API keys, and
the deployment configuration that selects it.

Every claim below was checked against the code on branch `local-admin`
(`crates/memory-mcp/src/{control/local_admin,service/local_admin,http/config,http/registry/surreal_store,cli}`),
and every command is labelled with whether it was actually executed. Where the
implementation does not match the approved design, the divergence is stated
rather than smoothed over.

Evidence in this revision was collected against the real binaries and a real
RocksDB-backed registry (`scripts/ci/local_admin_local_check.sh`) and against
the built image over trusted TLS in a real browser
(`scripts/ci/local_admin_image.py --scenario all`).

## 1. Scope and status

### What this document covers

- Local mode selection and the environment contract (`MEMORY_MCP_HTTP_AUTH_MODE=local`).
- The CLI `admin` subcommand (`create`, `recover`) and its environment.
- The local browser/API surface mounted at `/api/v1/auth/local/*` and `/api/v1/admin/*`.
- Durable records, sessions, challenges, throttling, audit, and the
  `browser_auth_policy` mode fence.
- Client creation, key issuance/revocation, suspend/resume.
- Backups, restore, rotation, rollback, and explicit non-goals.

### What is verified by automated tests

All of the following were executed in this repository and passed:

| Layer | Evidence |
|---|---|
| Service + durable registry (in-memory SurrealDB) | `cargo test -p memory_mcp --lib --features control-plane,test-fixtures --locked` — 1772 passed, 3 ignored |
| Service security experiments against the **real** store | `... --lib ... service::local_admin` — 31 passed (of which 13 are the plan §5 experiments; see §13.3 for the per-case mapping) |
| HTTP surface (real router, real pre-auth cookie, CSRF, Origin, session, SQL transactions, migrated in-memory registry) | `cargo test -p memory_mcp --features control-plane,test-fixtures --locked --test http_local_admin` — 42 passed |
| OIDC regression | `... --test http_control_plane` — 11 passed |
| CLI contract (clap surface + real subprocess against file-backed RocksDB) | `... --test local_admin_cli` — 7 passed |
| Durable query-shape regression | `... --test registry_query_shape` — 4 passed |
| Provisioning crash recovery | `... --test http_crash_recovery` — 11 passed |
| Whole conformance suite | `cargo test -p memory_mcp --features control-plane,test-fixtures --locked` and `cargo test -p memory_mcp --locked` — every target green |
| UI crate | `cargo test -p control-plane-ui --locked` — 53 passed; `cargo check -p control-plane-ui --locked` clean |
| Static assets + CSP | `MEMORY_MCP_CONTROL_PLANE_UI_DIST=<abs dist> cargo test -p memory_mcp --lib --features control-plane-ui,test-fixtures --locked control::static_assets` — 6 passed |
| Format, lint, non-default builds | `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --features … --locked -- -D warnings` for all four documented feature combinations; `cargo check -p memory_mcp --no-default-features --locked` and `--features streamable-http` |
| End-to-end against the real binaries | `sh scripts/ci/local_admin_local_check.sh` — local mode starts from env, activation/login/session, client create/list/get, provisioning reaches `ready` in one poll, restart persistence, plan-mismatch startup rejection, recovery invalidates the session, negative route/CSRF/Origin checks |
| Packaged image over real TLS in a real browser | `docker build --tag memory-mcp-local-admin:test .` then `python3 scripts/ci/local_admin_image.py --image memory-mcp-local-admin:test --scenario all` — 4 scenarios, 54 checks, exit 0 (`auth` 9, `clients` 29, `regression` 6, `ui` 10) |
| Compose modes resolve | `docker compose --env-file <operator env> -f docker-compose.yml -f docker-compose.{off,local,oidc}.yml config --quiet` — all three resolve |

### What is NOT verified

1. **The remote replica race tests.** Both `local_admin_remote.rs` tests
   (`local_admin_remote_replica_races`, `local_admin_session_revocation_race`)
   are `#[ignore]`d and require an isolated remote SurrealDB 3.2.4 plus the
   three `LOCAL_ADMIN_TEST_CONTROL_*` variables (§2.2, §13.2). They now live
   inside the crate as inline adapter tests, so the documented
   `cargo test -p memory_mcp --lib … -- --ignored` command selects them, and
   explicitly selecting them without the environment **fails** rather than
   skipping.
2. **A real identity provider.** OIDC mode is exercised at store and router
   level, but no OIDC login was performed against a live IdP in this
   environment.
3. **`dx` on the host.** The Dioxus CLI is absent from the host, so
   `dx bundle` was only run inside the Docker image (pinned 0.7.10). A
   host-side UI build is unverified; the image path is the verified one.
4. **The amd64 image path.** The verification host is `arm64`; Compose pins
   `linux/amd64` in CI, which was not reproduced locally.
5. **Forced-order interleavings.** The plan's §5 cases that require pausing a
   request between two statements (KDF-pause + recovery, prepare-reset + newer
   recovery, both resolve/logout commit orders, two rotations racing) are
   covered as *outcomes*, not as interleavings: there are no barrier-driven
   tests that order two in-flight transactions (§13.3).
6. **Database unavailability mid-transaction.** Storage-outage behaviour
   (sanitized `503` with no fallback) is proven with the test-only SQL fault
   hook (`sql_fault_tests` in the durable store), which fails a *named*
   statement before it reaches the engine. A real database process is not
   killed mid-request.
7. **Sub-second rate-window boundaries across replicas.** The durable cap and
   its saturation are tested; two independent replicas sharing live counters
   were not (no second server process against a shared registry).
8. **Second-administrator scoping.** ✅ now implemented and covered:
   `http_local_admin.rs::a_second_administrator_sees_and_can_administer_the_same_clients`
   (§6.3). The remaining remote-replica *interleavings* are still unverified
   (§13.2).

### Relationship to other documents

This file is the evidence-based runbook the plan's §2 inventory named
(`docs/operations/LOCAL_ADMIN.md`, "T10 evidence-based runbook after
implementation").

A pre-implementation draft occupied this path before the work landed. That draft
was deleted rather than left in place, because every one of its operational
statements was wrong and a reader could not tell: `memory_mcp admin create ops.one`
(positional; the real CLI requires `--username`), the route
`/api/v1/local/admin/challenge` (the real route is `POST /api/v1/auth/local/challenge`),
`SameSite=Lax` (the real cookies are `SameSite=Strict`), "printable ASCII"
passwords (the real rule is 15–128 Unicode scalar values with no NUL), and
`SURREALDB_URL`/`SURREALDB_DB_NAME` for the HTTP profile (the HTTP profile reads
`SURREALDB_CONTROL_*` and `SURREALDB_TENANT_*`). The draft's full text is
recoverable from git history (commits `14f2b53`, `893e01d`); the list above is
the part worth keeping, so nobody reintroduces those claims.

## 2. Prerequisites and exact commands

### 2.1 Tool versions observed

| Tool | Version observed | Notes |
|---|---|---|
| `rustc` / `cargo` | `1.97.1` | Matches `rust-toolchain.toml` |
| Docker | `29.4.0` (`docker compose` available) | `docker build` and `docker compose config` were both run |
| Node.js | `v22.22.3` | Runs the browser harness with the pinned runner package |
| Python | `3.14.7` | Runs the image harness |
| `playwright` (npm) | `1.63.0` | Pinned in `scripts/ci/package.json` + `package-lock.json`; installed under `scripts/ci/node_modules` |
| `dx` (Dioxus CLI) | **absent on the host** | Only the image builds the bundle, via `dioxus-cli 0.7.10` pinned as `DIOXUS_CLI_VERSION` in `Dockerfile` |
| `surreal` CLI | **absent** | Not needed: the harness runs the `surrealdb/surrealdb:v3.2.4` container |

Key material is operator-generated. Placeholders below are obviously fake and
must be replaced:

```bash
openssl rand -hex 32   # one value per 32-byte hex key variable
```

### 2.2 Execution validation commands (plan §6) and their status here

Run from the repository root.

| Command | Status in this session |
|---|---|
| `cargo fmt --all --check` | ✅ executed, clean |
| `cargo check -p memory_mcp --no-default-features --locked` | ✅ executed, clean |
| `cargo check -p memory_mcp --no-default-features --features streamable-http --locked` | ✅ executed, clean |
| `cargo test -p memory_mcp --locked` | ✅ executed, every target green |
| `cargo test -p memory_mcp --lib --features control-plane,test-fixtures --locked service::local_admin` | ✅ executed, 31 passed |
| `cargo test -p memory_mcp --features control-plane,test-fixtures --locked --test http_control_plane` | ✅ executed, 11 passed |
| `cargo test -p memory_mcp --features control-plane,test-fixtures --locked --test http_local_admin` | ✅ executed, 42 passed |
| `cargo test -p memory_mcp --features control-plane,test-fixtures --locked --test local_admin_cli` | ✅ executed, 7 passed |
| `cargo test -p memory_mcp --features control-plane,test-fixtures --locked --test local_admin_durable` | ❌ superseded: the target was not created (§13.4). Its two remote-race cases moved inline into `surreal_store/local_admin_remote.rs`, so the `-- --ignored` row below replaces it. This row is the only plan §6 command that no longer exists verbatim |
| `cargo test -p memory_mcp --features control-plane,test-fixtures --locked` | ✅ executed, every target green |
| `cargo test -p control-plane-ui --locked` | ✅ executed, 53 passed |
| `cargo check -p control-plane-ui --locked` | ✅ executed, clean (native check only; proves nothing about WASM) |
| `cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http,control-plane --locked -- -D warnings` | ✅ executed, clean |
| `cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http,control-plane,test-fixtures --locked -- -D warnings` | ✅ executed, clean |
| `cargo clippy ... --features ...,control-plane-ui,test-fixtures ...` | ✅ executed, clean (with `MEMORY_MCP_CONTROL_PLANE_UI_DIST` set) |
| `cargo test -p memory_mcp --lib --features control-plane,test-fixtures --locked local_admin_remote_replica_races -- --ignored` | ⚠️ not executed (needs an isolated remote SurrealDB 3.2.4 and the three `LOCAL_ADMIN_TEST_CONTROL_*` variables); the test is now an inline adapter test, so the command selects it, and running it without the variables **fails** rather than skipping |
| `MEMORY_MCP_CONTROL_PLANE_UI_DIST=<abs dist> cargo test -p memory_mcp --lib --features control-plane-ui,test-fixtures --locked control::static_assets` | ✅ executed, 6 passed — the build script requires an absolute, non-symlink dist directory containing a non-empty `index.html` |
| `MEMORY_MCP_CONTROL_PLANE_UI_DIST=<abs dist> cargo test -p memory_mcp --features control-plane-ui,test-fixtures --locked --test http_local_admin` | ✅ executed, 42 passed |
| `docker compose --env-file <operator env> -f docker-compose.yml -f docker-compose.{off,local,oidc}.yml config --quiet` | ✅ executed, all three overlays resolve. The base file alone still fails by design: it refuses to default any secret |
| `docker build --tag memory-mcp-local-admin:test .` | ✅ executed, `25/25 FINISHED` in ~338 s cold / ~321 s warm |
| `python3 scripts/ci/local_admin_image.py --image memory-mcp-local-admin:test --scenario all` | ✅ executed, 4 scenarios / 54 checks, exit 0 |
| `node scripts/ci/local_admin_browser.mjs --base-url https://localhost:8443 --scenario auth` | ✅ executed through the harness. Direct invocation requires `LOCAL_ADMIN_BROWSER_FIXTURE`; a bare URL is refused |
| `sh scripts/ci/local_admin_local_check.sh` (authorisation-code end-to-end against the real binaries and a real RocksDB registry) | ✅ executed, all sections pass (§2.5) |

Two additional checks were run for the CLI contract:

```bash
# No-default-features build must not expose `admin` (plan Task 5).
cargo run -p memory_mcp --no-default-features --locked --bin memory_mcp -- --help
```

✅ executed: the printed command list contains no `admin` subcommand.
`tests/local_admin_cli.rs::admin_variant_is_reachable_in_this_build` asserts the
positive case for a `control-plane` build.

### 2.3 The in-repo acceptance harnesses

The plan originally listed both harnesses as release blockers because the
pinned browser runner and its fixtures did not exist. Both are now real and
were executed; neither can pass without testing something.

- `scripts/ci/local_admin_image.py --image <tag> --scenario {all|auth,clients,regression,ui}`
  is a black-box Docker orchestrator. It validates prerequisites (docker daemon,
  image, node, `openssl`, the pinned runner package, and two in-image probes),
  then creates a disposable network, a `surrealdb/surrealdb:v3.2.4` container, a
  0600 env file, a self-signed CA and a `caddy:2` TLS terminator on
  `https://localhost:8443`. It issues a code through the **source-built** CLI in
  the image, runs the browser scenarios, and tears everything down in a `finally`
  block. Every prerequisite failure raises; there is no warn-and-continue branch
  and no path that exits `0` without running the requested scenarios.
- `scripts/ci/local_admin_browser.mjs --base-url https://localhost:8443 --scenario auth`
  drives Chromium through the pinned package in `scripts/ci/package.json`
  (`playwright 1.63.0`, locked in `package-lock.json`). It refuses to run without
  `LOCAL_ADMIN_BROWSER_FIXTURE`, which the image harness writes and which
  identifies the disposable CLI, the TLS trust and the base URL — a bare URL is
  not sufficient because the runner must mint its own codes. Secrets are
  registered in memory and redacted from every diagnostic.

Executed result: `auth` 9, `clients` 29, `regression` 6, `ui` 10 — 54 checks,
exit 0. The `ui` scenario loads the real bundle in a page and asserts it mounts.

### 2.4 Feature/runtime matrix

| Configuration | Status |
|---|---|
| Compiles with `streamable-http,control-plane,test-fixtures` | ✅ verified (all local-admin suites) |
| `admin` subcommand requires `streamable-http` + `control-plane` | ✅ verified (`cli.rs` gates `Command::Admin`; the `--no-default-features` help output has no `admin`, and the gated build adds exactly that one command to the frozen CLI snapshot) |
| Local routes require `control-plane`; UI requires `control-plane-ui` **and** a built bundle | ✅ verified for both: the image builds the bundle and serves it, and the UI-disabled router answers `/api/*` with JSON 404 |
| `control-plane-ui` + `test-fixtures` builds | ✅ verified with `MEMORY_MCP_CONTROL_PLANE_UI_DIST` set (build script requires it) |
| Default stdio build unchanged / no `admin` | ✅ verified via `--help` |
| Local mode from environment variables | ✅ verified end-to-end against the real binary (§2.5) |
| Two local replicas sharing durable auth/throttle/caps | ❌ not demonstrated (no multi-process test against a remote registry) |
| Mixed local/OIDC replicas rejected | ✅ both directions: `join_local_policy` refuses a non-local policy and `join_oidc_policy` refuses an existing local policy at startup; a session row from another epoch never resolves |

### 2.5 End-to-end evidence against the real binaries

```bash
sh scripts/ci/local_admin_local_check.sh
```

This script builds `memory_mcp` and `memory_mcp_http` with
`fs-watch,mcp-apps,streamable-http,control-plane` (deliberately **without**
`test-fixtures`, which would put the HTTP binary into bootstrap mode), then runs
the full administrator lifecycle against a real RocksDB control registry under
`/tmp/lmcp_local_check`:

1. `admin create --username ops.one` with the server stopped, printing one JSON
   object whose `activation_url` carries no code.
2. Start the server on the same registry; `GET /api/v1/auth/config` →
   `{"mode":"local"}`.
3. `GET /api/v1/auth/local/csrf` → pre-auth cookie + token; `POST
   /api/v1/auth/local/activate` → `204`.
4. `POST /api/v1/auth/local/login` → `204` with a `__Host-memory_mcp_admin`
   cookie and a cleared pre-auth cookie.
5. `GET /api/v1/admin/session` → admin id, username, auth time, absolute expiry,
   CSRF token.
6. `POST /api/v1/admin/clients` → `202` plus a `Location` header; the sidecar
   reports `tenant_status: reserved` as the create transaction wrote it.
7. `GET /api/v1/admin/clients` and `/{account_id}`. Section 7b polls until the
   tenant is `ready`; this now succeeds on the **first** poll because the view
   reads the live `tenant` row rather than the create-time snapshot.
7. Section 7c issues a client key through the admin API and uses it against
   `POST /mcp`: no credential → `401`, the issued key → `200` with the tool
   list, the same key after revocation → `401`. This is the end-to-end proof
   that a locally issued key is a real data-plane credential (§6.5).
8. Negative checks: no CSRF → `403`, wrong `Origin` → `403`, OIDC route → `404`,
   operator route → `404`, unmatched `/api/*` → JSON `404`.
9. Restart the server: the session still resolves and the client list survives,
   now reporting the live `ready`/`schema_version`.
10. Start with `MEMORY_MCP_HTTP_MAX_ACTIVE_API_KEYS=99`: startup fails with
    `plan_limit_mismatch`, so a drifted plan can never be silently adopted.
11. `admin recover`, then the old session → `401`.
12. Log evidence that the provisioning worker actually created the tenant
    namespace (`op=schema.init namespace=tns_…`).

`LMCP_CHECK_RESET=0` keeps the previous registry instead of starting clean.

### 2.6 Packaged UI and browser boot

```bash
docker build --tag memory-mcp-local-admin:test .
python3 scripts/ci/local_admin_image.py --image memory-mcp-local-admin:test --scenario all
```

The image is three stages: `dx bundle --platform web --release --package
control-plane-ui --out-dir /src/control-plane-ui-dist` (pinned `dioxus-cli
0.7.10`), then both binaries with
`streamable-http,control-plane,control-plane-ui` and
`MEMORY_MCP_CONTROL_PLANE_UI_DIST=/src/control-plane-ui-dist/public`, then a
`distroless/cc-debian13:nonroot` runtime.

**The shipped CSP needs `'wasm-unsafe-eval'`.** `script-src 'self'` alone makes
Chromium refuse `WebAssembly.instantiateStreaming`, so the SPA never mounted:

```
WebAssembly.instantiateStreaming(): Compiling or instantiating WebAssembly module
violates the following Content Security policy directive because 'unsafe-eval' is
not an allowed source of script … "script-src 'self'"
```

`control::static_assets::CONTENT_SECURITY_POLICY` now carries
`script-src 'self' 'wasm-unsafe-eval'`. That token permits WebAssembly
compilation only; JavaScript `eval`, `new Function` and inline script stay
blocked, and the unit test asserts exactly that (present `'wasm-unsafe-eval'`,
absent `'unsafe-eval'` and `'unsafe-inline'`). The policy literal and its
test share one constant, so the two cannot drift.

The `ui` scenario is what proves it: it loads `/admin/login` in a real page and
waits for the rendered `#admin-username` field, then asserts there are no CSP
violations, no uncaught errors, no external assets, and that the served header
still refuses general `eval` and inline script. Scenario totals: `auth` 9,
`clients` 32, `regression` 6, `ui` 7.

## 3. Deployment configuration

### 3.1 Environment variables read for local mode

Loader: `crates/memory-mcp/src/http/config/types.rs` (`HttpConfig::from_env`),
validator: `crates/memory-mcp/src/http/config/validate.rs`.

Always required by the HTTP profile (all modes; `require_env` / `parse_hex_32_env`):

| Variable | Notes |
|---|---|
| `MEMORY_MCP_HTTP_PUBLIC_BASE_URL` | Must start with `https://` in local mode, **or** be loopback (`localhost`, `127.0.0.1`, `[::1]`) as a development escape hatch. The exception keys off the URL *host*, so `http://evil.example/?localhost` is rejected |
| `ALLOWED_HOSTS` | Comma-separated; empty is rejected |
| `ALLOWED_ORIGINS` | Comma-separated; empty is rejected, `*` rejected. Every local POST/DELETE requires a present, single, exactly-matching `Origin` |
| `MEMORY_MCP_API_KEY_PEPPER` | ≥ 32 bytes; used to verify client keys |
| `MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY` | 64-char hex. **In local mode it is derived** from the session key under a purpose label, and supplying one **fails startup** (OIDC-only configuration must not be silently ignored). OIDC mode requires it |
| `MEMORY_MCP_HTTP_SESSION_KEY` | 64-char hex; local session/policy key (see §9.1 for its limited role) |
| `MEMORY_MCP_HTTP_OIDC_STATE_KEY` | 64-char hex, same rule as the identity-index key: derived in local mode, supplying one fails startup; OIDC mode requires it |
| `MEMORY_MCP_HTTP_OIDC_NONCE_KEY` | 64-char hex, same rule as the identity-index key: derived in local mode, supplying one fails startup; OIDC mode requires it |
| `MEMORY_MCP_HTTP_CSRF_KEY` | 64-char hex; local CSRF key |
| `MEMORY_MCP_HTTP_SIGNUP_MODE` | `invite_only` or `open`. In local mode it defaults to `invite_only` and an explicit `open` is **rejected at startup**, because local mode has no identity provider |
| `SURREALDB_CONTROL_{URL,USERNAME,PASSWORD,DB,NAMESPACE}` | Control registry (all local-admin records live here) |
| `SURREALDB_TENANT_{URL,USERNAME,PASSWORD,DB,NAMESPACE}` | Tenant storage; must not share the registry namespace/database binding |

Local-mode-specific:

| Variable | Notes |
|---|---|
| `MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE=true` | Required; without it no browser auth is configured at all |
| `MEMORY_MCP_HTTP_AUTH_MODE=local` | Explicitly set it; the default is `oidc`. Any other value fails startup |
| `MEMORY_MCP_HTTP_LOCAL_DEFAULT_PLAN_VERSION` | Required positive `u32`; never taken from browser input |
| See §3.3 | All seven plan-limit variables are required |

OIDC-only values must be **empty** in local mode. The loader fails with
`local mode must not have OIDC configuration` when any of these is set:
`MEMORY_MCP_HTTP_OIDC_ISSUER`, `MEMORY_MCP_HTTP_OIDC_CLIENT_ID`,
`MEMORY_MCP_HTTP_OIDC_AUDIENCE`, `MEMORY_MCP_HTTP_OIDC_REDIRECT_URI`,
`MEMORY_MCP_HTTP_OIDC_ALLOWED_ALG` (non-default), `MEMORY_MCP_HTTP_OPERATOR_IDENTITIES`.

Note the asymmetry: the OIDC *values* are rejected, but the three OIDC-only HMAC
keys and `MEMORY_MCP_HTTP_SIGNUP_MODE` are still unconditionally required. Do not
"clean up" a local Compose profile by deleting those four variables — startup
fails with `config missing: MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY` (verified).

Optional in local mode: `MEMORY_MCP_HTTP_BIND`, `MEMORY_MCP_HTTP_BODY_LIMIT`,
`MEMORY_MCP_HTTP_REQUEST_DEADLINE_SECS` (the local deadline; exhaustion returns
`503 temporarily_unavailable` with `Retry-After: 1`), the pool/runtime limits,
`MEMORY_MCP_HTTP_TRUSTED_PROXY_CIDRS`, `MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI`.

### 3.2 HTTPS and cookies

Local browser auth runs over TLS. The `__Host-` cookie prefix requires `Secure`,
`Path=/`, and no `Domain`, which browsers only accept over HTTPS (localhost is
the usual exception). Cookie attributes issued by
`crates/memory-mcp/src/control/local_admin/handlers.rs` and
`.../csrf.rs`:

| Cookie | Attributes |
|---|---|
| `__Host-memory_mcp_admin_preauth` | `Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=300` |
| `__Host-memory_mcp_admin` | `Path=/; Secure; HttpOnly; SameSite=Strict` (no `Max-Age` on login; `Max-Age=<remaining absolute lifetime>` on reauth; `Max-Age=0` on logout) |

There is no insecure-cookie switch. TLS may terminate at a trusted proxy, but
the proxy must overwrite `Host` and must not expose the private listener
directly. A request that presents the same cookie name twice is rejected
outright (`403`) rather than resolved by "first wins".

### 3.3 The seven plan limits and the default plan

All seven are read by `load_signup_plan_limits`; if any is set, all must be set,
otherwise startup fails. Local mode requires them explicitly — there is no
fallback to `PlanLimits::default()`.

| Variable |
|---|
| `MEMORY_MCP_HTTP_MAX_INGESTED_BYTES` |
| `MEMORY_MCP_HTTP_MAX_EPISODE_COUNT` |
| `MEMORY_MCP_HTTP_INGEST_PER_MINUTE` |
| `MEMORY_MCP_HTTP_MAX_OPEN_APP_SESSIONS` |
| `MEMORY_MCP_HTTP_MAX_ACTIVE_API_KEYS` |
| `MEMORY_MCP_HTTP_PER_TENANT_REQUEST_CONCURRENCY` |
| `MEMORY_MCP_HTTP_EXTRACTION_CONCURRENCY` |

At startup (`HttpState::assemble`, `crates/memory-mcp/src/http.rs`) the server
ensures a plan with version `MEMORY_MCP_HTTP_LOCAL_DEFAULT_PLAN_VERSION` and
exactly these limits (`ensure_local_plan`). Lookup is by globally unique version:
a stored plan at that version with a different limit set fails startup with
`plan_limit_mismatch`. Because the same allocation also always ensures the
version-1 `free` plan, a deployment using `MEMORY_MCP_HTTP_LOCAL_DEFAULT_PLAN_VERSION=1`
must keep the two limit sets identical (they come from the same variables, so
this holds unless the registry already contains a divergent `free` row).
`MEMORY_MCP_HTTP_MAX_ACTIVE_API_KEYS` is the per-client key cap enforced at
issuance (§6).

### 3.4 Local mode starts from environment variables (previously blocked)

This was a real, reproduced startup failure and it is now fixed. The validator
no longer demands OIDC completeness whenever the control plane is enabled: OIDC
validation is gated on `browser_auth_is_oidc()`, so local and off modes mount
neither OIDC routes nor OIDC configuration requirements.

Verified end to end with the real binary and a real RocksDB-backed registry:

```bash
sh scripts/ci/local_admin_local_check.sh
```

Section 2 of that script starts `memory_mcp_http` from the environment file in
`/tmp/lmcp_local_check/env` (local mode, HTTPS public URL, the seven plan
limits, both browser keys) and `GET /api/v1/auth/config` answers
`{"mode":"local"}`. Sections 3–11 then exercise activation, login, session,
client create/list/get, restart persistence, the plan-mismatch startup refusal,
and recovery. §3.1 and §3.3 are therefore a working deployment recipe, not an
intention.

Two related behaviours are enforced in the same path:

- A local replica rejects any OIDC provider setting by name
  (`local mode must not have OIDC configuration`).
- `MEMORY_MCP_HTTP_SIGNUP_MODE=open` is rejected in local mode; omitting it
  selects `invite_only`.

### 3.5 Browser-auth policy record and key material

`LocalBrowserConfig` holds `session_key`, `csrf_key`, `default_plan_version`, and
`default_plan_limits`. The two keys must be 32 bytes of operator-generated hex;
there is no zero-key fallback.

On startup a local replica joins the singleton `browser_auth_policy` row. The
first joiner creates it with `mode='local'`, `epoch=1`, and hex fingerprints
derived from both keys (domain-separated HMACs, not the keys themselves). A
replica whose mode or key fingerprints differ is rejected at startup.

The fence is now wired in **both** directions:

- Local mode joins with `join_local_policy`, which refuses an existing
  OIDC-owned row.
- OIDC mode joins with `join_oidc_policy` at startup
  (`HttpState::assemble_with_browser_policy`), which refuses an existing
  local-owned row. `create_oidc_account_bundle` checks the same fence before
  writing, and the six OIDC-only store operations take the policy as a
  parameter.

migration 047 adds the `browser_policy_epoch` column to
`control_plane_session`, and `resolve_session_record` rejects any session whose
epoch is missing or stale. **This is a deliberate compatibility break:**
OIDC sessions created before the upgrade do not carry an epoch and therefore
force one fresh login per browser. Data-plane API keys are unaffected.

## 4. First administrator: create and activate

### 4.1 CLI contract

`crates/memory-mcp/src/cli/commands/admin.rs`, `.../cli/admin_config.rs`, and
`.../cli/args.rs`. The subcommand exists only in builds compiled with both
`streamable-http` and `control-plane`.

Environment read by `AdminCliConfig::from_env` — and nothing else:

| Variable | Required |
|---|---|
| `MEMORY_MCP_HTTP_AUTH_MODE` | yes, and it must be exactly `local` (spec §6) |
| `MEMORY_MCP_HTTP_SESSION_KEY` (64-hex) | yes |
| `MEMORY_MCP_HTTP_CSRF_KEY` (64-hex) | yes |
| `SURREALDB_CONTROL_URL` | yes |
| `SURREALDB_CONTROL_USERNAME` | yes |
| `SURREALDB_CONTROL_PASSWORD` | yes |
| `SURREALDB_CONTROL_DB` | yes |
| `SURREALDB_CONTROL_NAMESPACE` | yes |
| `MEMORY_MCP_HTTP_PUBLIC_BASE_URL` | no (defaults to `https://localhost`) |

The CLI **requires local mode**: a missing or non-`local`
`MEMORY_MCP_HTTP_AUTH_MODE` fails before any connection is opened, because
creating a local administrator in a deployment that authenticates browsers
through an identity provider would write records nothing can use
(`tests/local_admin_cli.rs::admin_commands_require_local_mode` asserts the
refusal for absent, `oidc` and `off`).

Beyond that the CLI does not read the plan limits, the tenant
DB, or any model/OIDC setting; it connects only to the control registry. It also
does not start tenant provisioning, a runtime pool, embeddings, or OIDC
discovery — `tests/local_admin_cli.rs` runs the real binary with `env_clear()`
plus the variables above and asserts that stderr mentions none of
`oidc`/`discovery`/`model_not_ready`.

Create:

```bash
memory_mcp admin create --username ops.one
```

Exact response shape on stdout (one pretty-printed JSON object; keys are pinned
by `tests/local_admin_cli.rs`):

```json
{
  "admin_id": "adm_<32 lowercase hex>",
  "username": "ops.one",
  "code": "<64 lowercase hex>",
  "expires_at": "<RFC 3339>",
  "activation_url": "https://<public-base-url>/admin/activate"
}
```

- `activation_url` is a **fixed** path and never embeds the code; the operator
  must convey the code separately.
- The code is 15-minute material (see §4.3) delivered exactly once on stdout.
  It is never echoed to stderr and must not be captured into CI artifacts. If it
  is lost, run `admin recover`.
- The durable row holds only an **HMAC verifier** derived from the code under
  the session key with a fixed purpose label (`challenge_verifier`). The raw
  code is never written to `local_admin_challenge`, so reading the registry — or
  restoring a backup of it — cannot redeem a live activation or reset code
  inside its 15-minute window. The derivation and migration `047` shipped
  together, so no deployment holds a pre-derivation row.
- Running `create` twice for the same username does not mint a second code: the
  durable transaction rejects a repeated `activate` issue for a non-pending
  administrator (`admin_already_exists` → conflict). To replace lost or expired
  material, use `recover`.
- The record created is `state = pending_activation`,
  `credential_generation = 1`, and an audit row with `actor_kind='cli'`,
  `actor_id='local_admin_cli'`.

Username and password rules (`service/local_admin/policy.rs`):

| Field | Rule |
|---|---|
| Username | ASCII, `[a-z0-9][a-z0-9._-]{2,63}` (3–64 chars). Outer ASCII whitespace trimmed and ASCII case-folded to lowercase before validation. No Unicode lookalikes or aliases |
| Password | 15–128 Unicode scalar values, at most 1024 UTF-8 bytes, no NUL. Not trimmed, not normalized, no character-class rules; paste and password managers are fine |

The server stores only an Argon2id PHC hash: version 19, `m=19456 KiB, t=2, p=1`,
32-byte output, independent 16-byte random salt per hash. Hashing runs on
`spawn_blocking` with a per-process semaphore of 2 running jobs, at most 8
queued, and a 2-second admission timeout (exhaustion → `503`). Stored PHC
parameters are validated against supported bounds before any expensive work;
corrupt or foreign hashes fail closed as invalid credentials.

### 4.2 Activation over the browser

The activation page is the UI route `/admin/activate` (the CLI prints it). Note
that this route is only served when the binary is built with `control-plane-ui`
**and** `MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI=true`; otherwise it is an empty
`404` (`/api/` and `/auth/` paths always return JSON `404` instead). The API
sequence the page drives, in order (all verified at the router level by
`activation_login_session_roundtrip`):

1. `GET /api/v1/auth/local/csrf` → `200 {"csrf_token": "<hex>"}` and sets the
   pre-auth cookie. The pre-auth cookie carries no identity.
2. `POST /api/v1/auth/local/challenge` with `{"code": "...", "kind": "activate"}`
   → `200 {"username": "ops.one", "expires_at": "<RFC 3339>"}`. This validates
   without consuming. Every invalid, expired, used, wrong-kind, or malformed code
   returns the identical `400 invalid_challenge`. The `activate`/`reset` request
   DTOs accept only `code` and `password` (unknown fields are rejected), so a
   browser cannot substitute a different username — the reserved name is
   server-side state.
3. `POST /api/v1/auth/local/activate` with `{"code": "...", "password": "..."}`
   → `204`, **no session**. Atomically: the challenge is consumed, siblings and
   all of that administrator's sessions are revoked, the password hash is set,
   the state becomes `active`, and `credential_generation` increments. Two
   concurrent submissions yield exactly one success.
4. Login: `POST /api/v1/auth/local/login` with `{"username": "...", "password": "..."}`
   → `204` plus the session cookie; the pre-auth cookie is cleared in the same
   response.

Every public POST requires all three of: a present `Origin` exactly matching
`ALLOWED_ORIGINS`, the pre-auth cookie, and the matching `X-CSRF-Token` header.
`Content-Type` must be `application/json` (parameters such as `; charset=utf-8`
are accepted); any other type is `415 unsupported_media_type`. Bodies are capped
at 16 KiB (`413 payload_too_large` beyond that). Malformed JSON is
`400 bad_request`. Unknown JSON fields are rejected (strict DTOs). Each local
request is bounded by `MEMORY_MCP_HTTP_REQUEST_DEADLINE_SECS` via a local-only
deadline middleware.

Login failure is uniform: unknown user, pending user, recovery-required user, and
wrong password all get the same one dummy Argon2id verification followed by
`401 invalid_credentials`. The authentication state is never disclosed.

### 4.3 Session lifetime

`resolve_session` and the mutation guard (`surreal_store/local_admin.rs`) use
database time:

| Property | Value |
|---|---|
| Idle expiry | 30 minutes, refreshed on each resolved request, clamped to `min(now + 30 min, absolute expiry)` |
| Absolute expiry | 24 hours, never extended by reauth (reauth preserves the original deadline) |
| Challenge (activation/reset code) | 15 minutes, not renewable by the browser |
| Recent-auth window | 600 seconds; credential-affecting and client-mutating requests older than this get `403 reauth_required` |
| Pre-auth CSRF material | 5 minutes |

A revoked or expired session is never recreated or extended.

Reauthentication (`POST /api/v1/admin/reauth`, body `{"password": "..."}`) verifies
the current password, revokes the presented cookie, and inserts a new one inside
one transaction, preserving the absolute deadline. It does not require
already-recent auth (otherwise a stale session could never be repaired). It does
not change the password. Concurrent rotations have one winner; if logout commits
first, rotation fails.

Logout (`POST /api/v1/admin/logout`) requires `Origin`; a request presenting a
valid session must also carry the session CSRF token, otherwise `403`. A request
with no session (or with an expired/revoked cookie) returns `204` and clears the
cookie without touching any other session.

Session CSRF tokens bind `(admin_id, session_id, policy epoch)`; pre-auth tokens
bind the cookie value and the epoch. A change of mode or keys therefore
invalidates every outstanding token.

## 5. Recovery

```bash
memory_mcp admin recover --username ops.one
```

Exact response shape (again one JSON object; `reset_url` replaces
`activation_url`):

```json
{
  "admin_id": "adm_...",
  "username": "ops.one",
  "code": "<64 lowercase hex>",
  "expires_at": "<RFC 3339>",
  "reset_url": "https://<public-base-url>/admin/reset"
}
```

In one transaction this increments `credential_generation`, sets the state to
`recovery_required`, revokes **every** session and **every** outstanding
challenge for that administrator, inserts the new `reset` verifier, and appends
the audit row. Recovery works for a pending administrator too, so it is also the
way to replace lost or expired activation material while keeping the allocated
username. Two concurrent recoveries serialize; only the challenge matching the
final generation is usable.

The browser flow is `GET /api/v1/auth/local/csrf` →
`POST /api/v1/auth/local/challenge` with `{"code": "...", "kind": "reset"}` →
`POST /api/v1/auth/local/reset` with `{"code": "...", "password": "..."}` → `204`
(no session) → normal login.

### Why the browser can never reset a password

The only code paths that write a password are `finish_challenge` for the
`activate` and `reset` kinds. Both require a one-time code that only the CLI can
issue (`admin create` / `admin recover`), both require the pre-auth cookie, CSRF
token and `Origin`, and neither is reachable from a session. `POST /api/v1/admin/reauth`
verifies the existing password and rotates the session but does not change it.
There is no self-service or in-session password change anywhere in the local
surface.

### Linearization caveat

A mutation that committed before a recovery is **not** rolled back, and it may
still be returning its response after the recovery has completed. Concretely:
"the client was created" or "the key was issued" can be the last thing the caller
sees even though the credential that authorized it no longer exists. A mutation
serialized after the recovery is rejected. Reads already admitted before the
recovery may also finish. Recovery is a fence on future work, not a cancellation
of in-flight work, and it is not a rollback. (`resolve_session`, the mutation
guard, and the transaction ordering are the mechanism; the tests cover the
invalidation outcome, not the interleaving guarantee — see §13.)

## 6. Client and key administration

### 6.1 Route table

Mounting: `crates/memory-mcp/src/http/router.rs` (local branch only). Auth column
refers to the `__Host-memory_mcp_admin` session cookie. All local routes are
wrapped in the host-allowlist middleware and the local deadline middleware.

| Method | Path | Auth | Success | Failure codes observed in code |
|---|---|---|---|---|
| GET | `/api/v1/auth/config` | none | `200 {"mode":"local"\|"oidc"}` | mounted in **both** browser-auth modes so the UI can discover which flow to run, and only while the control plane is enabled; an **off** deployment does not mount it and answers the JSON `404` (`http::router::tests::off_mode_mounts_no_browser_auth_route`) |
| GET | `/api/v1/auth/local/csrf` | none | `200 {"csrf_token": ...}` + pre-auth cookie | `503` if local auth is not configured |
| POST | `/api/v1/auth/local/challenge` | pre-auth | `200 {username, expires_at}` | `400 invalid_challenge`, `400 bad_request`, `403`, `413`, `415`, `429`, `503` |
| POST | `/api/v1/auth/local/activate` | pre-auth | `204` | as above + `409` on state conflict |
| POST | `/api/v1/auth/local/reset` | pre-auth | `204` | as above |
| POST | `/api/v1/auth/local/login` | pre-auth | `204` + session cookie | `401 invalid_credentials`, `429`, `503` |
| GET | `/api/v1/admin/session` | session | `200 {admin_id, username, auth_time, absolute_expiry, csrf_token}` | `401 unauthorized` |
| POST | `/api/v1/admin/reauth` | session + CSRF | `204` + rotated cookie | `401`, `403`, `429` |
| POST | `/api/v1/admin/logout` | session + CSRF | `204` | `401`, `403`; `204` when no session is presented |
| GET | `/api/v1/admin/clients` | session | `200 {items, next_cursor}` | `400` (bad `limit`), `401` |
| POST | `/api/v1/admin/clients` | session + CSRF + recent auth | `202` + `Location: /api/v1/admin/clients/{account_id}` | `400`, `401`, `403 reauth_required`, `409 conflict`/`idempotency_conflict` |
| GET | `/api/v1/admin/clients/{account_id}` | session | `200 ClientView` | `401`, `404 not_found` |
| GET | `/api/v1/admin/clients/{account_id}/keys` | session | `200 {items, next_cursor}` | `400`, `401`, `404` |
| POST | `/api/v1/admin/clients/{account_id}/keys` | session + CSRF + recent auth | `201 {id, name, secret, expires_at}` | `400`, `401`, `403`, `404`, `409 key_cap_reached`/`idempotency_conflict`/`secret_already_issued`/`conflict` |
| DELETE | `/api/v1/admin/clients/{account_id}/keys/{key_id}` | session + CSRF | `204` | `401`, `403`, `404 not_found` |
| POST | `/api/v1/admin/clients/{account_id}/suspend` | session + CSRF + recent auth | `204` | `401`, `403`, `404`, `409 conflict` |
| POST | `/api/v1/admin/clients/{account_id}/resume` | session + CSRF + recent auth | `204` | `401`, `403`, `404`, `409 conflict` |

There is no client-delete, purge, quota-edit, or admin-removal route.

**Error envelope** (single renderer, `handlers.rs::error_response_with_key`):

```json
{"error": {"code": "<stable code>", "message": "<safe text>", "key_id": "<only for secret_already_issued>"},
 "correlation_id": "<uuid v4>"}
```

Stable codes: `bad_request` (400), `invalid_challenge` (400), `payload_too_large`
(413), `unsupported_media_type` (415), `invalid_credentials` (401), `unauthorized`
(401), `forbidden` (403), `reauth_required` (403), `not_found` (404), `conflict`
(409), `idempotency_conflict` (409), `key_cap_reached` (409),
`secret_already_issued` (409), `throttled` (429), `temporarily_unavailable`
(503). Storage error detail is never rendered.

Two details worth knowing as an operator or client author:

- A `429` carries `Retry-After: <seconds>` **and** the retry delay in the message
  text (`"retry after <n>s"`), as spec §7 requires. The `503` deadline response
  also carries `Retry-After: 1`.
- Every error response carries a server-generated request id twice: as the
  `x-request-id` response header and as `correlation_id` in the body. Both are
  the same UUID v4.

There is deliberately **no `500`** on the local surface. A storage or adapter
failure (`LocalAdminError::Infrastructure`, what the durable store's `infra()`
helper produces for every storage error) is rendered as `503
temporarily_unavailable`, exactly like the explicit `Unavailable` variant (KDF
admission exhaustion, missing peer metadata, unconfigured local auth) and the
deadline middleware. This is what spec §7 ("storage outage fails auth closed
with sanitized `503`, never process-local fallback") and spec §8 ("store/admission
outage `503`") require. An operator alerting on `503` therefore *does* see a
registry outage.

**Unmatched API paths** return a JSON `404`, not SPA HTML
(`{"error":{"code":"not_found","message":"not found"}}`). This is verified with
the UI disabled. See §13 for the risk that the UI-enabled asset fallback masks
it.

### 6.2 Pagination

`GET .../clients` and `GET .../keys` accept `after` (opaque cursor: `account_id`
for clients, `key_id` for keys) and `limit` (default 50, must be 1–100, otherwise
`400`). They return `{"items": [...], "next_cursor": <string|null>}`.
`next_cursor` is set only when a full page was returned. Key history grows
beyond the active-key cap and is always paginated.

### 6.3 Create and read clients

`POST /api/v1/admin/clients` takes `{"display_name": "..."}` plus an
`Idempotency-Key` header containing a UUID (required; a missing or non-UUID value
is `400 bad_request`). It atomically creates the `Account` (`Active`), the
`Tenant` (`Reserved`, plan version = the local default), the metadata sidecar,
the operation record, and the audit row, and returns `202` with the `ClientView`
and a `Location` header.

`ClientView` fields: `account_id`, `tenant_id`, `display_name`, `account_status`,
`tenant_status`, `plan_version`, `schema_version`, `version`,
`provisioning_reason`.

`account_status`, `tenant_status`, `plan_version` and `schema_version` are read
from the live `account` and `tenant` rows the provisioning worker writes, not
from the metadata sidecar the create transaction filled in. The sidecar is only
a point-in-time snapshot, so a client whose tenant reached `ready` reports
`ready` immediately. `version` is the one exception: it is the sidecar's optimistic
concurrency guard, which `suspend`/`resume` and key issuance advance, and it is
distinct from the tenant's own version.

`provisioning_reason` is `null` whenever provisioning is healthy, and otherwise a
closed, allowlisted sentence: `provisioning failed` or
`provisioning failed at <stage>` for a `failed` tenant, and
`retrying from <stage>` for a tenant that is still `reserved`,
`namespace_creating` or `migrating` with a recorded retry stage. `<stage>` is one
of `reserved`, `namespace_creating`, `migrating`, `ready`, `suspended`, `failed`,
`deleting`, `purged`. Raw storage or migration text is never returned through
this field.

The initial `tenant_status` is `reserved`; the existing provisioning worker
advances it to `ready`, normally within one scheduler tick (1 s). Poll
`GET /api/v1/admin/clients/{account_id}` until `account_status == "active"` and
`tenant_status == "ready"` before issuing keys — issuance requires exactly that
pair (§6.4).

**Client visibility is shared between local administrators.** No client read or
mutation filters on `creating_admin_id`: plan R2 requires equal administrators to
see the same clients, and the sidecar keeps `creating_admin_id` as an audit
attribute rather than an access boundary. A second local administrator lists,
reads, renames (key names), suspends and resumes a client created by the first;
`http_local_admin.rs::a_second_administrator_sees_and_can_administer_the_same_clients`
asserts the list, read, suspend and resume paths. What remains per-administrator
is the *operation/idempotency* scope (§6.4) and the audit trail, so a replay of
another administrator's operation id does not resolve to their resource.

### 6.4 Idempotency contract

| Operation | Rule |
|---|---|
| Client create | Scoped to `(admin_id, operation_kind='client_create', idempotency_id)`. Same key **and** same `display_name` → `202` with the existing resource's current view, no second Account/Tenant. Same key, different `display_name` → `409 idempotency_conflict` |
| Key issue | Scoped to `(admin_id, operation_kind='key_issue', idempotency_id)`, with the fingerprint covering `name` and the expiry choice. Same key and same body → `409 secret_already_issued` carrying `key_id`. Same key, different body → `409 idempotency_conflict` |

Operation records are kept with their resources and have no TTL. Note that the
key-issue fingerprint does not include the target `account_id`, so replaying one
`Idempotency-Key` against a *different* client with an identical name and expiry
is treated as a replay of the first operation (it returns the first key's id with
`409`). Use one idempotency key per deliberate issuance.

Rules for clients and clients-of-clients: the server never auto-retries a
credential POST; on a lost response, list the client's keys and revoke the unknown
one, then issue a new one with a fresh idempotency key.

### 6.5 One-time secret rule

`POST .../keys` body:

```json
{"name": "worker-a", "expiry": {"kind": "never"}}
{"name": "worker-b", "expiry": {"kind": "days", "days": 30}}
```

`expiry` is a required tagged object. `{"kind":"days"}` with `days` outside
`1..=3650` is `400 bad_request`; a missing, null, or additional/unknown expiry
value is rejected by the strict DTO (`400`). There is no "blank means never"
behaviour — a deliberate choice is mandatory.

Response `201`:

```json
{"id": "ak_<uuid>", "name": "worker-a", "secret": "mem_sk_ak_<uuid>_<64 hex>", "expires_at": null}
```

`secret` is the full bearer credential and is shown **exactly once**. Only
the keyed verifier (HMAC over the pepper, `KeyedVerifier`) is persisted, in
`local_admin_client_key`. The material is produced by one shared helper
(`service/credential_material.rs::generate_api_key_material`), which the
`credential_material` unit tests pin to the data-plane parser
(`principal::api_keys::ApiKeyCredential`) so the issued shape cannot drift from
the accepted shape. Retrying the same idempotency key can never produce the
secret a second time; it returns `409 secret_already_issued` with `key_id`. The
repair path is therefore **revoke + reissue**: `DELETE .../keys/{key_id}` then a
new `POST .../keys` with a new idempotency key. Deliver the secret to the client
out of band; the service has no email or other delivery mechanism.

Key cap: issuance requires `Account=Active` and `Tenant=Ready`, then counts
non-revoked, non-expired keys for the account and refuses with
`409 key_cap_reached` at `plan.limits.max_active_api_keys`. `created_at` and
`expires_at` are derived from database time inside the insertion transaction;
"never" is stored as `expires_at = NONE`. The count, the cap and the insert are
one transaction, and the same transaction advances the client's guard `version`,
so two admins issuing concurrently at `cap − 1` serialize on that row: at most
one succeeds. There is no separate per-account counter row.

**Issued keys authenticate `POST /mcp`.** The credential is
`mem_sk_ak_<uuid>_<secret>` (the form `principal/api_keys.rs` parses), and the
insertion transaction writes **two** rows: the authoritative `api_key` row that
`RegistryStore::find_api_key` reads for data-plane authorization, and the
`local_admin_client_key` sidecar that the admin surface lists and scopes
revocation by. Revocation updates both in one transaction, so the sidecar cannot
drift from the authorization row. `http_local_admin.rs::issued_key_authenticates_on_the_data_plane`
asserts the round trip; `revoking_an_issued_key_denies_with_and_without_a_warm_cache`
asserts that revocation takes effect immediately, on a cold and a warm cache;
`a_key_for_one_client_cannot_reach_another` asserts cross-client isolation.

## 7. Suspend and resume

`POST .../suspend` / `POST .../resume` take `{"expected_version": <u64>}` and are
compare-and-swap operations on `ClientView.version`.

| Operation | Requires (coherent source state) | Result |
|---|---|---|
| Suspend | `Account=Active`, `Tenant=Ready`, `suspended_from_ready=false` | `Account=Suspended`, `Tenant=Suspended`, marker `true` |
| Resume | `Account=Suspended`, `Tenant=Suspended`, marker `true` | `Account=Active`, `Tenant=Ready`, marker `false` |

- A request whose client is **already** at the coherent desired state returns
  `204` with no writes and no audit **even if `expected_version` is stale**. The
  no-op check runs before the version check.
- Otherwise, a source-state mismatch (including `Reserved`, `Migrating`,
  `Failed`, `Deleting`, `Purged` tenants) is `409 conflict`, and so is a stale
  `expected_version` (`409 conflict`, same code and message; refresh the client
  and retry).
- A successful change increments the sidecar `version` and the existing Tenant
  `version` in one transaction, and appends the audit row
  (`client_suspended` / `client_resumed`).
- An incoherent pair is never repaired implicitly.
- The sidecar `version` is the client's optimistic-concurrency guard and is also
  advanced by key issuance and key revocation. Read it, then mutate.
- A suspended tenant is **not** provisioning work: `list_due_provisioning`
  excludes `suspended`, and `provision_one` treats it as a terminal no-op, so the
  worker neither claims a lease nor logs a conflict every tick while a client is
  suspended. Resume is the explicit `Suspended → Ready` transition.

Suspension is a local-side state change: it flips the durable `Account` and
`Tenant` rows that the data plane already reads, so a cached bearer principal is
rejected on its next re-read. Preserving that guarantee requires the data-plane
key revalidation that exists in `authenticate_bearer`: a positive cache hit
re-reads the key row (status, expiry **and** owner) plus the account status
before authorizing. Keys already issued are preserved across suspend/resume;
revoked and expired keys stay invalid. The separate subscription
`is_current` recheck remains bounded by
`MEMORY_MCP_HTTP_SUBSCRIPTION_AUTH_RECHECK_SECS` (default 30 s, maximum 60 s);
in-flight operations are not cancelled retroactively. Data-plane key expiry uses
replica wall-clock time, so keep replica clocks synchronized.

## 8. Throttling

Admission is **mandatory and pre-credential**: every entry point that can consume
password or challenge material calls `LocalAdminService::admit` before any
credential lookup, KDF, or challenge read (`service/local_admin/auth.rs`). It is
not an optional HTTP pre-check, so no route can be used to bypass it.

### 8.1 Policy

Counters are durable rows in `local_admin_rate_bucket`, shared by all replicas
(no process-local fallback). Fixed windows and caps
(`surreal_store/local_admin.rs`):

| Domain | Dimension | Cap | Fixed window |
|---|---|---|---|
| Credentials (login **and** reauth) | per username bucket | 5 | 15 minutes (`900s`) |
| Credentials (login **and** reauth) | per source bucket | 30 | 5 minutes (`300s`) |
| Challenge (inspect, activate, reset) | per source bucket | 10 | 15 minutes (`900s`) |

A successful login does not clear any budget, and another replica shares the same
counters. Saturation fails closed: an over-cap bucket increments a saturating
denied counter instead of creating or evicting rows, and returns `429 throttled`
with the seconds until the window reopens in the message. There is no persistent
account lockout — the window simply reopens. A coarse in-process admission limit
supplements but never replaces these durable counters. Known and unknown
usernames use the same keyed normalization, and a malformed login name is keyed
as bounded raw input (first 64 bytes, char-boundary safe) into the same
credential domain plus the same source bucket, so syntax validation is not an
account-existence oracle.

`inspect` + `finish` deliberately share one challenge budget: a normal
inspect-then-finish flow consumes two attempts of the ten.

### 8.1.1 Concurrent writes contend for one row, so the execution seam retries

Every attempt in a window touches the same one or two bucket rows, so
simultaneous admissions — two logins at once, or every client behind a proxy
sharing one source bucket — contend for the same row. SurrealDB aborts the
losing transaction atomically (`Write conflict, retry the transaction`), which
without handling surfaced as a `503` for an ordinary concurrent login.

The retry lives in the **shared execution seam**
(`SurrealRegistryStore::admin_query` / `admin_query_at`), not in `reserve_attempt`
alone: it retries up to five times on exactly that error, with a short backoff
(2/4/8/16/32 ms). The retry is safe because every statement reaching the seam is
one atomic transaction — the guard-row update, the resource write, the
idempotency operation and the audit row either all commit or none do — so an
aborted attempt made no change and the re-run re-reads the persisted state. A
read cannot lose a write race, so the retry never fires for one.

Keeping it in the seam also covers the client-administration mutations, where it
is the difference between "serialized" and "a spurious `503`": two
administrators issuing a key for one client at `cap − 1` now produce exactly one
`201` and one `409 key_cap_reached`
(`http_local_admin.rs::two_administrators_racing_the_last_key_slot_issue_exactly_one_key`),
and `exp15_concurrent_logins_same_user` asserts three simultaneous logins for one
identity all succeed with distinct cookies.

### 8.2 Source identity

Throttling uses the **direct socket peer IP** only
(`control/local_admin.rs::direct_peer`). `Forwarded`, `X-Forwarded-For`, and
`X-Real-IP` are deliberately ignored, because a client can set them freely and
honouring them would let one source mint unlimited buckets. IPv4-mapped IPv6
peers are normalized to IPv4 so one client cannot appear as two buckets. If the
server has no `ConnectInfo` (started without connect-info), no source can be
determined and every such attempt fails closed with
`503 temporarily_unavailable`.

Consequence for a deployment behind one reverse proxy: **all browser and API
clients share the proxy's source bucket.** Thirty credential attempts per five
minutes and ten challenge attempts per fifteen minutes are then a deployment-wide
budget, and a single abusive client can throttle unrelated users. This is an
accepted availability trade-off, not a bug; forwarded client-IP support is
explicitly deferred. Trusted forwarded **Host** handling is separate and
unchanged: the proxy must overwrite `Host` and reject duplicates/comma lists.

### 8.3 Bucket collisions and the fixed-window boundary

Keyed usernames and sources map into a fixed slot range of **4096 slots per
dimension** (`ATTEMPT_BUCKET_SLOTS = 4096`, valid bucket ids `0..=4095`); the
store rejects out-of-range buckets and requires the username dimension for
credential attempts. Two consequences the operator must accept:

- **Collisions throttle unrelated users** (two distinct usernames or sources hashing
  to the same slot share a budget) but can never grant extra attempts.
- **Fixed-window boundary bursts** are possible: a caller can spend a full budget
  at the end of one window and another full budget at the start of the next. The
  windows are fixed, not rolling.

Because the table is keyed by `(domain, dimension, slot)` with a unique index,
cardinality is bounded by construction to roughly 12,288 rows (three slot spaces
of 4096) regardless of offered input. A bounded cleanup pass
(`LocalAdminStore::cleanup_rate_buckets` → `SurrealRegistryStore::cleanup_expired_rate_buckets`,
batches of 512, database-time based) is registered as a scheduler job
(`local_admin_rate::rate_bucket_cleanup_scheduler_job`, at most one pass every
300 seconds per process). Because expiry is enforced by database time on every
reservation, a missed or delayed pass can never admit an extra attempt; the job
therefore reclaims rows rather than enforcing the policy.

### 8.4 Failure auditing

The audit table records two kinds of event:

- **Admitted authentication failures** append at most one deduplicated row per
  `(request id, action)` pair. The service reserves the attempt first and only
  then records the rejection, so a throttled or never-admitted request leaves no
  audit row at all — it only moves the saturating counter in
  `local_admin_rate_bucket`. Actions are `login`, `reauth` and `challenge`;
  reasons are `invalid_credentials` and `invalid_challenge`; the actor is
  `anonymous` unless the username resolved to an admin.
- **Everything else** (invalid cookie, stale mutation, pre-admission denial) is
  counted by the fixed action/reason aggregate slots in the same rate table.
  There is deliberately no append-per-invalid-cookie path.

If the failure-audit write fails, the request fails closed with `503
temporarily_unavailable` rather than the caller's `401`/`400`: spec §10 requires
that a failed-login audit failure is never silently dropped, and it never permits
the login. The audit row carries `event_time`, `actor_kind`/`actor_id`, `action`,
`outcome`, the safe `reason` enum, the request id, and — for key issuance —
`grants_client_data_access`. Bounded bucket identifiers are **not** projected
into the audit row: the approved migration `047` has no such columns, and the
bucket dimensions are already aggregated in `local_admin_rate_bucket` (§13.4).

## 9. Secrets and rotation

### 9.1 Where the keys live

| Key | Source | Role in this revision |
|---|---|---|
| `MEMORY_MCP_HTTP_SESSION_KEY` | Deployment env / protected operator env file | Feeds the `browser_auth_policy` configuration fingerprint only. Session cookies are independent 256-bit random verifiers, so this key does not sign or verify a session and rotating it does not by itself invalidate existing sessions — it makes the policy join fail |
| `MEMORY_MCP_HTTP_CSRF_KEY` | Deployment env / protected operator env file | Pre-auth cookie signatures, session CSRF tokens, and the domain-separated throttle bucketing key |
| `MEMORY_MCP_API_KEY_PEPPER` | Deployment env | Client key verifier HMAC |

No key is stored in the database. The registry stores only hex **fingerprints**
(HMACs) of the two browser keys in `browser_auth_policy`, plus the policy `mode`
and `epoch`. The same variables must be supplied to the CLI; the CLI joins the
same policy and will fail if the fingerprints differ.

Both keys must be generated by the operator with a CSPRNG (for example
`openssl rand -hex 32`) and must never be example defaults or zeros. Do not log
the fingerprints.

### 9.2 Rotation is not implemented as a procedure

The design calls for rotation to be an offline maintenance operation that
increments the policy epoch and invalidates every outstanding session, challenge,
and CSRF token. What this revision actually provides is the **fence**, not the
ceremony:

- Pre-auth and session CSRF tokens are HMACs of the CSRF key and bind the policy
  epoch, so replacing the CSRF key invalidates every outstanding CSRF token
  immediately. Durable sessions and challenges are *not* revoked by that: they
  are keyed by stored random verifiers and by credential generation, not by the
  CSRF key.
- Every guarded mutation re-reads the policy epoch inside its transaction
  (`policy_stale`), so a changed epoch would abort in-flight mutations. The
  chapter is theoretical, because **no code path increments `epoch` or rewrites
  the fingerprints**: the epoch is created as `1` and stays there.
- Replacing either key changes its policy fingerprint, so the next replica start
  fails the policy join with `policy_mismatch` — a hard startup error rather than
  a graceful rotation.

As a result there is no supported rotation command. If you replace the keys you
must also update (or remove) the `browser_auth_policy` row through direct,
offline database access, and you must first stop every control-plane replica and
force every administrator to log in again. Two further design requirements are
only partly present: replicas with differing key fingerprints are rejected at
startup (the fingerprint comparison exists and is enforced), while the 15-minute
public-authentication admission hold after rotation has no implementation at all.
Treat key rotation as an unsupported operation until a procedure is added.

### 9.3 Clock requirements

Most deadlines use database time (`time::now()`): challenge expiry, session
idle/absolute expiry, recent-auth, throttle windows, and key expiry derivation.
Two things do **not**: the pre-auth material timestamp is checked against server
UTC with a maximum accepted skew of **30 seconds** in either direction (out of
range fails closed, it is not clamped), and data-plane key expiry uses replica
wall-clock time. Replica clocks must therefore be synchronized (NTP or
equivalent) and skewed replicas must be fixed rather than tolerated.

## 10. Backups, restore, and the embedded-CLI caveat

All local-admin records live in the control registry
(`SURREALDB_CONTROL_NAMESPACE` / `SURREALDB_CONTROL_DB`, migration `047_local_admin_auth.surql`):
`browser_auth_policy`, `local_admin`, `local_admin_challenge`,
`local_admin_session`, `local_admin_rate_bucket`, `local_admin_client`,
`local_admin_client_key`, `local_admin_operation`, `local_admin_audit`. All are
`SCHEMAFULL` with unique indices where required.

**Treat the registry backup as a credential database.** It contains Argon2id
password hashes, session cookie verifiers, challenge verifiers, and client key
verifiers. Restoring an older snapshot can resurrect previously revoked or
expired credentials and older credential generations. After any restore that
rewinds the registry:

1. Replace the browser session and CSRF keys (see §9.2 — this currently requires
   direct policy-row maintenance, and a registry rewind can resurrect old
   credential rows).
2. Recover each administrator with `admin recover` so the generation advances and
   old sessions/challenges are invalidated durably.
3. Only then return traffic to the deployment.

Keep CLI access restricted to operators who already hold control-registry
credentials; multi-administrator equality does not remove the need for host and
database access control. `docs/operations/RESTORE_DRILL.md` covers the generic
SurrealDB restore procedure.

**Embedded-RocksDB caveat.** The CLI opens `SURREALDB_CONTROL_URL` itself. With
`rocksdb://<path>` an embedded RocksDB permits one process handle per path, so the
CLI must run with the HTTP server stopped for that path, or against a remote
registry URL. `tests/local_admin_cli.rs` proves the CLI works against a
file-backed `rocksdb://` registry when nothing else holds it (it creates and then
recovers an administrator across two separate processes). The failure mode of
running the CLI while the server holds the same path was not exercised.

**Migration asset path.** The registry migration SQL is read at runtime from
`CARGO_MANIFEST_DIR/migrations`, i.e. the absolute build-time path
`<build-root>/crates/memory-mcp/migrations`. Any image or deployment that ships
the binary must keep `047_local_admin_auth.surql` (and `001_registry`,
`045_deletion_and_usage_hardening`, `046_registry_correctness`, the only other
catalog entries) at that path — the current `Dockerfile` does this with a `COPY`
from the builder stage.

## 11. Rollback

The plan's rollback posture is: disable local browser control before running an
older binary, and retain the additive records and data-plane keys. Verified
against the code:

| Question | Answer |
|---|---|
| Is migration 047 additive? | Yes. It only defines new tables/fields/indices; it alters no existing table and no existing record. Verified by reading the migration. |
| Are local-admin records retained on rollback? | Yes. Migration files are applied from a catalog; an older binary simply does not know about 047 and does not remove its tables. Administrator, session, challenge, client, key, operation, rate-bucket, audit, and policy rows persist. |
| Are data-plane keys retained? | Yes. Existing keys are untouched: a rollback only unmounts the browser surface. Locally issued keys live in **both** `api_key` (the authoritative authorization row the data plane reads) and the `local_admin_client_key` sidecar, so a key issued for a client keeps working against `POST /mcp` after a rollback as long as its tenant is `ready`; revoking it before the rollback removes it from both tables (§6.5). |
| Does an older binary enforce the mode fence? | No. The `browser_auth_policy` fence is checked only by this revision's code (`join_local_policy` and the mutation guard). The approved design states an older binary cannot be made safe by the new table alone, so stopping every control-plane replica before rolling back is a mandatory precondition, not a precaution. |
| What happens to a locally created Account/Tenant? | The `account` and `tenant` rows are ordinary rows in existing tables and remain. With the local surface unmounted they have no browser administrator; they are not adopted by any other workflow. |
| Can an old binary serve local administrators? | No. There is no local admin route or credential in a pre-047 binary; administrators created locally have no OIDC identity, so they cannot authenticate against the OIDC control plane. |

Practical rollback sequence: stop all replicas → switch to the mode you are
rolling back to (`MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE=false` for a pure
data-plane rollback, or an OIDC-mode configuration) → start the older binary.
Re-enabling local mode afterwards requires the identical session/CSRF keys,
otherwise the policy join fails (`policy_mismatch`); administrators are retained
unless you deliberately recover or rotate them.

## 12. Explicit non-goals

- **No adoption of pre-existing OIDC accounts.** A local administrator cannot
  claim, list, or administer an Account created by the OIDC workflow; local
  listing is limited to sidecar-backed clients, and further scoped to the
  creating administrator.
- **No administrator removal.** There is no delete, disable, or role-management
  command or route. An administrator can be recovered (password reset) but not
  removed.
- **No MFA, no email, no self-registration.** No TOTP/WebAuthn, no password-reset
  email, no `open` signup route mounted in local mode.
- **No purge UI.** No client deletion, quota editing, or purge route.
- **No browser password reset.** Recovery is CLI-only (§5).

**Honest statement of administrator privilege:** an administrator can create a
client, wait for its tenant to become ready, and issue API keys for it. Issuing a
key is audited (`key_issued` with `target_account_id`, `target_tenant_id`,
`target_key_id`, and `grants_client_data_access = true`), and the key is a real
data-plane credential: it authenticates `POST /mcp` and reaches that client's
memory (§6.5). Do not claim that a tenant's memory is private from the
deployment administrator: the administrator controls key issuance for the
clients they created, and the operator controls host and database access
regardless.

## 13. Verification matrix

### 13.1 Claims demonstrated by a test or a command actually run

| Claim | Evidence |
|---|---|
| Local routes are mounted only in local mode; OIDC routes are absent | `http_local_admin.rs::local_mode_does_not_mount_oidc_routes` |
| `GET /api/v1/auth/config` reports the mode in both browser-auth modes | `http_local_admin.rs::auth_config_reports_local_mode_only` (local), `http_control_plane.rs::auth_config_reports_oidc_mode_without_disclosing_more` (OIDC) |
| Pre-auth cookie is `__Host-`, `Secure`, `HttpOnly`, `SameSite=Strict`, `Path=/` | `http_local_admin.rs::preauth_cookie_is_host_scoped_and_httponly` |
| Activation → login → session round trip, no session from activation | `http_local_admin.rs::activation_login_session_roundtrip` |
| Unknown user / wrong password are indistinguishable (`401 invalid_credentials`) | `http_local_admin.rs::wrong_password_is_uniformly_unauthorized`; service `login_unknown_user_fails_with_dummy_hash`, `login_wrong_password_fails` |
| All bad challenge shapes return one identical body | `http_local_admin.rs::every_bad_challenge_shape_returns_the_same_body` |
| Public POST without CSRF / with another cookie's token / without Origin / with a bad Origin is rejected | `public_post_without_csrf_token_is_rejected`, `public_post_with_another_cookies_token_is_rejected`, `public_post_without_origin_is_rejected`, `public_post_with_disallowed_origin_is_rejected` |
| Session mutation without CSRF is rejected | `session_mutation_without_csrf_token_is_rejected` |
| Duplicate cookie values are rejected | `duplicate_cookie_values_are_rejected` (plus the `csrf.rs` unit test `duplicate_cookie_is_rejected`) |
| Body limits and content type are distinct failures | `oversized_and_malformed_and_wrong_content_type_are_distinct`, `oversized_body_is_rejected` |
| Missing direct peer fails closed | `missing_peer_fails_closed` |
| Logout revokes only the presented session; no-session logout is a no-op | `logout_revokes_the_presented_session_only`, `logout_without_a_session_is_a_no_op` |
| Recovery invalidates old credentials and old sessions | service `local_admin_recovery_invalidates_old_credentials`; `http_local_admin.rs::reset_changes_the_password_and_fences_old_sessions` |
| A client mutation after logout is rejected by the transaction (not merely by middleware) | `http_local_admin.rs::client_mutation_follows_the_session_fence` |
| Stale credential generation is rejected | `http_local_admin.rs::auth_fence_requires_a_matching_generation` |
| Client create is durable and reachable through the store | `client_lifecycle_uses_the_durable_store`, `created_clients_reach_the_durable_store` |
| Missing `Idempotency-Key` is `400` | `http_local_admin.rs::missing_idempotency_key_is_rejected` |
| Unauthenticated client list is `401` | `http_local_admin.rs::unauthenticated_client_list_is_unauthorized` |
| Unmatched `/api/*` and `/auth/*` return JSON `404` (UI disabled) | `http_local_admin.rs::unmatched_api_and_auth_paths_are_json_404_not_html` |
| A forged/unknown admin cookie is `401`, not a bearer privilege | `http_local_admin.rs::bearer_keys_cannot_authenticate_local_admin_routes` |
| Auth config / pre-auth / session CSRF token binding to epoch and session | `csrf.rs` unit tests (`preauth_roundtrip`, `preauth_rejects_expired`, `preauth_rejects_future_timestamp`, `preauth_rejects_wrong_epoch`, `preauth_rejects_wrong_token`, `origin_must_be_present_and_exact`, `session_token_binds_epoch_and_session`) |
| Argon2id parameters, per-hash salting, corrupt/foreign PHC rejection, bounded KDF admission | `password.rs` unit tests, including `a_saturated_admission_queue_fails_closed` |
| Username and password policy | `policy.rs` unit tests |
| CLI surface, `--username` only, no password/code flags, stable mode label, not a one-shot | `local_admin_cli.rs` parser tests |
| CLI persists across processes against file-backed RocksDB, no OIDC/model init | `local_admin_cli.rs::admin_create_then_recover_persists_across_processes` |
| `admin` absent from a `--no-default-features` build | `cargo run -p memory_mcp --no-default-features --locked --bin memory_mcp -- --help` |
| Local mode starts from environment variables against the real binary | `scripts/ci/local_admin_local_check.sh` §2–§12 (§2.5) |
| Provisioning advances a freshly created client to `ready` | Same script §7b — `tenant_status after 1 polls: ready`, plus `op=schema.init` in the instance log (§2.5, §12) |
| The client view reports live tenant state, not the create-time snapshot | `http_local_admin.rs::client_view_reflects_live_tenant_state` (writes plan 3 / schema 7 directly and reads them back) |
| `provisioning_reason` is a closed, allowlisted mapping | `http_local_admin.rs::client_view_reports_a_bounded_provisioning_reason` (a `failed` tenant at stage `migrating` → `provisioning failed at migrating`) |
| Suspend/resume coherent no-op, stale CAS, and no reprovision while suspended | `http_local_admin.rs::suspend_and_resume_follow_the_coherent_state_contract`, `a_provisioning_client_cannot_be_suspended` |
| A stale plan is refused at startup rather than adopted | `scripts/ci/local_admin_local_check.sh` §10 — `plan_limit_mismatch` |
| Pre-auth code, activation, login, session and recovery survive a real restart | Same script §9, §11 |
| `control-plane-ui` requires an absolute, non-symlink bundle directory containing `index.html` | `crates/memory-mcp/build.rs`; the suite only builds with `MEMORY_MCP_CONTROL_PLANE_UI_DIST` pointing at one |
| The image builds both binaries and a real UI bundle | `docker build` → `25/25 FINISHED`; `memory_mcp --help` lists `admin`; `control-plane-ui-dist/public` contains `index.html`, a 46 KB JS and a 775 KB WASM |
| The UI boots in a real browser under the shipped CSP | `local_admin_image.py --scenario ui` → 10/10 checks, including `wasm app boots under the shipped csp` and no console/page errors |
| The CLI in the image issues a code and the browser completes activation, login, client, key, suspend/resume | `--scenario all` → 54 checks (auth 9, clients 29, regression 6, ui 10) |
| All three Compose modes resolve with operator-generated secrets | `docker compose --env-file … -f docker-compose.yml -f docker-compose.{off,local,oidc}.yml config --quiet` |
| Issued key is a real data-plane credential | `scripts/ci/local_admin_local_check.sh` §7c — live key `200` on `POST /mcp`, revoked key `401`, no credential `401`; `http_local_admin.rs::issued_key_authenticates_on_the_data_plane` |
| A tenant stranded in `NamespaceCreating` is resumable | `http_crash_recovery.rs::provisioning_resumes_a_tenant_stranded_in_namespace_creating` |
| Concurrent admissions for one identity all succeed | `security_tests.rs::exp15_concurrent_logins_same_user` (three simultaneous logins, distinct cookies) |
| The durable rate caps are enforced and per-identity | `security_tests.rs::exp9_rate_buckets_enforce_the_cap`, `exp9b_challenge_budget_is_shared_and_source_scoped` |
| Two administrators racing the last key slot on one client issue exactly one key | `http_local_admin.rs::two_administrators_racing_the_last_key_slot_issue_exactly_one_key` — one `201`, one `409 key_cap_reached`, and the client holds exactly `cap` keys |
| Generated secrets never reach a durable row, `Debug` or `Display` | `surreal_store/local_admin.rs::secret_hygiene_tests::sentinel_secrets_never_reach_debug_display_or_a_durable_row` |
| An off deployment mounts no browser-auth route | `http::router::tests::off_mode_mounts_no_browser_auth_route` (paired with `an_enabled_control_plane_mounts_the_mode_disclosure`, so it cannot pass vacuously) |

### 13.2 Claims NOT demonstrated

| Claim | Status |
|---|---|
| Remote replica races (`local_admin_remote_replica_races`, `local_admin_session_revocation_race`) | ❌ both `#[ignore]`d and moved inline into `surreal_store/local_admin_remote.rs` so `--lib … -- --ignored` selects them; they need an isolated remote SurrealDB 3.2.4 and the three `LOCAL_ADMIN_TEST_CONTROL_*` variables, and fail rather than skip when selected without them |
| Two local replicas sharing live throttle/auth/idempotency state | ❌ no multi-process test: the durable rows are the mechanism, but two servers were never run against one registry |
| OIDC login against a live identity provider | ❌ no IdP in this environment; OIDC is exercised at store and router level |
| Host-side `dx bundle` | ❌ `dx` is absent on the host; only the pinned `dioxus-cli 0.7.10` inside the image builds the bundle |
| The `linux/amd64` image | ❌ the verification host is `arm64`; CI pins amd64 but that path was not reproduced locally |
| Forced ordering / barriers between two in-flight transactions (KDF-pause + recovery; prepare-reset + newer recovery; both resolve/logout orders; two rotations racing) | ❌ outcomes are covered, interleavings are not (§13.3) |
| Database unavailable during reserve/auth/success-audit/failure-audit | ✅ `surreal_store/local_admin.rs::sql_fault_tests` — the SQL fault hook fails the reservation, the session insert, the success-audit insert and the failure-audit insert in turn; each case asserts a sanitized failure, no session/key, and no partial credential/state change |
| Rate-window boundary across replicas and after a restart | ❌ the fixed window and its saturation are tested in one process; a restart in the middle of a window is not |
| Client visibility across two administrators | ✅ `http_local_admin.rs::a_second_administrator_sees_and_can_administer_the_same_clients` (list, read, suspend, resume) |
| `surrealdb` SQL fault injection in the local-admin store (nonselected statement error, audit insert error) | ✅ `sql_fault_tests` — a test-only SQL fault hook fails a named local-admin statement; the aborted transaction leaves no session, no success audit row and no half-applied credential change |
| CLI versus a live holder of the same embedded RocksDB path | ❌ only standalone CLI use is proven; the script stops the server first by design |
| Unscheduled rate-bucket cleanup | ✅ scheduled: `local_admin_rate::rate_bucket_cleanup_scheduler_job` is registered by `memory_mcp_http`, at most one bounded pass every 300 s per process |

### 13.3 Where each plan §5 security case is proven

The experiment list lives in
`crates/memory-mcp/src/service/local_admin/security_tests.rs`, whose module
documentation carries the same table. Every experiment there now runs against
the real durable store; cases that cannot be expressed at that seam point at the
suite that covers them instead of asserting against a test double.

| §5 case | Evidence |
|---|---|
| Same activation/reset verifier through independent handles | `exp1_same_verifier_double_submit` (durable) |
| KDF completes, pause before session insert; recover on a second handle | `http_local_admin.rs::client_mutation_follows_the_session_fence`, `reset_changes_the_password_and_fences_old_sessions` |
| Reset hash prepared, newer recovery commits | `exp3_recovery_invalidates_old_credentials` (generation, not interleaving) |
| Resolve/touch and recovery/logout, both commit orders | `exp4_revoke_after_login`; interleavings unproven |
| Client mutation paused after middleware, logout commits | `client_mutation_follows_the_session_fence` |
| Two rotations | `exp6_reauth_rotates_session`; interleavings unproven |
| Replica joins wrong mode or wrong key fingerprints | `exp16_wrong_mode_policy_fingerprint_mismatch`, `surreal_store.rs::join_oidc_policy_rejects_an_existing_local_policy` |
| OIDC transition; legacy session lacks an epoch | `surreal_store.rs::find_session_rejects_legacy_and_stale_epoch_rows` |
| Rate: two handles, collision, window boundary | `exp9_rate_buckets_enforce_the_cap`, `exp9b_challenge_budget_is_shared_and_source_scoped`, `exp15_concurrent_logins_same_user` |
| Spoofed `X-Forwarded-*`, missing peer, mapped IPv6 | `missing_peer_fails_closed`; `control::local_admin` peer-normalization tests |
| KDF timeout/queue/cancellation/corrupt PHC | `password.rs` unit tests, including `a_saturated_admission_queue_fails_closed` (a held slot fails the bounded deadline for a real *and* a dummy verification) |
| Nonselected SQL statement error / audit insert error | `surreal_store/local_admin.rs::sql_fault_tests` — `session_insert_statement_error_leaves_no_session_and_rolls_back`, `success_audit_error_rolls_back_the_credential_change`, `failure_audit_storage_error_is_sanitized_unavailable_not_a_rejection` |
| DB unavailable during reserve/auth/audit | `sql_fault_tests::reservation_storage_error_fails_closed_without_admitting_the_attempt`, `failure_audit_storage_error_is_sanitized_unavailable_not_a_rejection`; `control::local_admin::handlers::tests::spec_status_table_is_exhaustive` pins `Infrastructure → 503` |
| Two admins issue at `cap − 1` | `two_administrators_racing_the_last_key_slot_issue_exactly_one_key` — two logged-in administrators race the last slot on one client: exactly one `201`, one `409 key_cap_reached`, and the client holds exactly `cap` keys; `active_key_cap_counts_only_live_keys` covers the serial cap/revoke arithmetic |
| Create same operation/body; different body; lost response | `client_lifecycle_uses_the_durable_store`, `missing_idempotency_key_is_rejected` |
| Key issue response lost and repeated | `insert_client_key` → `AlreadyIssued`; `http_local_admin.rs::a_repeated_key_issue_never_returns_a_second_secret` (409 + public key id, one key row, no second verifier); UI `secret_already_issued` tests |
| Provisioner restart in `Reserved`/`NamespaceCreating`/`schema 0`/`Migrating` | `http_crash_recovery.rs` (11 tests, including `provisioning_resumes_a_tenant_stranded_in_namespace_creating`) |
| Coherent suspend/resume, stale CAS, no false `Ready` | `suspend_and_resume_follow_the_coherent_state_contract`, `a_provisioning_client_cannot_be_suspended` |
| Warm cache; revoke or expire; resume | `revoking_an_issued_key_denies_with_and_without_a_warm_cache`, `expiry_at_the_boundary_is_rejected`, `cache_hit_with_a_mismatched_owner_is_denied` |
| Public auth and every admin route: Origin/CSRF/content type/body/duplicate cookie/unknown fields | 42 tests in `http_local_admin.rs` |
| Cookie/bearer privilege separation, unmounted APIs | `bearer_keys_cannot_authenticate_local_admin_routes`, `unmatched_api_and_auth_paths_are_json_404_not_html`, `a_key_for_one_client_cannot_reach_another` |
| Packaged UI/CLI over trusted TLS | `local_admin_image.py --scenario all` — 54 checks in a real browser |
| Generated sentinel secrets through errors/Debug/logs/audit/metadata | `surreal_store/local_admin.rs::secret_hygiene_tests` — the activation code, the session cookie and the password are absent from all nine tables and from every `Debug`/`Display` rendering, while each is present in the value its authorized caller receives; `LocalAdminError::Infrastructure` prints `<redacted>` instead of the wrapped `MemoryError` |

### 13.4 Divergences from the approved interface ledger

Stated rather than smoothed over:

| Plan §3.4 entry | What shipped |
|---|---|
| Eight new tables | Nine shipped: `local_admin_client_key` is a ninth. It is the sidecar that scopes key *ownership* to the creating administrator (the `api_key` row alone cannot express which administrator issued a key), and it is written in the same transaction as the authoritative row. |
| "never ensure hardcoded `free` for local/off" | Diverges for `off`: the `free` v1 plan is still ensured except in local browser mode. Off-mode tenants carry `plan_version 1` (the data plane resolves that row on every ingest, and tenants that predate this change keep that version), so removing the row strands their quota resolution. Local browser mode no longer publishes it at all — its plan is the deployment's `local_plan_v{version}`. |
| The in-memory `LocalAdminStore` fixture | Removed. Every plan §5 experiment now runs against the real durable store; the test double had no callers and asserted behaviour the production store does not have. |
| Off mode needing no browser keys | Diverges: `HmacKeys` is a single non-optional struct, so `off` still requires the five key variables (the compose `off` overlay refuses to default them). Zero-filling them is forbidden by the same plan, so the implementation chose "fail closed and require operator keys" over "start with forgeable keys". |
| "Every local client accessible to either administrator" | Implemented: reads and mutations no longer filter on `creating_admin_id` (§6.3). The column remains as an audit attribute. |
| Explicit `account_id` in the key-issue idempotency fingerprint | Diverges: the fingerprint covers `name` and the expiry choice only, so one idempotency key replayed against a different client with an identical body resolves to the first key (§6.4). |
| The `LocalAdminStore` method surface | One method beyond the ledger: `cleanup_rate_buckets`, the bounded maintenance pass for `local_admin_rate_bucket`. The ledger's T4 surface was reservation + failure audit only, and the runtime scheduler needed a way to reach the store's maintenance pass through the same handle the routes use. |
| `LocalAdminService::resolve(request, cookie: &str)` | Takes the decoded `cookie_verifier: &[u8; 32]` instead. The cookie *format* (`__Host-` prefix, hex, duplicate-value rejection) is parsed once in the control layer by `control::local_admin::parse_admin_cookie`, so the service never depends on `control::csrf` — which the plan forbids. The extractor calls the service for the policy/epoch check, so the method is on the production path. |
| NEW-file names | The plan's inventory proposed `service/local_admin/clients.rs`, `control/local_admin/{mod,auth,clients}.rs` and `surreal_store/local_admin_clients.rs`. Shipped as `service/local_admin/client.rs`, `control/local_admin/{csrf,handlers}.rs` (the module itself is `control/local_admin.rs`, matching how every sibling control module is declared) and `surreal_store/local_admin{,_rate,_remote}.rs`. The split follows the code's seams (auth vs client, CSRF vs handlers, reservation vs maintenance) rather than the inventory's placeholder names. |
| `crates/memory-mcp/tests/local_admin_durable.rs` | Not created. The plan §2 inventory and the plan §6 `--test local_admin_durable` command predate the decision to keep the two remote-race cases inline. They assert "both replicas observe the same persisted row" through `SurrealRegistryStore::admin_query`, which is `pub(super)` — an external `tests/` target is a separate crate and cannot reach that statement seam. Both cases now live in `http/registry/surreal_store/local_admin_remote.rs`, where `cargo test -p memory_mcp --lib … -- --ignored` selects them (§2.2, §13.2). Everything else the plan's `tests/` inventory named exists: `tests/http_local_admin.rs`, `tests/local_admin_cli.rs` and `tests/http_control_plane.rs`. |
| "Require HTTPS public base URL for local browser auth" | Diverges for loopback only: `http://localhost`, `http://127.0.0.1` and `http://[::1]` are accepted so the documented local development flow and the in-repo end-to-end script can run without a proxy. Any other host must be `https://` (the check keys off the URL host, not a substring), so the `Secure` cookies cannot be served over public plain HTTP. |
| `FailureAudit.username_bucket` / `source_bucket` | Not projected into the audit row. Migration `047` (approved) gives `local_admin_audit` no bucket columns, and the buckets are already aggregated in `local_admin_rate_bucket`; the fields remain part of the event so the service describes a rejection fully. `policy` is likewise carried as a stale-epoch *attribute* and never re-validated, which is what keeps a stale-fence rejection from turning into an audit failure (§8.4). |
| UI routes | The bundle serves `/admin/login`, `/admin/activate`, `/admin/reset`, `/admin/reauth`, `/admin/clients` and `/admin/clients/:account_id` (§2.6, §4.2, §5). The spec's §9 lists the *flows* rather than fixed paths, and the two extra routes exist because the activation/reset and re-authentication flows need their own page: a code has to be entered somewhere, and a stale session must be able to re-enter its password without a full login. |
