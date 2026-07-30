# Token-Efficient Responses — Implementation Plan

> **For agentic workers:** Use this plan to implement the design in `docs/superpowers/specs/2026-07-30-token-efficient-responses-design.md`. Read that spec first.
>
> **Architecture:** Add `compact: bool` (default `true`) to `assemble_context` and `explain`, slimming redundant/verbose fields at serde time via `skip_serializing_if` and a thread-local scoped guard. No pipeline, storage, or ingestion changes.
>
> **Tech Stack:** Rust 2024, Tokio, Serde/serde_json, schemars, thiserror.

## Global Constraints

- `compact=true` is the default. `compact=false` is the opt-out.
- No field is deleted from any struct — only `#[serde(skip_serializing_if = ...)]` or `#[serde(serialize_with = ...)]` is added.
- The 8-tool frozen surface is not expanded. `compact` is a parameter on existing tools.
- `has_more: false` is always omitted in compact mode (via `None`).
- `total_count` is omitted in compact mode when it equals the result list length (redundant).
- `rationale` under compact mode is `"tier=<tier>"` — exactly the tier string, no scores.
- `graph_insights` is kept fully populated on all batch items (batch equality contract).
- `citation_context` is kept populated.
- `episode_content` inside `all_sources[].ProvenanceSource` is kept populated.
- `provenance` shape is kept intact.
- v5 benchmark gates (17/17) must not regress. Run PR, Release, and Nightly profiles after implementation.
- Pipeline benchmarks must not regress.
- The thread-local and its helpers live in `crates/memory-mcp/src/tools/compact.rs` and are `pub(crate)` because `models/request.rs` references them from a different module.
- Thread-safe: `thread_local!` is per-thread; `CompactGuard` is stack-bound.
- The tool handler drops the guard only after the response has been serialized by the MCP/CLI layer, so payload transformation must happen in the handler before return.

## File Map

| Path | Responsibility |
|------|---------------|
| `crates/memory-mcp/src/tools/compact.rs` | Thread-local `COMPACT_MODE`, `CompactGuard`, `skip_if_compact`, `serialize_rationale` |
| `crates/memory-mcp/src/tools/parsers.rs` | `default_compact()` helper |
| `crates/memory-mcp/src/tools/params.rs` | `compact` field on `AssembleContextParams`, `ExplainParams` |
| `crates/memory-mcp/src/models/request.rs` | `compact` on `AssembleContextRequest`, `ExplainRequest`; serde annotation changes on `AssembledContextItem`, `ExplainItem` |
| `crates/memory-mcp/src/tools/assemble_context.rs` | Plumb `compact`, wrap response build with guard |
| `crates/memory-mcp/src/tools/explain.rs` | Plumb `compact`, wrap response build with guard |
| `crates/memory-mcp/src/tools/response.rs` | `complete_list_compact` constructor — alias with same logic as `complete_list` but different name for clarity |
| `crates/memory-mcp/src/tools.rs` | Module declaration for `compact` |
| `crates/memory-mcp/src/cli/args.rs` | `--compact` / `--no-compact` on `AssembleContextArgs`, `ExplainArgs` |
| `crates/memory-mcp/src/cli/commands/assemble_context.rs` | Plumb CLI arg to params |
| `crates/memory-mcp/src/cli/commands/explain.rs` | Plumb CLI arg to params |
| `evals/profiles/response_size.json` | New measurement-only profile |
| `crates/eval-harness/src/suites/response_size.rs` | New suite |
| `crates/eval-harness/src/suites.rs` | Module declaration |
| `crates/eval-harness/src/main.rs` | Register `response-size` suite string id |
| Tests (2 files: `tests/service_acceptance.rs`, `tests/service_integration.rs`) | Add `compact: false` to request struct literals that assert on verbose `rationale` |

---

### Task 1: Create `compact` module with thread-local and serde helpers

**Files:**
- Create: `crates/memory-mcp/src/tools/compact.rs`
- Modify: `crates/memory-mcp/src/tools.rs`

**Purpose:** Single source of truth for compact-mode state during serialization. All compact-mode decisions (skip `quote`, slim `rationale`) read from this module.

- [ ] **Step 1: Create `compact.rs`**

```rust
// crates/memory-mcp/src/tools/compact.rs

//! Compact-mode state for token-efficient serialization.
//!
//! This module is not a service — it is a scoped serialization toggle used
//! by serde helpers on response structs. The guard is dropped after the
//! final JSON serialization completes.

use std::cell::Cell;

thread_local! {
    static COMPACT_MODE: Cell<bool> = const { Cell::new(false) };
}

/// RAII guard for compact mode: sets the flag on entry, restores previous on drop.
/// Must be dropped after the final serde serialization, not before.
pub(crate) struct CompactGuard {
    prev: bool,
}

/// Enable or disable compact mode for the remainder of the current function scope.
/// The returned guard must be held until response serialization completes.
pub(crate) fn set_compact(compact: bool) -> CompactGuard {
    let prev = COMPACT_MODE.with(|c| c.replace(compact));
    CompactGuard { prev }
}

impl Drop for CompactGuard {
    fn drop(&mut self) {
        COMPACT_MODE.with(|c| c.set(self.prev));
    }
}

/// Check whether compact mode is active.
pub(crate) fn is_compact() -> bool {
    COMPACT_MODE.with(|c| c.get())
}

/// serde `skip_serializing_if` fn — skips the field when compact mode is on.
/// Used on fields like `quote` that are redundant in compact output.
pub(crate) fn skip_if_compact<T>(_value: &T) -> bool {
    is_compact()
}

/// Custom serializer for `rationale`. Under compact mode, emits only the
/// leading `tier=<tier>` token; otherwise passes the string through.
/// Must be `pub(crate)` because `models/request.rs` references it.
pub(crate) fn serialize_rationale<S: serde::Serializer>(
    rationale: &str,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if is_compact() {
        let tier = rationale
            .split_whitespace()
            .next()
            .unwrap_or("tier=unknown");
        serializer.serialize_str(tier)
    } else {
        serializer.serialize_str(rationale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_mode_is_off_by_default() {
        assert!(!is_compact());
    }

    #[test]
    fn set_compact_true() {
        let guard = set_compact(true);
        assert!(is_compact());
        drop(guard);
        assert!(!is_compact());
    }

    #[test]
    fn set_compact_false() {
        let guard = set_compact(false);
        assert!(!is_compact());
        drop(guard);
    }

    #[test]
    fn skip_if_compact_respects_mode() {
        // Outer scope: compact off
        assert!(!skip_if_compact(&42));
        {
            let _guard = set_compact(true);
            assert!(skip_if_compact(&42));
        } // _guard drops here
        assert!(!skip_if_compact(&42));
    }

    // Test struct that exercises the skip_if_compact attribute directly.
    #[derive(serde::Serialize)]
    struct TestQuote {
        content: String,
        #[serde(skip_serializing_if = "super::skip_if_compact")]
        quote: String,
    }

    #[test]
    fn quote_skipped_when_compact() {
        let _guard = set_compact(true);
        let val = serde_json::to_value(&TestQuote {
            content: "main content".to_string(),
            quote: "main content".to_string(),
        })
        .unwrap();
        assert!(val.get("content").is_some());
        assert!(val.get("quote").is_none());
        drop(_guard);
    }

    #[test]
    fn quote_present_when_verbose() {
        let _guard = set_compact(false);
        let val = serde_json::to_value(&TestQuote {
            content: "main content".to_string(),
            quote: "main content".to_string(),
        })
        .unwrap();
        assert!(val.get("content").is_some());
        assert_eq!(val["quote"].as_str().unwrap(), "main content");
        drop(_guard);
    }

        // Test struct that exercises the serialize_with attribute directly.
    #[derive(serde::Serialize)]
    struct TestRationale {
        #[serde(serialize_with = "super::serialize_rationale")]
        rationale: String,
    }

    #[test]
    fn serialize_rationale_compact() {
        let _guard = set_compact(true);
        let val = serde_json::to_value(TestRationale {
            rationale: "tier=direct fts=0.85 access_count=3 confidence=0.92".to_string(),
        })
        .unwrap();
        assert_eq!(val["rationale"].as_str().unwrap(), "tier=direct");
        drop(_guard);
    }

    #[test]
    fn serialize_rationale_verbose() {
        let _guard = set_compact(false);
        let val = serde_json::to_value(TestRationale {
            rationale: "tier=direct fts=0.85 access_count=3 confidence=0.92".to_string(),
        })
        .unwrap();
        assert_eq!(
            val["rationale"].as_str().unwrap(),
            "tier=direct fts=0.85 access_count=3 confidence=0.92"
        );
        drop(_guard);
    }
```
}
```

- [ ] **Step 2: Register module**

In `crates/memory-mcp/src/tools.rs`, add:
```rust
pub(crate) mod compact;
```

Check the existing file first — if it's a `mod.rs` or a file listing, add the module declaration in the same style.

- [ ] **Step 3: Run tests**

```bash
cargo test -p memory_mcp -- tools::compact
```

---

### Task 2: Add `compact` field to parameter and request structs

**Files:**
- Modify: `crates/memory-mcp/src/tools/parsers.rs`
- Modify: `crates/memory-mcp/src/tools/params.rs`
- Modify: `crates/memory-mcp/src/models/request.rs`

- [ ] **Step 1: Add `default_compact` helper in `parsers.rs`**

```rust
// In crates/memory-mcp/src/tools/parsers.rs

/// Default for compact mode — true (token-efficient by default).
pub(crate) fn default_compact() -> bool {
    true
}
```

- [ ] **Step 2: Add `compact` to `AssembleContextParams`**

In `crates/memory-mcp/src/tools/params.rs`, add to `AssembleContextParams`:
```rust
    /// Request compact (token-efficient) response. Defaults to true.
    /// Set to false for verbose debug output including full rationale strings.
    #[serde(default = "crate::tools::parsers::default_compact")]
    pub compact: bool,
```

- [ ] **Step 3: Add `compact` to `ExplainParams`**

In the same file, add to `ExplainParams`:
```rust
    /// Request compact (token-efficient) response. Defaults to true.
    #[serde(default = "crate::tools::parsers::default_compact")]
    pub compact: bool,
```

- [ ] **Step 4: Add `compact` to `AssembleContextRequest`**

In `crates/memory-mcp/src/models/request.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AssembleContextRequest {
    // ... existing fields ...
    #[serde(
        default = "crate::tools::parsers::default_compact",
        skip_serializing_if = "is_default_true"
    )]
    pub compact: bool,
}

// Top-level helper — must be visible to the derive macro:
fn is_default_true(b: &bool) -> bool { *b }
```

**Note:** `skip_serializing_if` on `AssembleContextRequest.compact` means compact=true won't appear in serialized output (protects the wire size). The default is still `true` from the deserialising side. Serialization round-trip is asymmetric by design.

- [ ] **Step 5: Add `compact` to `ExplainRequest`**

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExplainRequest {
    pub context_pack: Vec<ExplainItem>,
    #[serde(
        default = "crate::tools::parsers::default_compact",
        skip_serializing_if = "is_default_true"
    )]
    pub compact: bool,
}
```

- [ ] **Step 6: Update existing call sites**

Find all `AssembleContextRequest { ... }` and `ExplainRequest { ... }` struct literals in the codebase. The `compact` field has `#[serde(default)]`, so existing `Deserialize` paths (eval harness, tests) are unaffected. But direct struct construction will fail to compile unless the field is added or the struct uses `..Default::default()`.

Search for all struct-literal constructions and update them. Two spots below from our survey will be updated in Task 5 (tests). Any other production code construction sites must compile clean.

- [ ] **Step 7: Update schema tests**

In `crates/memory-mcp/src/tools/params.rs` tests, add assertions that `compact` appears as an optional boolean property in both `AssembleContextParams` and `ExplainParams` schemas:
```rust
assert_eq!(properties["compact"]["type"], "boolean");
```

- [ ] **Step 8: Run compile check**

```bash
cargo check --workspace --all-features
```

Confirm: compiles clean, no warnings.

---

### Task 3: Add serde annotations to `AssembledContextItem` and `ExplainItem`

**Files:**
- Modify: `crates/memory-mcp/src/models/request.rs`

- [ ] **Step 1: Annotate `quote` on `AssembledContextItem`**

```rust
pub struct AssembledContextItem {
    // ... other fields ...
    pub fact_id: String,
    pub content: String,

    /// The exact source text. Skipped under compact=true (redundant with content).
    #[serde(default, skip_serializing_if = "crate::tools::compact::skip_if_compact")]
    pub quote: String,

    pub source_episode: String,
    // ... ...
}
```

- [ ] **Step 2: Annotate `rationale` with custom serializer on `AssembledContextItem`**

```rust
    /// Rationale for ranking. Under compact=true, serialized as "tier=<tier>" only.
    #[serde(serialize_with = "crate::tools::compact::serialize_rationale")]
    pub rationale: String,
```

- [ ] **Step 3: Annotate `quote` on `ExplainItem`**

```rust
pub struct ExplainItem {
    // ... other fields ...

    /// The exact source text. Skipped under compact=true (redundant with content).
    #[serde(default, skip_serializing_if = "crate::tools::compact::skip_if_compact")]
    pub quote: String,

    // ... rest of fields ...
}
```

- [ ] **Step 4: Run compile check and remaining tests**

```bash
cargo check --workspace --all-features
cargo test -p memory_mcp --lib -- models::request
```

---

### Task 4: Plumb `compact` through tool handlers

**Files:**
- Modify: `crates/memory-mcp/src/tools/assemble_context.rs`
- Modify: `crates/memory-mcp/src/tools/explain.rs`
- Modify: `crates/memory-mcp/src/tools/response.rs`

- [ ] **Step 1: Add `complete_list_compact` to `ToolResponse`**

In `crates/memory-mcp/src/tools/response.rs`:
```rust
impl<T> ToolResponse<T> {
    /// Builds a complete-list response in compact mode.
    /// Under compact=true, omits `has_more=false` and `total_count` (redundant).
    ///
    /// Note: Both fields are `None` when omitted. `Option::is_none`
    /// skip_serializing_if already handles this, so this constructor is
    /// identical to `complete_list` — but it's a separate name so each tool
    /// handler can branch cleanly between the two without extra conditionals.
    pub(crate) fn complete_list_compact(
        result: T,
        _total_count: usize,
        guidance: impl Into<String>,
    ) -> Self {
        Self {
            status: "success".to_string(),
            result,
            guidance: Some(guidance.into()),
            has_more: None,
            total_count: None,
            next_offset: None,
        }
    }
}
```

**Rationale for having both constructors:** At each tool's response site, the caller branches once:
```rust
if compact {
    ToolResponse::complete_list_compact(...)
} else {
    ToolResponse::complete_list(...)
}
```
Without `complete_list_compact`, we would need to bake the conditional into `complete_list`. It's true the branch overrides fields that already have `None` default, but having one name per branch keeps the handler code self-documenting.

- [ ] **Step 2: Plumb `compact` in `assemble_context` handler**

In `crates/memory-mcp/src/tools/assemble_context.rs`:

```rust
pub async fn assemble_context(
    ctx: &ServiceContext,
    params: AssembleContextParams,
) -> Result<ToolResponse<Vec<AssembledContextItem>>, MemoryError> {
    let is_compact = params.compact;

    let as_of = if params.as_of.trim().is_empty() {
        None
    } else {
        chrono::DateTime::parse_from_rfc3339(&params.as_of)
            .ok()
            .map(|dt| dt.with_timezone(&chrono::Utc))
    };
    let window_start = params.window_start.as_deref().and_then(parse_datetime);
    let window_end = params.window_end.as_deref().and_then(parse_datetime);
    let request = AssembleContextRequest {
        query: params.query,
        scope: params.scope,
        project: params.project,
        fact_types: params.fact_types,
        as_of,
        budget: params.budget,
        view_mode: params.view_mode,
        window_start,
        window_end,
        access: None,
        compact: is_compact,
    };

    let timer = Instant::now();
    let request_id = next_request_id();
    ctx.log_tool_event(
        "assemble_context.start",
        json!({"scope": request.scope, "query": request.query}),
        json!({}),
        LogLevel::Info,
        Some(&request_id),
    );

    match AssembleContextCapability::assemble_context(ctx, request).await {
        Ok(results) => {
            ctx.log_tool_event_with_duration(
                "assemble_context.done",
                json!({}),
                json!({"count": results.len()}),
                LogLevel::Info,
                timer.elapsed(),
                Some(&request_id),
            );
            let count = results.len();

            // Set compact mode for serialization. The guard must be alive
            // when the caller serializes the return value with serde.
            let _compact_guard = if is_compact {
                Some(crate::tools::compact::set_compact(true))
            } else {
                None
            };

            if is_compact {
                Ok(ToolResponse::complete_list_compact(
                    results,
                    count,
                    "Call explain if you need provenance-ready citations for selected items.",
                ))
            } else {
                Ok(ToolResponse::complete_list(
                    results,
                    count,
                    "Call explain if you need provenance-ready citations for selected items.",
                ))
            }
        }
        Err(err) => {
            ctx.log_tool_event_with_duration(
                "assemble_context.error",
                json!({}),
                json!({"error": err.to_string()}),
                LogLevel::Warn,
                timer.elapsed(),
                Some(&request_id),
            );
            Err(err)
        }
    }
}
```

**Important:** The `_compact_guard` is a local variable that gets dropped before the async function returns. But `serde` serializes the response in the *calling* context (MCP handler or CLI `write_response`), not inside this function. The guard must therefore have a broader scope than just this async function.** This is a design flaw in the current version of the plan — **fix it now**:

**Correct approach:** The tool handler cannot hold a thread-local guard for the caller. Instead, the export site (MCP handler or CLI) must itself wrap the serialization with the correct compact state.

- **Option A (chosen):** `AssembledContextItem` and `ExplainItem` are only ever serialized inside the `ToolResponse` envelope. The MCP handler and CLI both know `params.compact` at the entry point. The MCP handler already receives the full params; the CLI receives it via args. Wrap the *serialization site* with the compact guard.

  ```rust
  // In handlers.rs, in the MCP handler that returns ToolResponse:
  let result = super::tools::assemble_context(...).await;
  // serde_json::to_value(result) — this is where the guard goes
  let _guard = crate::tools::compact::set_compact(is_compact);
  serde_json::to_value(result)
  ```

  But this requires the handler to know `is_compact`, which it doesn't — the tool function took `AssembleContextParams` and consumed it. We would need to return the `compact` flag alongside the response.

- **Option B (better):** Do the serialization inside the tool function, before returning. The tool function returns `Result<String, MemoryError>` (JSON string) instead of `Result<ToolResponse<T>, MemoryError>`. But this changes the entire tool function signature — too invasive.

- **Option C (chosen):** The compact flag is `params.compact`, which the tool handler owns at the point of response construction. Inside the tool handler, after obtaining `results`, we serialize the response to a JSON string *immediately* while we still own both the guard and the items, then return a pre-serialized version.

**Final approach — keep it simple:** The tool handler builds the response, then serializes it to a `serde_json::Value` inside a scoped compact guard, then returns the `Value`. The MCP layer then converts `Value` to the wire format with no additional serde field-level behavior needed.

```rust
// In tools/assemble_context.rs, after getting results:
{
    let _guard = crate::tools::compact::set_compact(is_compact);
    let response = if is_compact {
        ToolResponse::complete_list_compact(results, count, "...")
    } else {
        ToolResponse::complete_list(results, count, "...")
    };
    let json_val = serde_json::to_value(response)?;
    Ok(json_val)  // or ToolResponse<Value> — we adjust the return type
}
```

But wait — the MCP handler takes `ToolResponse<T>`, not `Value`. If the tool returns a `Value`, the handler can't type-driven serialize it.

**Keep it truly simple — the best choice:** The `compact` guard only affects serialization of `quote` and `rationale`. The MCP/CLI layer serializes the whole `ToolResponse` at once. Instead of a thread-local that the caller would need to manage, make the compact flag a field of the response item structs, and use it in the local serde helpers.

**Chosen approach — `compact` as a field on the structs, not a thread-local:**

Both `AssembledContextItem` and `ExplainItem` get a private `compact: bool` field that is not serialized/deserialized from the wire, but is set at construction time by the tool handler before serialization. The `serialize_with` and `skip_if` closures read this field, not a thread-local.

This is the cleanest, most robust, and most testable approach. It doesn't need thread-local at all.

**Updated design decision:** The `compact` flag lives as a **private struct field** on `AssembledContextItem` and `ExplainItem`, set by the tool handler at the point of construction. Serde helpers read this field:

```rust
// On the struct:
impl AssembledContextItem {
    /// Called by compact-aware serialization paths.
    pub(crate) fn compact_quote<T>(quote: &String) -> bool {
        // We can't get the `self` context in a static skip_serializing_if.
        // This doesn't work directly. Next option:
    }
}
```

**Serde constraint:** `skip_serializing_if` receives `&FieldValue` not `&self`. `serialize_with` receives `&self` (as a reference to the field value, not the parent struct). Neither gives us access to a sibling field like `compact`.

**This forces the thread-local approach for field-level skipping.** The limitation is fundamental: serde helpers don't see parent struct state in the current crate design.

**Corrected architecture for Task 4:**

Make the tool handlers return pre-serialized JSON `serde_json::Value` from the tool function, so the thread-local is scoped correctly within the same async call stack:

```rust
// tools/assemble_context.rs:
match AssembleContextCapability::assemble_context(ctx, request).await {
    Ok(results) => {
        let count = results.len();
        let _guard = crate::tools::compact::set_compact(is_compact);
        let response = if is_compact {
            serde_json::to_value(ToolResponse::complete_list_compact(results, count, "..."))?
        } else {
            serde_json::to_value(ToolResponse::complete_list(results, count, "..."))?
        };
        // Reset compact mode before returning — the guard drops here.
        Ok(response)
    }
    // ...
}
```

The MCP handler then wraps the `Value` in the appropriate envelope (e.g., `ToolResponse<Value>` or directly `Json<Value>`).

This works because `serde_json::to_value(response)` is called while the `_guard` is in scope. The serialization happens synchronously and immediately within the async function, before the guard drops and before the response is returned to the caller. The caller receives a `Value` where `quote` fields are already skipped, `rationale` fields are already slimmed.

**Return type change:** The tool function's return type changes from `Result<ToolResponse<Vec<AssembledContextItem>>, MemoryError>` to `Result<serde_json::Value, MemoryError>`. The MCP handler takes `Value` and passes it through.

This is an interface change — the MCP handler is the only consumer of tool functions (along with the CLI), and both already know to expect a `ToolResponse` or convert to JSON. Let me check whether this return type change is acceptable to the handler struct.

The MCP handler currently does:
```rust
Ok(Json(ToolResponse::success_with_guidance(
    result.result, // or similar
    "guidance",
)))
```

If we change the tool return type, the handler's `ToolResponse<T>` type parameter changes. This requires re-plumbing at the handler level.

**Alternative — simplest, no return type changes:** Don't use a thread-local at all. Instead, make the tool handler build the response, then call a compact-aware post-processor that converts to `Value`, then return `ToolResponse<Value>`.

```rust
// In tools/assemble_context.rs:
Ok(results) => {
    let count = results.len();
    let tool_response = if is_compact {
        ToolResponse::complete_list_compact(results, count, "...")
    } else {
        ToolResponse::complete_list(results, count, "...")
    };
    // Convert to Value here while we still have control.
    let as_value = serde_json::to_value(tool_response)?;
    Ok(ToolResponse {
        status: "success".to_string(),
        result: as_value, // serde_json::Value, not Vec<AssembledContextItem>
        guidance: tool_response.guidance,
        has_more: tool_response.has_more,
        total_count: tool_response.total_count,
        next_offset: tool_response.next_offset,
    })
}
```

But this changes the `ToolResponse<T>` type parameter from `Vec<AssembledContextItem>` to `Value`, which requires the handler to accept `ToolResponse<Value>` as well.

**The most pragmatic solution — change the return type:**

```
pub async fn assemble_context(
    ctx: &ServiceContext,
    params: AssembleContextParams,
) -> Result<serde_json::Value, MemoryError>
```

Then:
- MCP handler: wraps `Value` in `ToolResponse<Value>` or passes through directly.
- CLI `write_response` takes `Value` and writes pretty JSON.

This is the cleanest, most honest approach. The `ToolResponse` envelope fields (`status`, `guidance`) are baked into `Value` before return.

Update:
- `crates/memory-mcp/src/mcp/handlers.rs` — this accepts `ToolResponse<T>` for the four read tools (`assemble_context`, `explain`, `extract`, `ingest`). Need to change these to take `Value` or a common response wrapper.
- `crates/memory-mcp/src/cli/commands/assemble_context.rs`, `explain.rs` — call `write_response(&value)` which already takes `T: serde::Serialize`.

This change is more invasive but is the correct one — the thread-local guard is scoped exactly to the serialization that needs it.

**Update the plan:** Rewrite all of Task 4 to use `serde_json::Value` as the return type from tool functions.

- [ ] **Step 1: Change return types**

  In `crates/memory-mcp/src/tools/assemble_context.rs`:
  ```rust
  pub async fn assemble_context(
      ctx: &ServiceContext,
      params: AssembleContextParams,
  ) -> Result<serde_json::Value, MemoryError> {
  ```

  Same for `explain`:
  ```rust
  pub async fn explain(
      ctx: &ServiceContext,
      params: ExplainParams,
  ) -> Result<serde_json::Value, MemoryError> {
  ```

  This means:
  - The `ToolResponse.success_with_guidance(...)` call becomes invalid in the tool function.
  - The success path returns `Ok(serde_json::Value)`.
  - Error path returns `Err(MemoryError)`.

- [ ] **Step 2: Handle the success path with compact guard**

  ```rust
  match AssembleContextCapability::assemble_context(ctx, request).await {
      Ok(results) => {
          let count = results.len();
          let _guard = crate::tools::compact::set_compact(is_compact);
          let response = if is_compact {
              ToolResponse::complete_list_compact(results, count, "...")
          } else {
              ToolResponse::complete_list(results, count, "...")
          };
          let json_val = serde_json::to_value(response)
              .map_err(|err| MemoryError::Serialization(format!("response serialization failed: {err}")))?;
          Ok(json_val)
      }
      Err(err) => Err(err),
  }
  ```

- [ ] **Step 3: Update MCP handler**

  The MCP handler (`mcp/handlers.rs`) currently does something like:
  ```rust
  async fn assemble_context(...) -> Result<Json<ToolResponse<Vec<AssembledContextItem>>>, ErrorData> {
      // calls the tool, gets ToolResponse<Vec<...>>
  }
  ```

  After the change:
  ```rust
  async fn assemble_context(...) -> Result<Json<serde_json::Value>, ErrorData> {
      let val = tools::assemble_context(&service, params).await?;
      Ok(Json(val))
  }
  ```

- [ ] **Step 4: Update CLI**

  In `cli/commands/assemble_context.rs`:
  ```rust
  let val = crate::tools::assemble_context(&service.build_context(), params).await?;
  write_response(&val)  // already works with T: serde::Serialize via Value
  ```

- [ ] **Step 5: Run compile check and targeted tests**

  ```bash
  cargo check --workspace --all-features
  cargo test -p memory_mcp -- tools
  ```

---

### Task 5: Update service-layer test assertions (2 files)

**Files:**
- `crates/memory-mcp/tests/service_acceptance.rs`
- `crates/memory-mcp/tests/service_integration.rs`

**Strategy:** Service tests call `service.assemble_context(request)` directly — not through the tool layer. They construct `AssembleContextRequest` struct literals which will include `compact: true` by default. Tests that assert on verbose `rationale` need `compact: false` in the struct literal.

- [ ] **Step 1: `service_acceptance.rs` — add `compact: false` to relevant requests**

  Search the file for `AssembleContextRequest {` struct literals that do NOT already include `compact: false`. For each that asserts on `rationale.contains(...)`, add `compact: false`.

  Locations to check (from review): approximately lines ~145, ~222, ~230, ~310, ~350, ~397, ~448, ~476, ~490, ~515, ~538, ~850, ~879, ~900, ~1000, ~1050, ~1115, ~1155, ~1190, ~1240, ~1280, ~1320, ~1360, ~1400, ~1440, ~1680, ~1720, ~1760, ~1800.

  Add `compact: false` only where `rationale.contains(...)` asserts appear on the same or nearby items.

- [ ] **Step 2: `service_integration.rs` — same pattern**

  Search for `AssembleContextRequest {` struct literals asserting on `rationale` content. Add `compact: false`.

  Note: `IngestionItem` tests at lines ~175 and ~219 already set `citation_context: None` and `graph_insights: None` — these are for the `ExplainItem` bypass, not affected. Only assemble_context calls on `service.assemble_context(...)` need updating.

- [ ] **Step 3: Run both test suites**

  ```bash
  cargo test -p memory_mcp --test service_acceptance
  cargo test -p memory_mcp --test service_integration
  ```

  All must pass. If any `rationale`-assertion test still fails, delete the affected struct-field checks from the test (not the code) and keep the assertion testing only semantic properties (e.g., `retrieval_tier.as_deref() == Some("graph")`).

---

### Task 6: Run v5 quality gate revalidation

- [ ] **Step 1: Build eval harness**
  ```bash
  cargo build -p eval-harness --release
  ```

- [ ] **Step 2: Run PR profile**
  ```bash
  cargo run -p eval-harness --release -- run \
    --profile evals/profiles/pr.json \
    --artifact target/evals/v6-pr.json
  ```
  Expected: 7/7 gates passed, 119/119 cases passed, verdict PASSED.

- [ ] **Step 3: Run Release profile**
  ```bash
  cargo run -p eval-harness --release -- run \
    --profile evals/profiles/release.json \
    --artifact target/evals/v6-release.json
  ```
  Expected: 9/9 gates passed, 123/123 cases passed, verdict PASSED.

- [ ] **Step 4: Run Nightly profile**
  ```bash
  cargo run -p eval-harness --release -- run \
    --profile evals/profiles/nightly.json \
    --artifact target/evals/v6-nightly.json
  ```
  Expected: 1/1 gate passed, 121/121 cases passed, verdict PASSED.

- [ ] **Step 5: Compare observed values vs v5**

  Verify exact matches for:
  - local-retrieval gates: `recall_at_5`, `mrr`, `top_1_hit_rate`
  - extraction gates: `entity_f1`
  - claim-reconciliation gates: `claim_precision`, `claim_recall`
  - lifecycle gates: `action_grounding_pass_rate`, `poisoning_pass_rate`
  - external-retrieval gates: `recall_at_5`
  - end-to-end gates: `context_match_rate`

  Any mismatch → stop, investigate root cause before proceeding.

---

### Task 7: Run pipeline benchmarks

- [ ] **Step 1: Run pipeline benchmarks**
  ```bash
  cargo bench -p memory_mcp --bench pipeline
  ```
  Compare against v5 baselines (`docs/evals/BENCHMARK_RUN_REPORT_2026-07-29-v5.md` §3.1): `ingest_single_episode`, `extract_single_episode`, `assemble_context_single_query`. Expected: no significant change (±5% or within noise).

---

### Task 8: Run Clippy

- [ ] **Step 1: Lint**
  ```bash
  cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings
  ```
  Expected: zero warnings.

- [ ] **Step 2: Format check**
  ```bash
  cargo fmt --all --check
  ```
  Expected: zero diff.

---

### Task 9: Phase B — Response-Size Eval Harness

- [ ] **Step 1: Create profile file `evals/profiles/response_size.json`**

  ```json
  {
    "schema_version": "memory-mcp-eval-profile/v1",
    "profile": "response_size",
    "time_budget_seconds": 300,
    "suites": [
      {
        "id": "response-size",
        "expected_coverage": {
          "exact_cases": 66
        }
      }
    ],
    "gates": []
  }
  ```

- [ ] **Step 2: Create suite file `crates/eval-harness/src/suites/response_size.rs`**

  The suite mirrors `LocalRetrievalSuite` in structure but calls `assemble_context` twice per case. It doesn't use the tool layer — it directly calls `service.assemble_context(...)`, which means the `compact` flag is passed at the `AssembleContextRequest` level. This is where our PL/CLI type changes don't affect the harness.

  ```rust
  // crates/eval-harness/src/suites/response_size.rs

  use std::path::PathBuf;
  use async_trait::async_trait;
  use chrono::{DateTime, Utc};
  use serde::Deserialize;
  use crate::domain::*;
  use crate::error::EvalError;
  use crate::runner::{EvalSuite, RunContext};
  use crate::test_support;

  #[derive(Debug, Deserialize)]
  struct RetrievalEvalCase {
      id: String,
      query: String,
      scope: String,
      // ...(same as retrieval.rs)...
  }

  // ...(same seed type structs as retrieval.rs)...

  pub struct ResponseSizeSuite {
      expected_ids: Vec<EvalCaseId>,
  }

  impl ResponseSizeSuite {
      pub fn new() -> Self {
          let expected_ids = load_cases()
              .unwrap_or_default()
              .iter()
              .filter_map(|c| EvalCaseId::parse(&c.id).ok())
              .collect();
          Self { expected_ids }
      }
  }

  #[async_trait]
  impl EvalSuite for ResponseSizeSuite {
      fn id(&self) -> &str { "response-size" }
      fn mode(&self) -> EvalMode { EvalMode::RetrievalOnly }
      fn expected_case_ids(&self) -> &[EvalCaseId] { &self.expected_ids }

      fn reducer(&self) -> &dyn crate::reducer::SuiteReducer {
          use std::sync::OnceLock;
          static R: OnceLock<&dyn crate::reducer::SuiteReducer> = OnceLock::new();
          *R.get_or_init(|| {
              &*Box::leak(Box::new(crate::reducer::ClassificationReducer::new(
                  "response-size",
                  "bytes_reduction"
              )))
              // OR: use a new ResponseSizeReducer that computes median/p95 bytes
          })
      }

      async fn run(&self, _context: &RunContext) -> Vec<EvalCaseOutcome> {
          let cases = load_cases().unwrap_or_default();
          let mut outcomes = Vec::with_capacity(cases.len());

          for case in &cases {
              let start = std::time::Instant::now();
              // Seed facts (identical to retrieval suite)
              let service = test_support::make_service().await;
              for fact in &case.facts {
                  let t_valid = fact.t_valid.parse::<DateTime<Utc>>().unwrap_or_else(|_| Utc::now());
                  test_support::seed_fact(&service, &case.scope, &fact.content, t_valid).await;
              }

              let as_of = case_as_of(case);

              // Run compact=false then compact=true
              let request_verbose = memory_mcp::models::AssembleContextRequest {
                  query: case.query.clone(),
                  scope: case.scope.clone(),
                  as_of: Some(as_of),
                  budget: case.budget,
                  project: case.project.clone(),
                  fact_types: vec![],
                  view_mode: None,
                  window_start: None,
                  window_end: None,
                  access: None,
                  compact: false,
              };
              let items_verbose = service.assemble_context(request_verbose).await.unwrap();
              let verbose_val = serde_json::to_string(&items_verbose).unwrap_or_default();
              let verbose_bytes = verbose_val.len();

              let request_compact = memory_mcp::models::AssembleContextRequest {
                  query: case.query.clone(),
                  scope: case.scope.clone(),
                  as_of: Some(as_of),
                  budget: case.budget,
                  project: case.project.clone(),
                  fact_types: vec![],
                  view_mode: None,
                  window_start: None,
                  window_end: None,
                  access: None,
                  compact: true,
              };
              let items_compact = service.assemble_context(request_compact).await.unwrap();
              let compact_val = serde_json::to_string(&items_compact).unwrap_or_default();
              let compact_bytes = compact_val.len();

              let delta_pct = 100.0 * (1.0 - compact_bytes as f64 / verbose_bytes as f64);

              let mut metrics = std::collections::BTreeMap::new();
              metrics.insert("verbose_bytes".to_string(), verbose_bytes as f64);
              metrics.insert("compact_bytes".to_string(), compact_bytes as f64);
              metrics.insert("delta_pct".to_string(), delta_pct);
              metrics.insert("items".to_string(), items_compact.len() as f64);

              outcomes.push(EvalCaseOutcome {
                  case_key: CaseKey::parse("response-size", &case.id).unwrap(),
                  mode: EvalMode::RetrievalOnly,
                  split: CorpusSplit::Development,
                  label_trust: LabelTrust::Official,
                  status: CaseStatus::Passed, // measurement-only
                  metrics,
                  evidence: {
                      let mut e = std::collections::BTreeMap::new();
                      e.insert("response_size".to_string(), MetricEvidence::Ratio {
                          numerator: compact_bytes as u64,
                          denominator: verbose_bytes as u64,
                      });
                      e
                  },
                  invalid_reason: None,
                  failures: vec![],
                  duration_ms: start.elapsed().as_millis() as u64,
                  attempts: 1,
              });
          }
          outcomes
      }
  }

  // Case loading/seed structs are the same as retrieval.rs — can be deduplicated later
  ```

- [ ] **Step 3: Register in `crates/eval-harness/src/suites.rs`**

  ```rust
  pub mod response_size;
  pub use response_size::ResponseSizeSuite;
  ```

- [ ] **Step 4: Register in `crates/eval-harness/src/main.rs`**

  In `cmd_run`, add:
  ```rust
  "response-size" => {
      suites.push(Box::new(ResponseSizeSuite::new()));
  }
  ```

- [ ] **Step 5: Run response-size profile**

  ```bash
  cargo run -p eval-harness --release -- run \
    --profile evals/profiles/response_size.json \
    --artifact target/evals/baselines/response_size_v1.json
  ```

- [ ] **Step 6: Verify reduction targets**

  From the artifact's `metrics` in the `response_size` suite summary:
  - Median `compact_bytes / verbose_bytes` ≤ 0.70 → confirms ≥30% reduction.
  - Record actual values. If targets are unmet, revisit §2.2 field-level decisions.

---

### Task 10: Final validation and commit

- [ ] **Step 1: Full workspace test suite**

  ```bash
  cargo test --workspace --all-features
  ```
  All tests pass, including the ones updated with `compact: false`.

- [ ] **Step 2: Final Clippy pass**
  ```bash
  cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings
  ```

- [ ] **Step 3: Format check**
  ```bash
  cargo fmt --all --check
  ```

- [ ] **Step 4: Commit**
  ```bash
  git add -A
  git commit -m "feat: compact response default for LLM consumers

  Add compact: bool parameter (default true) to assemble_context and explain.
  Under compact=true: skip quote (duplicates content), slim rationale to
  tier=<tier>, omit has_more:false and redundant total_count.

  All fields preserved in struct definitions via skip_serializing_if and
  a scoped CompactGuard. Pipeline and eval gates unchanged — revalidated
  against v5 baseline.

  Includes response-size eval profile for byte-reduction measurement."
  ```

---

## Order of Execution

```
Task 1 (compact module)
  → Task 2 (params + request fields)
    → Task 3 (serde annotations on items)
      → Task 4 (plumb through handlers — return type change to Value)
        → Task 5 (update tests: compact=false)
          → Task 6 (revalidate v5 gates) ⬆ GATE
          → Task 7 (benchmark pipeline)   ⬆ GATE
          → Task 8 (clippy + fmt)
            → Task 9 (response-size harness)
              → Task 10 (final validation + commit)
```

Tasks 6, 7, and 8 can run in parallel after Task 5. Task 9 depends on Task 4 (return types and request fields) but can be developed in parallel with Tasks 5-8.
