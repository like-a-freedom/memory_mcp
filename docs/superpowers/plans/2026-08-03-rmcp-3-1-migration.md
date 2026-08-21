# rmcp 3.1 Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
> Status: Complete (rmcp 3.1.x in workspace)

**Goal:** Migrate `memory_mcp` from `rmcp` 2.2.0 to `rmcp` 3.1.0 while preserving synchronous tool behavior, making only `extract` task-capable through the official `io.modelcontextprotocol/tasks` extension, and keeping stdio protocol tests aligned with the new wire contract.

**Architecture:** Replace the project-owned `TaskStore` state machine with rmcp 3.1’s cloneable `TaskManager`. Add a manual `ServerHandler::call_tool` seam only for task-capable `extract` calls; all other calls continue through the generated `ToolRouter`. Keep resource construction and app-specific resource logic separate: internal helpers continue producing `ReadResourceResult`, while the protocol boundary returns the new `ReadResourceResponse`.

**Tech Stack:** Rust 2024 workspace, Cargo resolver 3, `rmcp` 3.1.0 (`macros`, `transport-io`, `server`), Tokio, serde/serde_json, stdio MCP transport, integration tests in `crates/memory-mcp/tests/tools_e2e.rs`.

## Global Constraints

- The dependency must be exactly `rmcp = { version = "3.1.0", features = ["macros", "transport-io", "server"] }`; do not add an rmcp feature that is not required by the migrated code.
- The workspace MSRV must be declared as `rust-version = "1.88"`, matching rmcp 3.x; regular CI jobs use `stable`, and a dedicated MSRV job uses Rust `1.88`.
- Existing packages must inherit the workspace MSRV with `rust-version.workspace = true`; package metadata must report Rust `1.88` for both workspace crates.
- Keep `hf-hub = { version = "0.5.0", default-features = false, features = ["tokio", "rustls-tls"] }` to avoid the Rust-incompatible `hf-xet`/`konst`/`redb` graph, and adapt the model loader to its `ApiBuilder`/`Repo::get` API.
- Keep `surrealdb = { version = "3.0.0", features = ["kv-rocksdb", "kv-mem"] }` for the Rust 1.88-compatible graph; preserve the locked Candle git revision `21cca0b1` when updating dependencies.
- The migration must follow the official guide at `https://github.com/modelcontextprotocol/rust-sdk/discussions/969`; the old experimental Tasks API has no compatibility shim.
- The official task extension identifier is `io.modelcontextprotocol/tasks`.
- Only `extract` may be materialized as a task; clients that do not advertise the extension must receive the ordinary synchronous `extract` response.
- Do not preserve the removed client task hint, `tasks/list`, `tasks/result`, `#[task_handler]`, or `#[tool(execution(task_support = ...))]` APIs.
- Use `rmcp::task_manager::TaskManager` for task lifecycle, payloads, TTL, polling state, `tasks/update`, and cooperative cancellation; do not adapt the deleted custom state machine to new wire types.
- Do not add custom related-task `_meta` to task creation or terminal payloads. The task ID is carried by the official task result and `tasks/get` state.
- Do not add cache hints, HTTP headers, discovery lifecycle changes, OAuth behavior, subscriptions, or distributed event storage; this server uses stdio and has no code in those guide areas.
- Do not add new dependencies. The dependency migration itself is approved; all other implementation must use existing workspace dependencies and rmcp 3.1 APIs.
- Keep `main.rs` thin, keep MCP behavior in `crates/memory-mcp/src/mcp/`, and use `ErrorData`/`Result` instead of `unwrap()` in production code.
- The existing custom 64-active/1024-retained task limits are intentionally removed. rmcp 3.1’s `TaskManager` owns retention but exposes no equivalent retained-capacity contract; documentation must not claim those old bounds after migration.

---

## Execution status

- [x] Tasks 1–6 implemented: dependency/MSRV setup, rmcp 3.1 TaskManager routing, resource result migration, unit contracts, stdio Tasks wire coverage, and documentation.
- [x] Task 7 compatibility fixes implemented: hf-hub 0.5 loader API, Rust 1.88-safe logging boundary, SurrealDB 3.0 safe FTS literals, and nested re-embedding cursor filtering.
- [x] Stable fmt, locked checks, strict clippy, workspace tests, metadata, dependency-tree, forbidden-production-API, wire-level resource coverage, and diff-hygiene gates completed.
- [ ] Commit steps are intentionally not executed because the user explicitly requested no commit.
- [ ] Native Rust 1.88 validation remains CI-authoritative on Ubuntu; local macOS ARM validation is blocked by Candle’s Rust-1.88-incompatible NEON implementation.

## Migration decisions and source contract

### Decisions

1. **Use the official Tasks extension, not a compatibility layer.** rmcp 3.1 removes `TasksCapability`, `TaskMetadata`, the old task request hint, `tasks/list`, `tasks/result`, `#[task_handler]`, and `execution(task_support = ...)`. The server advertises tasks with `.enable_tasks()` and the client capability is checked with `caps.supports_tasks()`.
2. **Use `TaskManager` directly.** `TaskManager::spawn(TaskOptions, closure)` creates durable task state before returning, embeds completed `CallToolResult` values under `tasks/get`, serializes failures under `error`, and exposes `get_task`, `update_task`, and `cancel_task`. The closure must return `Result<CallToolResult, TaskExit>` and must observe cancellation cooperatively with `TaskContext::cancelled()`.
3. **Keep the generated router for ordinary calls.** rmcp 3.1’s `#[tool_handler]` macro only generates `call_tool` when the impl does not already define it. A manual `call_tool` can therefore intercept task-capable `extract` and delegate every other request with `ToolCallContext::new(self, request, context)` followed by `self.tool_router.call(tcc).await`.
4. **Use one shared extraction helper.** The ordinary `extract` tool and the task closure must call the same helper returning `Result<ToolResponse<ExtractResult>, ErrorData>`. The ordinary path wraps it with `Json`; the task path serializes the same envelope into `CallToolResult::structured`, avoiding duplicated extraction behavior.
5. **Preserve internal resource helpers.** `read_resource_result` in `handlers/apps.rs` continues returning `ReadResourceResult` so existing app tests remain focused on app content. Only `ServerHandler::read_resource` widens to `Result<ReadResourceResponse, ErrorData>` and converts with `.into()`.
6. **Retain the legacy initialize lifecycle.** The server stays on stdio and continues using `serve()` in `cli/runtime.rs`; no discovery or inline-negotiation implementation is introduced. Raw E2E clients continue to exercise the current initialize flow while adding the tasks extension capability where needed.

### rmcp 3.1 source cross-check

The local user-supplied source path under `rusty_apple_mail_mcp` is not present. The checked source is the Cargo registry copy:

```text
/Users/solovey/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rmcp-3.1.0/
```

The plan is cross-checked against:

- `src/task_manager.rs`: `TaskManager`, `TaskOptions`, `TaskExit`, `TaskContext::cancelled`, `get_task`, `update_task`, `cancel_task`, SDK TTL/retention behavior.
- `src/model/task.rs`: `CreateTaskResult` with the task fields flattened at the result level; `DetailedTask` with flattened terminal `result`/`error`; `GetTaskResult`, `TaskAckResult`.
- `src/model/capabilities.rs`: extension-based `ClientCapabilities::supports_tasks`, `ServerCapabilities::supports_tasks`, and `.enable_tasks()`.
- `src/handler/server/tool.rs`: `ToolCallContext`, `IntoCallToolResult`, and generated router return conversion.
- `src/handler/server.rs`: task-capability validation and the `CallToolResponse` dispatch guard.
- `rmcp-3.1.0/tests/test_task.rs`: the official manual `call_tool` routing pattern and cooperative cancellation test.

### Migration-guide coverage matrix

| Guide area | Project action | Plan task |
|---|---|---|
| MRTR / SEP-2322 handler widening | Change manual `call_tool` to `Result<CallToolResponse, ErrorData>` and manual `read_resource` to `Result<ReadResourceResponse, ErrorData>`. No `InputRequiredResult` behavior is added. | Tasks 2–3 |
| MRTR client helpers and `requestState` | No rmcp client implementation exists in this repository; raw stdio tests do not use high-level rmcp clients. | N/A, recorded here |
| `resultType` on server results | Use rmcp constructors and conversions; avoid old struct literals missing new fields. Keep the existing legacy initialize protocol unless a test explicitly inspects modern discriminators. | Task 3 |
| Cache hints / client response cache | Leave `ttl_ms` and `cache_scope` as `None`; no rmcp client response cache is configured. | N/A, recorded here |
| Standard HTTP headers | No HTTP transport or `StreamableHttpService` exists; runtime uses `transport::io::stdio`. | N/A, recorded here |
| `Annotations.lastModified` | No project use was found. | N/A, recorded here |
| `ToolResultContent.structured_content` widening | The project uses `Json<T>` and `CallToolResult::structured` without treating structured content as an object in application code. | N/A, recorded here |
| Relaxed `outputSchema` roots | No source change required; existing `Json<T>` schemas remain valid. | N/A, recorded here |
| `Meta` split | Remove the custom task `Meta`/related-task metadata path. No request/notification metadata adapter is introduced. | Task 2 |
| Discovery / protocol negotiation | Keep `serve()` and default handler discovery behavior; no custom `ServerResult` or protocol-union matches require wildcard changes. | N/A, recorded here |
| Stateless HTTP / subscriptions | No HTTP session manager or resource subscription implementation exists. | N/A, recorded here |
| Official Tasks extension | Replace all old APIs with `TaskManager`, manual `call_tool`, `get_task`, `update_task`, and `cancel_task`; rewrite wire tests. | Task 2 and Task 4 |
| Distributed `EventStore` / SEP-2260 | No custom HTTP session serialization exists; stdio does not need event replay or cross-process request association. rmcp’s task manager receives the originating request context internally. | N/A, recorded here |
| OAuth startup / discovery / issuer / resource indicator | No OAuth integration exists. | N/A, recorded here |
| Rust MSRV | Add `rust-version = "1.88"`, keep regular CI on stable, and add a dedicated Rust 1.88 `cargo check --workspace --all-targets --locked` job. | Task 1 |
| Removed deprecated APIs | Compile errors from old task types and aliases are removed in the focused handler migration; run the full validation suite for any remaining rmcp 3.x API removals. | Tasks 1–5 |

## File structure map

Files that change together and their responsibilities:

- Modify `Cargo.toml`: workspace rmcp version/features, existing `hf-hub`/`surrealdb` MSRV-compatible versions, and Rust MSRV metadata.
- Modify `Cargo.lock`: generated rmcp 3.1.0/rmcp-macros 3.1.0 resolution, compatible `hf-hub`/`surrealdb` transitive checksums, and the existing Candle git revision.
- Modify `crates/memory-mcp/Cargo.toml` and `crates/eval-harness/Cargo.toml`: inherit the workspace Rust MSRV.
- Modify `.github/workflows/ci.yml`: keep regular CI jobs on stable Rust, enforce locked tests and metadata validation, and add a dedicated Rust 1.88 MSRV check.
- Modify `crates/memory-mcp/src/mcp.rs`: remove the custom `tasks` module declaration.
- Modify `crates/memory-mcp/src/mcp/handlers.rs`: own the `TaskManager`, implement the rmcp 3.1 task routing seam, widen the manual resource/call return types, and use new pagination constructors.
- Modify `crates/memory-mcp/src/mcp/handlers/apps.rs`: use rmcp 3.1 pagination constructors while retaining app-specific `ReadResourceResult` construction.
- Delete `crates/memory-mcp/src/mcp/tasks.rs`: remove the obsolete custom task lifecycle, TTL clamps, abort handles, old result/list APIs, and related-task metadata helpers.
- Modify `crates/memory-mcp/tests/tools_e2e.rs`: advertise the extension from task-capable clients and test the new `tools/call` → `tasks/get` → `tasks/update`/`tasks/cancel` contract.
- Modify `README.md`: describe the official extension and remove `tasks/list`, `tasks/result`, related metadata, and old capacity promises.
- Modify `docs/performance/NER_PERFORMANCE.md`: update the operational task description to rmcp 3.1’s SDK-owned lifecycle and wire methods.
- Modify `crates/memory-mcp/src/service/model_loader.rs`: adapt model downloads from hf-hub 1.0’s `HFClient` API to hf-hub 0.5’s Tokio `ApiBuilder`/`Repo::get` API while preserving the project cache layout.
- Modify `crates/memory-mcp/src/logging.rs`: use a Rust 1.88-compatible UTF-8 truncation boundary calculation instead of `str::floor_char_boundary`.
- Modify `crates/memory-mcp/src/storage/queries.rs`: keep FTS retrieval and `search::score` compatible with SurrealDB 3.0 by escaping only the MATCHES operand as a SurrealQL literal; keep scope, temporal, project, type, and limit values bound.
- Modify `crates/memory-mcp/src/storage/context_store.rs`: use the shared safe FTS literal helper for community summary matching.
- Modify `crates/memory-mcp/src/storage/reembed_store.rs`: use a stale-fact subquery before cursor filtering because SurrealDB 3.0 incorrectly eliminates rows for the combined predicate.

No changes are planned for `crates/memory-mcp/src/cli/runtime.rs`: it already uses the stdio transport and `ServerHandler::serve`, which is compatible with the migration.

---

## Implementation tasks

### Task 1: Update dependency, lockfile, and MSRV metadata

**Files:**
- Modify: `Cargo.toml:6-8,32`
- Modify: `Cargo.lock` (generated by Cargo)
- Modify: `.github/workflows/ci.yml:32-40,42-56,65-109,118-122,147-151,177-181,199-235` (stable toolchains plus the dedicated MSRV job)

**Interfaces:**
- Consumes: the existing workspace dependency declaration and CI setup jobs.
- Produces: rmcp 3.1.0 and rmcp-macros 3.1.0 in the locked dependency graph; workspace metadata declaring Rust 1.88.

- [ ] **Step 1: Update workspace metadata and the rmcp dependency.**

Change the workspace package section and dependency line to:

```toml
[workspace.package]
edition = "2024"
rust-version = "1.88"
license = "MIT"

[workspace.dependencies]
hf-hub = { version = "0.5.0", default-features = false, features = ["tokio", "rustls-tls"] }
rmcp = { version = "3.1.0", features = ["macros", "transport-io", "server"] }
surrealdb = { version = "3.0.0", features = ["kv-rocksdb", "kv-mem"] }
```

Do not add `client`, `local`, `request-state`, HTTP, OAuth, or cache features.

- [ ] **Step 2: Update the lockfile with Cargo.**

Run:

```bash
cargo update -p rmcp --precise 3.1.0
```

Expected: `Cargo.lock` records `rmcp` 3.1.0 and `rmcp-macros` 3.1.0, with Cargo updating only compatible transitive entries required by the new release.

- [ ] **Step 3: Keep regular CI on stable and add the MSRV check.**

Leave the regular `fmt`, `clippy`, `clippy_macos`, `test`, evaluation, and release-build jobs on:

```yaml
toolchain: stable
```

Add a separate job that exercises the declared MSRV:

```yaml
  msrv:
    name: Rust MSRV
    runs-on: ubuntu-latest
    timeout-minutes: 40
    steps:
      - uses: actions/checkout@v7.0.0
      - name: Set up Rust toolchain
        uses: actions-rust-lang/setup-rust-toolchain@v1.17.0
        with:
          toolchain: 1.88
          cache: true
      - name: Check MSRV
        run: cargo check --workspace --all-targets --locked
```

Add `msrv` to `build_binaries.needs` so release artifacts cannot build unless the MSRV check passes. Leave job names, feature matrices, targets, and other commands unchanged.

- [ ] **Step 4: Validate metadata before source migration.**

Run:

```bash
cargo metadata --locked --no-deps
cargo tree --locked -i rmcp@3.1.0
```

Expected: metadata succeeds, the inverse dependency tree names `memory_mcp` through the workspace dependency, no rmcp 2.2.0 package remains in the locked graph, and `cargo tree --locked` contains no `hf-xet`, `konst`, or `redb` path.

- [ ] **Step 5: Commit the dependency/toolchain slice.**

```bash
git add Cargo.toml Cargo.lock .github/workflows/ci.yml
git commit -m "build: prepare rmcp 3.1 migration"
```

### Task 2: Replace the custom task store with rmcp 3.1 TaskManager routing

**Files:**
- Modify: `crates/memory-mcp/src/mcp.rs:20-28`
- Modify: `crates/memory-mcp/src/mcp/handlers.rs:5-42,67-115,130-335,337-377`
- Delete: `crates/memory-mcp/src/mcp/tasks.rs`

**Interfaces:**
- Consumes: `MemoryService`, `ExtractParams`, `ToolResponse<ExtractResult>`, rmcp 3.1 `ToolCallContext`, `TaskManager`, `TaskOptions`, and `TaskExit`.
- Produces: `MemoryMcp { tasks: TaskManager }`; `ServerHandler::call_tool(...) -> Result<CallToolResponse, ErrorData>`; `get_task`, `update_task`, and `cancel_task` handlers; one shared `extract_response` helper.

- [ ] **Step 1: Remove the custom task module and state field.**

In `crates/memory-mcp/src/mcp.rs`, remove:

```rust
mod tasks;
```

In `MemoryMcp`, replace:

```rust
task_store: Arc<tokio::sync::Mutex<TaskStore>>,
```

with:

```rust
tasks: rmcp::task_manager::TaskManager,
```

Initialize it with `TaskManager::new()` in `MemoryMcp::new`. The rmcp `TaskManager` is already cloneable and internally shared, so no project `Arc<Mutex<...>>` is needed.

- [ ] **Step 2: Replace old rmcp task imports and enable the extension capability.**

Use imports equivalent to:

```rust
use rmcp::handler::server::tool::{ToolCallContext, ToolRouter};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, CancelTaskParams,
    CreateTaskResult, GetTaskParams, GetTaskResult, JsonObject, ListResourceTemplatesResult,
    ListResourcesResult, PaginatedRequestParams, ReadResourceRequestParams,
    ReadResourceResponse, ReadResourceResult, ServerCapabilities, ServerInfo, UpdateTaskParams,
};
use rmcp::task_manager::{TaskExit, TaskManager, TaskOptions};
```

Remove `CancelTaskResult`, `GetTaskPayloadParams`, `GetTaskPayloadResult`, `ListTasksResult`, `TasksCapability`, and every import from `super::tasks`.

Change the server capability builder to:

```rust
ServerCapabilities::builder()
    .enable_tools()
    .enable_resources()
    .enable_tasks()
    .build()
```

- [ ] **Step 3: Add one shared extraction helper.**

Add this private helper in `handlers.rs` near the `MemoryMcp` inherent methods:

```rust
async fn extract_response(
    service: Arc<MemoryService>,
    params: ExtractParams,
) -> Result<ToolResponse<ExtractResult>, ErrorData> {
    crate::tools::extract(&service.build_context(), params)
        .await
        .map_err(mcp_error)
}
```

Update the generated `extract` tool method to call the helper and preserve the current ordinary structured-output wrapper:

```rust
pub async fn extract(
    &self,
    params: Parameters<ExtractParams>,
) -> Result<Json<ToolResponse<ExtractResult>>, ErrorData> {
    extract_response(Arc::clone(&self.service), params.0)
        .await
        .map(Json)
}
```

Keep the existing description and `#[tool]` annotation, but remove `execution(task_support = "optional")` completely.

- [ ] **Step 4: Add the manual rmcp 3.1 `call_tool` task seam.**

Define this method inside the existing `#[tool_handler(router = self.tool_router)] impl ServerHandler for MemoryMcp` block. The presence of this method makes the rmcp macro retain generated `list_tools`/`get_tool` methods without generating a second `call_tool`:

```rust
async fn call_tool(
    &self,
    request: CallToolRequestParams,
    context: RequestContext<RoleServer>,
) -> Result<CallToolResponse, ErrorData> {
    let client_supports_tasks = context
        .client_capabilities()
        .is_some_and(|caps| caps.supports_tasks());

    if request.name == "extract" && client_supports_tasks {
        let params: ExtractParams = serde_json::from_value(serde_json::Value::Object(
            request.arguments.clone().unwrap_or_default(),
        ))
        .map_err(|error| {
            ErrorData::invalid_params(
                format!("failed to deserialize parameters: {error}"),
                None,
            )
        })?;
        let service = Arc::clone(&self.service);
        let task = self.tasks.spawn(
            TaskOptions::new().with_status_message("Task accepted"),
            move |ctx| {
                Box::pin(async move {
                    tokio::select! {
                        _ = ctx.cancelled() => Err(TaskExit::Cancelled),
                        result = extract_response(service, params) => {
                            let response = result.map_err(TaskExit::Error)?;
                            let structured = serde_json::to_value(response).map_err(|error| {
                                TaskExit::Error(ErrorData::internal_error(
                                    format!("failed to serialize extract task result: {error}"),
                                    None,
                                ))
                            })?;
                            Ok(CallToolResult::structured(structured))
                        }
                    }
                })
            },
        );
        return Ok(CallToolResponse::Task(CreateTaskResult::new(task)));
    }

    let tcc = ToolCallContext::new(self, request, context);
    self.tool_router.call(tcc).await
}
```

The task path must not parse or preserve a removed `request.task` field. The absence of the tasks capability must select the ordinary router path, not an error path.

- [ ] **Step 5: Replace old task handlers with the official extension handlers.**

Delete `enqueue_task`, `list_tasks`, `get_task_info`, `get_task_result`, and the old custom `cancel_task`. Add these methods:

```rust
async fn get_task(
    &self,
    request: GetTaskParams,
    _context: RequestContext<RoleServer>,
) -> Result<GetTaskResult, ErrorData> {
    Ok(GetTaskResult::new(self.tasks.get_task(&request.task_id)?))
}

async fn update_task(
    &self,
    request: UpdateTaskParams,
    _context: RequestContext<RoleServer>,
) -> Result<(), ErrorData> {
    self.tasks
        .update_task(&request.task_id, request.input_responses)
}

async fn cancel_task(
    &self,
    request: CancelTaskParams,
    _context: RequestContext<RoleServer>,
) -> Result<(), ErrorData> {
    self.tasks.cancel_task(&request.task_id)
}
```

These exact return types let rmcp serialize `tasks/update` and `tasks/cancel` as empty `TaskAckResult` responses. Do not return `CancelTaskResult` or a project cancellation error code.

- [ ] **Step 6: Delete the obsolete implementation and its unit tests.**

Delete `crates/memory-mcp/src/mcp/tasks.rs` and its module declaration. This removes custom TTL normalization, custom task IDs, abort handles, custom payload state, active/retained capacity checks, `tasks/list`, `tasks/result`, `Meta` construction, and the `-32800` cancellation error. The SDK-generated UUID task IDs and SDK retention policy become the source of truth.

- [ ] **Step 7: Run the focused compile check.**

Run:

```bash
cargo check -p memory_mcp
```

Expected: the task-related rmcp errors are resolved; the only expected compile errors at this checkpoint are the known `ReadResourceResponse` and rmcp 3.1 pagination updates completed in Task 3.

- [ ] **Step 8: Commit the task integration slice.**

```bash
git add crates/memory-mcp/src/mcp.rs crates/memory-mcp/src/mcp/handlers.rs
git rm crates/memory-mcp/src/mcp/tasks.rs
git commit -m "refactor: use rmcp task manager extension"
```

### Task 3: Migrate resource result types and pagination constructors

**Files:**
- Modify: `crates/memory-mcp/src/mcp/handlers.rs:8-12,290-335`
- Modify: `crates/memory-mcp/src/mcp/handlers/apps.rs:6-8,69-86`
- Modify: `crates/memory-mcp/tests/tools_e2e.rs:500-542` (feature-gated stdio resource wire regression)

**Interfaces:**
- Consumes: existing `read_resource_result(...) -> Result<ReadResourceResult, ErrorData>` app helper and app catalog vectors.
- Produces: `ServerHandler::read_resource(...) -> Result<ReadResourceResponse, ErrorData>`; rmcp 3.1-compatible list result values with `result_type`, `ttl_ms`, and `cache_scope` initialized by constructors; a wire-level `resources/list`/`resources/read` regression under `mcp-apps`.

- [ ] **Step 1: Widen the manual resource handler return type.**

Change the handler signature and convert the existing helper at the protocol boundary:

```rust
async fn read_resource(
    &self,
    request: ReadResourceRequestParams,
    _context: RequestContext<RoleServer>,
) -> Result<ReadResourceResponse, ErrorData> {
    self.read_resource_result(request).await.map(Into::into)
}
```

Keep `read_resource_result` returning `ReadResourceResult`; this avoids changing app payload tests and keeps the rmcp MRTR widening at the actual `ServerHandler` boundary.

- [ ] **Step 2: Replace non-app direct list result literals.**

Use constructors that initialize all rmcp 3.1 fields:

```rust
#[cfg(not(feature = "mcp-apps"))]
{
    Ok(ListResourcesResult::with_all_items(Vec::new()))
}
```

```rust
#[cfg(not(feature = "mcp-apps"))]
{
    Ok(ListResourceTemplatesResult::with_all_items(Vec::new()))
}
```

Do not add TTL or cache-scope hints; the constructors leave both optional fields unset.

- [ ] **Step 3: Replace app list result literals.**

In `handlers/apps.rs`, change the two constructors to:

```rust
pub(super) fn list_resources_result() -> ListResourcesResult {
    ListResourcesResult::with_all_items(app_catalog_resources())
}

pub(super) fn list_resource_templates_result() -> ListResourceTemplatesResult {
    ListResourceTemplatesResult::with_all_items(app_resource_templates())
}
```

Leave the `ReadResourceResult::new(...)` calls in `read_resource_result` unchanged.

- [ ] **Step 4: Update resource-focused tests for the widened boundary.**

Keep the existing direct app tests calling `read_resource_result` and matching `ResourceContents`; those tests intentionally exercise the internal `ReadResourceResult` helper. Keep the feature-gated `test_mcp_resources_read_over_stdio` test in `tools_e2e.rs`; it initializes a server with `mcp-apps`, verifies `resources/list` exposes `ui://memory/apps`, and verifies `resources/read` returns one JSON `ResourceContents` item through the widened protocol response. Do not add an `InputRequiredResult` branch: this server returns only completed resource results. Run the existing resource/app test subset and the focused wire test after the signature change so compilation proves the protocol boundary conversion and the direct helper tests prove the app payload contract.

- [ ] **Step 5: Validate both feature configurations.**

Run:

```bash
cargo check -p memory_mcp
cargo check -p memory_mcp --features mcp-apps
```

Expected: no direct struct literal errors for resource/list results and no `ReadResourceResult`/`ReadResourceResponse` trait mismatch.

- [ ] **Step 6: Commit the resource migration slice.**

```bash
git add crates/memory-mcp/src/mcp/handlers.rs crates/memory-mcp/src/mcp/handlers/apps.rs
git commit -m "refactor: migrate rmcp resource responses"
```

### Task 4: Update unit-level capability and routing expectations

**Files:**
- Modify: `crates/memory-mcp/src/mcp/handlers.rs:669-712`

**Interfaces:**
- Consumes: `MemoryMcp::build_server_info`, generated tool lookup, and the protocol-level task behavior covered by the E2E process.
- Produces: unit assertions for the extension-shaped server capability; no references to removed `TaskSupport` or `Tool::task_support()` APIs.

- [ ] **Step 1: Rewrite the server capability assertion.**

Replace the old `capabilities["tasks"]` assertions with:

```rust
assert!(capabilities["extensions"]["io.modelcontextprotocol/tasks"].is_object());
assert!(capabilities.get("tasks").is_none());
```

Keep the existing tools/resources/instructions assertions.

- [ ] **Step 2: Remove the obsolete generated-tool task-support test.**

Delete `only_extract_allows_task_execution`, including its `TaskSupport` import and calls to `Tool::task_support()`. rmcp 3.1 intentionally removes that tool annotation/API; task eligibility is now server routing behavior based on the request name and client extension capability.

- [ ] **Step 3: Preserve ordinary tool lookup coverage.**

Keep or add this focused assertion so the manual `call_tool` did not suppress the macro-generated router methods:

```rust
#[tokio::test]
async fn tool_router_still_exposes_extract_and_non_task_tools() {
    let mcp = create_test_mcp().await;
    assert!(mcp.get_tool("extract").is_some());
    assert!(mcp.get_tool("ingest").is_some());
}
```

The E2E test in Task 5 is the authoritative check that only task-capable `extract` is asynchronous and that `ingest` remains synchronous.

- [ ] **Step 4: Run the handler unit tests.**

```bash
cargo test -p memory_mcp mcp::handlers::tests --lib
```

Expected: capability, schema, router, and existing handler tests pass without old task-support references.

- [ ] **Step 5: Commit the unit contract slice.**

```bash
git add crates/memory-mcp/src/mcp/handlers.rs
git commit -m "test: assert rmcp tasks extension capability"
```

### Task 5: Rewrite stdio E2E coverage for the official Tasks wire contract

**Files:**
- Modify: `crates/memory-mcp/tests/tools_e2e.rs:24-161,292-555`

**Interfaces:**
- Consumes: raw JSON-RPC stdio helper, ordinary `tools/call`, rmcp 3.1 extension capability, `tasks/get`, `tasks/update`, and `tasks/cancel`.
- Produces: regression coverage for plain-client synchronous extraction, task-capable extraction, flattened task creation, embedded terminal payloads/errors, SDK cooperative cancellation, and removed old methods.

- [ ] **Step 1: Make initialization optionally advertise the tasks extension.**

Replace the fixed empty capability map with a helper that accepts a boolean:

```rust
fn initialize(&mut self, task_capable: bool) {
    let client_capabilities = if task_capable {
        serde_json::json!({
            "extensions": {
                "io.modelcontextprotocol/tasks": {}
            }
        })
    } else {
        serde_json::json!({})
    };

    let response = self.request(
        1,
        "initialize",
        serde_json::json!({
            "protocolVersion": "2025-11-25",
            "capabilities": client_capabilities,
            "clientInfo": {"name": "memory-mcp-e2e", "version": "1.0.0"}
        }),
    );
    let capabilities = &response["result"]["capabilities"];
    assert!(
        capabilities["extensions"]["io.modelcontextprotocol/tasks"].is_object(),
        "server must advertise the tasks extension: {response}"
    );
    self.notify("notifications/initialized", serde_json::json!({}));
}
```

Update every existing call site to `initialize(false)` unless the test explicitly needs asynchronous extraction. Do not send a removed top-level `task` request field.

- [ ] **Step 2: Make task polling use `tasks/get`’s flattened state.**

Keep the terminal statuses `completed`, `failed`, and `cancelled`, but read them from `response["result"]["status"]`. Use the SDK-provided `pollIntervalMs` from the create response when sleeping; use `1_000` milliseconds as the test fallback because rmcp 3.1’s default `TaskOptions` polling interval is 1 second:

```rust
fn wait_for_terminal_task(
    &mut self,
    request_id: &mut i64,
    task_id: &str,
    poll_interval_ms: u64,
) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(Instant::now() < deadline, "task {task_id} did not complete before the E2E deadline");
        let status = self.request(
            *request_id,
            "tasks/get",
            serde_json::json!({"taskId": task_id}),
        );
        *request_id += 1;
        match status["result"]["status"].as_str() {
            Some("completed" | "failed" | "cancelled") => return status,
            Some("working" | "input_required") => {
                std::thread::sleep(Duration::from_millis(poll_interval_ms));
            }
            other => panic!("unexpected task state {other:?}: {status}"),
        }
    }
}
```

Remove the old `task_payload_for_sync_result` helper entirely.

- [ ] **Step 3: Split the lifecycle test into plain and task-capable clients.**

Use a plain client to establish the synchronous reference:

```rust
let mut synchronous_client = StdioMcpProcess::start();
synchronous_client.initialize(false);
```

Ingest the episode and call `extract` normally. Assert the response is an ordinary result and retain its `structuredContent` value as the reference payload.

Use a separate task-capable process for the task lifecycle:

```rust
let mut task_client = StdioMcpProcess::start();
task_client.initialize(true);
```

Ingest the same logical input and call `extract` without task metadata. Assert the creation response uses the flattened rmcp 3.1 shape:

```rust
let created = task_client.request(4, "tools/call", serde_json::json!({
    "name": "extract",
    "arguments": {"episode_id": episode_id}
}));
let task_id = created["result"]["taskId"]
    .as_str()
    .unwrap_or_else(|| panic!("missing flattened task id: {created}"))
    .to_string();
assert!(created["result"]["task"].is_null());
assert_eq!(created["result"]["status"], "working");
let created_at = created["result"]["createdAt"].clone();
let poll_interval_ms = created["result"]["pollIntervalMs"]
    .as_u64()
    .unwrap_or(1_000);
```

Do not assert a related-task `_meta` object. The task ID is already in the official task result.

- [ ] **Step 4: Assert the completed payload is embedded in `tasks/get`.**

Poll with the helper and assert:

```rust
let completed = task_client.wait_for_terminal_task(
    &mut request_id,
    &task_id,
    poll_interval_ms,
);
assert_eq!(completed["result"]["status"], "completed");
assert_eq!(completed["result"]["createdAt"], created_at);
assert_eq!(
    completed["result"]["result"]["structuredContent"],
    synchronous_structured_content,
);
assert!(completed["result"]["error"].is_null());
```

Compare `structuredContent` rather than the whole serialized `CallToolResult`, because result discriminators are version-gated and the task payload is nested in the `tasks/get` detailed task.

- [ ] **Step 5: Cover `tasks/update` and unknown task handling.**

Send an empty input response set and assert the official empty acknowledgement:

```rust
let update_ack = task_client.request(
    request_id,
    "tasks/update",
    serde_json::json!({"taskId": task_id, "inputResponses": {}}),
);
request_id += 1;
assert!(update_ack["result"].is_object());
```

For a missing ID, call each official task method with its complete request shape and assert `-32602`. `tasks/update` requires the `inputResponses` object even when it is empty:

```rust
for method in ["tasks/get", "tasks/update", "tasks/cancel"] {
    let params = match method {
        "tasks/update" => {
            serde_json::json!({"taskId": "missing-task", "inputResponses": {}})
        }
        "tasks/get" | "tasks/cancel" => serde_json::json!({"taskId": "missing-task"}),
        _ => unreachable!("method list is fixed above"),
    };
    let response = task_client.request(request_id, method, params);
    request_id += 1;
    assert_eq!(response["error"]["code"], -32602);
}
```

The expected SDK message is `unknown task: missing-task`; assert that exact substring when checking the `tasks/get` response. A malformed `tasks/update` request without `inputResponses` is rejected during request decoding and can appear as `-32601`, so the test must not omit that required field.

- [ ] **Step 6: Cover failed task payloads without `tasks/result`.**

Call task-capable `extract` with a missing episode, poll `tasks/get`, and assert:

```rust
assert_eq!(failed["result"]["status"], "failed");
assert_eq!(
    failed["result"]["error"]["code"],
    synchronous_failure["error"]["code"],
);
assert!(failed["result"]["result"].is_null());
```

The failure is embedded in the detailed task’s `error` field. Do not fetch it through `tasks/result`.

- [ ] **Step 7: Cover cooperative cancellation correctly.**

Start a long-running task-capable extraction, call `tasks/cancel`, assert the response is an acknowledgement rather than a terminal task object, then poll `tasks/get` until one of `completed`, `failed`, or `cancelled` is returned. Assert the original `createdAt` remains stable. Do not require immediate cancellation or the removed `-32800` error: rmcp 3.1 cancellation is cooperative and the task may legally reach another terminal status.

- [ ] **Step 8: Assert ordinary clients remain synchronous and old methods are absent.**

In the plain client path, call `extract` without the extension and assert `result.structuredContent` is present while `result.taskId` is absent. Call `ingest` from the task-capable client and assert it remains a normal completed tool result. Do not issue the removed `tasks/list` or `tasks/result` calls as part of the positive lifecycle.

Send `tasks/list` and `tasks/result` as raw JSON-RPC requests and assert `-32601 Method not found` for each. This confirms the server does not accidentally expose the removed APIs; it must not reintroduce any implementation for them.

- [ ] **Step 9: Run the focused stdio regression test.**

```bash
cargo test -p memory_mcp --test tools_e2e test_mcp_extract_task_lifecycle_over_stdio -- --nocapture
```

Expected: the plain client receives synchronous extraction, the task-capable client receives a flattened task handle, terminal result/error fields are read from `tasks/get`, and cancellation is accepted without an immediate-terminal assertion.

- [ ] **Step 10: Commit the wire-contract test slice.**

```bash
git add crates/memory-mcp/tests/tools_e2e.rs
git commit -m "test: migrate task lifecycle to rmcp extension"
```

### Task 6: Update user-facing task documentation

**Files:**
- Modify: `README.md:742-749`
- Modify: `docs/performance/NER_PERFORMANCE.md:149-165`

**Interfaces:**
- Consumes: the new rmcp 3.1 wire behavior and SDK task defaults.
- Produces: documentation that names only the official task methods and does not promise removed metadata or custom capacity behavior.

- [ ] **Step 1: Replace the README Tasks section.**

Use wording equivalent to the following, preserving the project’s existing Markdown style:

```markdown
### MCP Tasks (optional)

The server advertises the official `io.modelcontextprotocol/tasks` extension.
`extract` is the only task-capable tool. A client that advertises the extension
calls `extract` through ordinary `tools/call` and receives a task handle with
`taskId`, `status`, timestamps, TTL, and a suggested polling interval at the
result level. Poll `tasks/get` until the task is terminal; completed payloads are
embedded in the detailed task’s `result` field and failed payloads in its `error`
field. `tasks/update` is available for input responses and `tasks/cancel` requests
cooperative cancellation. Task listing and a separate terminal-result request are
not part of this extension contract.

Clients that do not advertise `io.modelcontextprotocol/tasks` continue to receive
synchronous `extract` results. rmcp’s `TaskManager` supplies the default five-minute
TTL, polling metadata, lifecycle, and retention behavior.
```

- [ ] **Step 2: Replace the performance task description.**

Replace the section body with:

```markdown
## MCP Tasks

Task-capable clients advertise `io.modelcontextprotocol/tasks`. The server
materializes only `extract` from an ordinary `tools/call`; clients poll
`tasks/get`, send input responses through `tasks/update`, and request
cooperative cancellation through `tasks/cancel`. The detailed task returned by
`tasks/get` embeds a completed tool payload under `result` or a failed payload
under `error`. Task listing and a separate terminal-result request are not part
of the extension contract. Clients without the extension continue to receive
synchronous tool results.

The MCP adapter delegates task lifecycle, TTL, polling, retention, and
cooperative cancellation to rmcp 3.1 `TaskManager`.
```

Remove the old related-task metadata, custom lifecycle-store, `-32800` cancellation,
and fixed 64-active/1024-retained-bound claims.

- [ ] **Step 3: Check the documentation for removed API names.**

Run:

```bash
python3 -c "from pathlib import Path; paths=[Path('README.md'), Path('docs/performance/NER_PERFORMANCE.md')]; text='\\n'.join(p.read_text() for p in paths); forbidden=['tasks/list','tasks/result','related-task','64 active','1024 retained','-32800']; print([term for term in forbidden if term in text])"
```

Expected: `[]`.

- [ ] **Step 4: Commit the documentation slice.**

```bash
git add README.md docs/performance/NER_PERFORMANCE.md
git commit -m "docs: describe rmcp tasks extension"
```

### Task 7: Run the complete migration validation gate

**Files:**
- No source changes expected; fix only migration regressions in the files listed above before re-running validation.

**Interfaces:**
- Consumes: all migrated source, tests, lockfile, documentation, and CI metadata.
- Produces: a clean formatted, compiled, tested, and linted rmcp 3.1 workspace.

- [x] **Step 1: Format the migrated Rust code and compatibility fixes.**

```bash
cargo fmt --all
cargo fmt --all --check
```

Expected: the check reports no diff.

- [x] **Step 2: Run the required compile commands.**

```bash
cargo check --workspace --all-targets --locked
cargo check --workspace --all-targets --all-features --locked
cargo build
cargo metadata --locked --no-deps
```

Expected: all commands compile the workspace with rmcp 3.1.0, the locked dependency graph is Rust 1.88-compatible, and metadata reports `rust_version: "1.88"` for both packages.

- [x] **Step 3: Run the production crate tests.**

```bash
cargo test -p memory_mcp
```

Expected: unit, binary, integration, and stdio E2E tests pass, including the rewritten task lifecycle, SurrealDB 3.0 FTS retrieval, and re-embedding cursor pagination.

Also run the workspace suite without early exit:

```bash
cargo test --workspace --all-targets --no-fail-fast
```

- [x] **Step 4: Run the repository’s required clippy gate.**

```bash
cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings
```

Expected: zero warnings and zero errors. In particular, no stale rmcp 2.x imports, exhaustive match failures, or dead custom-task code may remain. Run this on stable Rust; the separate CI job is the authoritative native Rust 1.88 check.

- [x] **Step 5: Confirm the old task API is absent from source.**

Run:

```bash
python3 -c "from pathlib import Path; root=Path('crates/memory-mcp/src'); text='\\n'.join(p.read_text() for p in root.rglob('*.rs')); forbidden=['TasksCapability','TaskSupport','GetTaskPayload','ListTasksResult','task_support','enqueue_task','get_task_info','get_task_result','execution(task_support','tasks/list','tasks/result','related_task_metadata','TaskStore','-32800']; print([term for term in forbidden if term in text])"
```

Expected: `[]`. The production-source scan intentionally excludes `crates/memory-mcp/tests`, where `tools_e2e.rs` mentions `tasks/list` and `tasks/result` only as negative requests that assert `-32601`; those are not server API implementations. This scan does not inspect Cargo’s registry source.

- [x] **Step 6: Review the final diff and leave the requested changes uncommitted.**

```bash
git --no-pager diff --check
git --no-pager status --short
```

Expected: no whitespace errors, only the planned files changed, and no generated target artifacts are staged. No commit is created because the user explicitly requested an uncommitted working tree. The final working tree should contain only the planned migration, CI, test, documentation, and plan files.

## Self-review performed

- [x] Every rmcp migration-guide section is mapped to a source change or explicitly marked N/A for this stdio-only server.
- [x] The plan uses `CallToolResponse`, `ReadResourceResponse`, `TaskManager`, `TaskOptions`, `TaskExit`, `GetTaskParams`, `GetTaskResult`, `UpdateTaskParams`, and `CancelTaskParams` exactly as exposed by rmcp 3.1.0.
- [x] The plan does not describe the old nested `result.task` creation shape; task fields are top-level under the JSON-RPC result, and `tasks/get` terminal payloads are under `result` or `error` in the flattened detailed task.
- [x] The plan removes, rather than adapts, `tasks/list`, `tasks/result`, the old task hint, `execution(task_support)`, `TaskSupport`, and `Meta`-based related-task metadata.
- [x] The plan preserves ordinary router dispatch and synchronous behavior for clients without the tasks extension.
- [x] The plan explicitly handles cooperative cancellation and does not require an immediate `cancelled` status.
- [x] The plan removes outdated 64-active/1024-retained promises instead of silently claiming rmcp 3.1 preserves them.
- [x] All tasks name exact files, interfaces, code shapes, test commands, and commit commands; every implementation step is specified.
- [x] The required validation commands from `AGENTS.md` are included verbatim, and CI now enforces locked tests plus locked metadata validation.
- [x] MSRV review fixes are documented: package-level inheritance, hf-hub 0.5 API adaptation, SurrealDB 3.0 dependency selection, safe literal FTS operands with bound ordinary filters, nested re-embedding cursor filtering, logging boundary compatibility, and Candle lock preservation.
- [x] Final review follow-ups are closed: stable CI tests use `--locked`, CI has an explicit metadata job, and `resources/read` has feature-gated stdio wire coverage.
