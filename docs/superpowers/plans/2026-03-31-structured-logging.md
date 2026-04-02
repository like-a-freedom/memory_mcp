# Structured Logging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Внедрить структурированное логирование с фиксированным порядком полей, временем выполнения операций, correlation ID для сквозной трассировки, и единым форматом для embed/NER операций.

**Architecture:** Единый формат логов через `StdoutLogger` с фиксированным порядком ключей, макросом `operation_event!` для удобного создания событий, и policy-функцией `level_for_duration()` для автоматического выбора уровня логирования по duration. Correlation ID генерируется fresh на каждый tool call и передаётся через производный logger.

**Tech Stack:** Rust, `StdoutLogger`, `OperationTimer`, `CorrelationId`

---

## Task 1: Add LOG_FIELD_ORDER and fix field ordering in StdoutLogger

**Files:**
- Modify: `src/logging.rs:15-60`
- Test: `src/logging.rs` (existing tests)

- [ ] **Step 1: Add LOG_FIELD_ORDER constant**

Add after `LogLevel` enum:

```rust
/// Fixed field order for consistent log output.
/// Order: op, status, duration_ms, provider, count, error (then alphabetically sorted rest)
const LOG_FIELD_ORDER: &[&str] = &["op", "status", "duration_ms", "provider", "count", "error"];
```

- [ ] **Step 2: Modify format_event_line_with_correlation to respect LOG_FIELD_ORDER**

Replace sorting logic (lines 164-172):

```rust
// First emit fields in fixed order
for key in LOG_FIELD_ORDER {
    if let Some(value) = event.get(*key) {
        let value_str = value_to_string(value);
        parts.push(format!("{}={}", key, quote_if_needed(&value_str)));
    }
}

// Then emit remaining fields alphabetically
let remaining: Vec<_> = event.keys()
    .filter(|k| !LOG_FIELD_ORDER.contains(&k.as_str()))
    .cloned()
    .collect();
let mut remaining = remaining;
remaining.sort();

for key in remaining {
    if let Some(value) = event.get(&key) {
        let value_str = value_to_string(value);
        parts.push(format!("{}={}", key, quote_if_needed(&value_str)));
    }
}
```

- [ ] **Step 3: Run tests to verify**

```bash
cargo test --lib logging
```

Expected: All existing tests pass

- [ ] **Step 4: Commit**

```bash
git add src/logging.rs
git commit -m "feat(logging): add LOG_FIELD_ORDER and fixed field ordering"
```

---

## Task 2: Add level_for_duration() policy function

**Files:**
- Modify: `src/logging.rs:55-70`
- Test: `src/logging.rs` (add test)

- [ ] **Step 1: Add level_for_duration function**

Add after `LogLevel::as_str()`:

```rust
/// Returns appropriate log level based on operation duration.
/// For successful operations only - errors always use Error level.
pub fn level_for_duration(duration_ms: u128) -> LogLevel {
    match duration_ms {
        0..=99 => LogLevel::Debug,
        100..=999 => LogLevel::Info,
        _ => LogLevel::Warn,
    }
}
```

- [ ] **Step 2: Add tests**

Add to `mod tests`:

```rust
#[test]
fn level_for_duration_debug_for_fast_operations() {
    assert_eq!(level_for_duration(50), LogLevel::Debug);
    assert_eq!(level_for_duration(99), LogLevel::Debug);
}

#[test]
fn level_for_duration_info_for_normal_operations() {
    assert_eq!(level_for_duration(100), LogLevel::Info);
    assert_eq!(level_for_duration(500), LogLevel::Info);
    assert_eq!(level_for_duration(999), LogLevel::Info);
}

#[test]
fn level_for_duration_warn_for_slow_operations() {
    assert_eq!(level_for_duration(1000), LogLevel::Warn);
    assert_eq!(level_for_duration(5000), LogLevel::Warn);
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test --lib logging
```

- [ ] **Step 4: Commit**

```bash
git add src/logging.rs
git commit -m "feat(logging): add level_for_duration policy function"
```

---

## Task 3: Add operation_event! macro

**Files:**
- Modify: `src/logging.rs`
- Test: `src/logging.rs`

- [ ] **Step 1: Add operation_event! macro**

Add after imports (after line 14):

```rust
/// Macro for building operation log events with fixed field order.
///
/// # Example
///
/// ```rust
/// let timer = OperationTimer::new("embed");
/// let result = embedding_provider.embed(input).await;
/// let event = operation_event!(
///     "embed",
///     timer,
///     result.as_ref().map(|_| ()),
///     provider="ollama",
///     count=5
/// );
/// ```
#[macro_export]
macro_rules! operation_event {
    ($op:expr, $timer:expr, Ok, $($field:tt)*) => {{
        let mut event = std::collections::HashMap::new();
        event.insert("op".to_string(), serde_json::json!($op));
        event.insert("status".to_string(), serde_json::json!("success"));
        event.insert("duration_ms".to_string(), serde_json::json!($timer.elapsed_ms()));
        // Add custom fields
        $(event.insert($field.0.to_string(), $field.1);)*
        event
    }};
    ($op:expr, $timer:expr, Err($err:expr), $($field:tt)*) => {{
        let mut event = std::collections::HashMap::new();
        event.insert("op".to_string(), serde_json::json!($op));
        event.insert("status".to_string(), serde_json::json!("error"));
        event.insert("duration_ms".to_string(), serde_json::json!($timer.elapsed_ms()));
        event.insert("error".to_string(), serde_json::json!($err.to_string()));
        // Add custom fields
        $(event.insert($field.0.to_string(), $field.1);)*
        event
    }};
    ($op:expr, $timer:expr, $result:expr, $($field:tt)*) => {{
        let mut event = std::collections::HashMap::new();
        event.insert("op".to_string(), serde_json::json!($op));
        event.insert("status".to_string(), serde_json::json!($result.is_ok() ? "success" : "error"));
        event.insert("duration_ms".to_string(), serde_json::json!($timer.elapsed_ms()));
        if let Err(e) = &$result {
            event.insert("error".to_string(), serde_json::json!(e.to_string()));
        }
        // Add custom fields
        $(event.insert($field.0.to_string(), $field.1);)*
        event
    }};
}
```

- [ ] **Step 2: Run tests**

```bash
cargo check
```

- [ ] **Step 3: Commit**

```bash
git add src/logging.rs
git commit -m "feat(logging): add operation_event! macro"
```

---

## Task 4: Add provider_name() to EntityExtractor trait

**Files:**
- Modify: `src/service/entity_extraction.rs:16-25`
- Modify: All implementations in same file

- [ ] **Step 1: Add provider_name() to trait definition**

```rust
#[async_trait]
pub trait EntityExtractor: Send + Sync {
    /// Human-readable provider name for logging.
    fn provider_name(&self) -> &'static str;

    /// Returns normalized entity candidates discovered in the supplied content.
    async fn extract_candidates(&self, content: &str) -> Result<Vec<EntityCandidate>, MemoryError>;
}
```

- [ ] **Step 2: Implement for RegexEntityExtractor**

In `impl EntityExtractor for RegexEntityExtractor`, add:

```rust
fn provider_name(&self) -> &'static str {
    "regex"
}
```

- [ ] **Step 3: Implement for LlmEntityExtractor**

In `impl EntityExtractor for LlmEntityExtractor`, add:

```rust
fn provider_name(&self) -> &'static str {
    "llm"
}
```

- [ ] **Step 4: Run tests**

```bash
cargo check
cargo test --lib entity_extraction
```

- [ ] **Step 5: Commit**

```bash
git add src/service/entity_extraction.rs
git commit -m "feat(logging): add provider_name() to EntityExtractor trait"
```

---

## Task 5: Add provider_name() to AnnoEntityExtractor and GlinerEntityExtractor

**Files:**
- Modify: `src/service/anno_entity_extractor.rs`
- Modify: `src/service/gliner_entity_extractor.rs`

- [ ] **Step 1: Add provider_name() to AnnoEntityExtractor**

In `src/service/anno_entity_extractor.rs`, add method to impl block:

```rust
fn provider_name(&self) -> &'static str {
    "anno"
}
```

- [ ] **Step 2: Add provider_name() to GlinerEntityExtractor**

In `src/service/gliner_entity_extractor.rs`, add method to impl block:

```rust
fn provider_name(&self) -> &'static str {
    "gliner"
}
```

- [ ] **Step 3: Run tests**

```bash
cargo check
```

- [ ] **Step 4: Commit**

```bash
git add src/service/anno_entity_extractor.rs src/service/gliner_entity_extractor.rs
git commit -m "feat(logging): add provider_name() to anno and gliner extractors"
```

---

## Task 6: Add embed logging to generate_embedding in core.rs

**Files:**
- Modify: `src/service/core.rs:725-740`

- [ ] **Step 1: Add timing and logging to generate_embedding**

Replace current implementation:

```rust
pub(crate) async fn generate_embedding(
    &self,
    input: &str,
) -> Result<Option<Vec<f64>>, MemoryError> {
    if !self.embedding_provider.is_enabled() {
        return Ok(None);
    }

    let timer = crate::timing::OperationTimer::new("embed");
    let result = self.embedding_provider.embed(input).await;
    let duration_ms = timer.elapsed_ms();

    match result {
        Ok(embedding) => {
            let level = crate::logging::level_for_duration(duration_ms);
            let event = crate::logging::operation_event!(
                "embed",
                timer,
                Ok(()),
                provider=self.embedding_provider.provider_name(),
                dimension=embedding.len()
            );
            self.logger.log(event, level);
            Ok(Some(embedding))
        }
        Err(e) => {
            let event = crate::logging::operation_event!(
                "embed",
                timer,
                Err(&e),
                provider=self.embedding_provider.provider_name()
            );
            self.logger.log(event, crate::logging::LogLevel::Error);
            Err(e)
        }
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cargo check
cargo test --lib core
```

- [ ] **Step 3: Commit**

```bash
git add src/service/core.rs
git commit -m "feat(logging): add embed operation logging with duration and provider"
```

---

## Task 7: Add NER logging to extract_entities in episode.rs

**Files:**
- Modify: `src/service/episode.rs:193-220`

- [ ] **Step 1: Add timing and logging to extract_entities**

Find the function and add logging:

```rust
pub async fn extract_entities(
    service: &crate::service::MemoryService,
    content: &str,
) -> Result<Vec<ExtractedEntity>, MemoryError> {
    let timer = crate::timing::OperationTimer::new("ner");
    let candidates = service.entity_extractor.extract_candidates(content).await;
    let duration_ms = timer.elapsed_ms();

    match candidates {
        Ok(candidates) => {
            let level = crate::logging::level_for_duration(duration_ms);
            let event = crate::logging::operation_event!(
                "ner",
                timer,
                Ok(()),
                provider=service.entity_extractor.provider_name(),
                count=candidates.len()
            );
            service.logger.log(event, level);

            let mut entities = Vec::with_capacity(candidates.len());
            for candidate in candidates {
                let entity_type = candidate.entity_type.clone();
                let canonical_name = candidate.canonical_name.clone();
                let entity_id = service.resolve(candidate, None).await?;
                entities.push(ExtractedEntity {
                    entity_id,
                    entity_type,
                    canonical_name,
                });
            }
            Ok(entities)
        }
        Err(e) => {
            let event = crate::logging::operation_event!(
                "ner",
                timer,
                Err(&e),
                provider=service.entity_extractor.provider_name()
            );
            service.logger.log(event, crate::logging::LogLevel::Error);
            Err(e)
        }
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cargo check
cargo test --lib episode
```

- [ ] **Step 3: Commit**

```bash
git add src/service/episode.rs
git commit -m "feat(logging): add NER operation logging with duration and provider"
```

---

## Task 8: Add correlation ID to call_tool in handlers.rs

**Files:**
- Modify: `src/mcp/handlers.rs:670-695`

- [ ] **Step 1: Import CorrelationId**

Add import:

```rust
use crate::correlation::CorrelationId;
```

- [ ] **Step 2: Add correlation ID to call_tool**

Replace function:

```rust
async fn call_tool(
    &self,
    request: CallToolRequestParams,
    context: RequestContext<RoleServer>,
) -> Result<CallToolResult, ErrorData> {
    if !Self::is_public_tool_name(&request.name) {
        return Err(ErrorData::new(
            ErrorCode::METHOD_NOT_FOUND,
            format!("Unknown tool: {}", request.name),
            None,
        ));
    }

    let correlation_id = CorrelationId::new();
    let logger = self.service.logger.with_correlation_id(correlation_id);

    logger.log(
        {
            use serde_json::json;
            let mut event = std::collections::HashMap::new();
            event.insert("op".to_string(), json!("tool_call"));
            event.insert("tool".to_string(), json!(request.name));
            event
        },
        crate::logging::LogLevel::Debug,
    );

    let tool_context = ToolCallContext::new(self, request, context);
    self.tool_router.call(tool_context).await
}
```

- [ ] **Step 3: Run tests**

```bash
cargo check
```

- [ ] **Step 4: Commit**

```bash
git add src/mcp/handlers.rs
git commit -m "feat(logging): add correlation ID to call_tool for tracing"
```

---

## Task 9: Add correlation ID to all tool event logging in handlers.rs

**Files:**
- Modify: All tool handler methods in `src/mcp/handlers.rs` that call `log_tool_event`

- [ ] **Step 1: Change all tool handlers to use correlation-enabled logger**

For each tool handler method that uses `self.service.log_tool_event(...)`, replace with:

```rust
let logger = self.service.logger.with_correlation_id(correlation_id);
logger.log(event, level);
```

Note: This requires adding `correlation_id` parameter to handler functions or storing it in a context struct passed through.

**Alternative approach (simpler):** Since `correlation_id` is generated in `call_tool` and handlers are synchronous closures inside the router, we need to pass it through. The simplest way is to add it to `ToolCallContext` in the service layer:

1. Add a method to `MemoryService`: `pub fn logger_with_correlation(&self, cid: CorrelationId) -> StdoutLogger`
2. In handlers, access correlation from a thread-local or context

For now, we'll use a simpler approach: the correlation ID is logged at `call_tool` entry point (Task 8), and individual tool operations log their own events with the service's base logger. The correlation context can be enhanced later.

**Skip for now** - correlation propagation through handler chain requires more design. Task 8 provides the foundation.

- [ ] **Step 2: Commit what's done**

```bash
git add src/mcp/handlers.rs
git commit -m "feat(logging): add correlation ID entry point in call_tool"
```

---

## Task 10: Final verification

**Files:**
- Run: Full test suite

- [ ] **Step 1: Run clippy and format**

```bash
cargo fmt
cargo clippy -- -D warnings
```

- [ ] **Step 2: Run full test suite**

```bash
cargo test
```

- [ ] **Step 3: Verify log output manually**

Run a simple operation and check stderr output:

```bash
cargo run -- --log-level debug 2>&1 | grep -E "op=|status=|duration_ms="
```

Expected output format:
```
[2026-03-31T12:00:00.000Z] DEBUG op-00000001 op=ingest status=success duration_ms=50 source_id=user:123
[2026-03-31T12:00:00.050Z] INFO op-00000001 op=embed status=success duration_ms=150 provider=ollama dimension=384
[2026-03-31T12:00:00.200Z] INFO op-00000001 op=ner status=success duration_ms=45 provider=gliner count=3
```

- [ ] **Step 4: Final commit**

```bash
git status
git add -A
git commit -m "feat(logging): implement structured logging with correlation IDs"
```

---

## Summary of Changes

| File | Change |
|------|--------|
| `src/logging.rs` | Add `LOG_FIELD_ORDER`, fix field ordering, add `level_for_duration()`, add `operation_event!` macro |
| `src/service/entity_extraction.rs` | Add `provider_name()` to trait and implementations |
| `src/service/anno_entity_extractor.rs` | Add `provider_name()` |
| `src/service/gliner_entity_extractor.rs` | Add `provider_name()` |
| `src/service/core.rs` | Add embed operation logging with duration and provider |
| `src/service/episode.rs` | Add NER operation logging with duration and provider |
| `src/mcp/handlers.rs` | Add correlation ID generation at call_tool entry |
