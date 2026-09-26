//! Reusable database transaction mechanics: retry with backoff and a
//! per-query timeout.
//!
//! This is a technical mechanism, not a policy. It retries only what
//! [`is_transient_db_error`](super::db_errors::is_transient_db_error)
//! classifies as safe to retry, so a non-idempotent command is never
//! silently repeated: the caller decides whether the operation is
//! safe to run again by choosing whether to route it through here.
//!
//! Only infra and bootstrap use this. A bounded context reaches
//! storage through its owner-scoped store, so the retry policy is
//! applied in one place rather than restated per call site.

use std::collections::HashMap;
use std::time::Duration;

use crate::logging::{LogLevel, StdoutLogger};
use crate::platform::persistence::db_errors::is_transient_db_error;
use crate::shared::error::MemoryError;

/// Default maximum attempts for database retry on transient errors.
pub(crate) const DEFAULT_DB_RETRY_ATTEMPTS: u32 = 3;
/// Initial delay in milliseconds for database retry backoff (doubles each attempt).
pub(crate) const DEFAULT_DB_RETRY_INITIAL_DELAY_MS: u64 = 200;
/// Per-query timeout to guard against stalled database connections (e.g. WebSocket hang).
const DEFAULT_DB_QUERY_TIMEOUT_SECS: u64 = 30;

/// Runs a fallible database operation with exponential backoff retry and a per-query timeout.
///
/// Each individual attempt is guarded by `DEFAULT_DB_QUERY_TIMEOUT_SECS` to prevent
/// hanging indefinitely on stalled connections (e.g. WebSocket to SurrealDB).
/// Only retries on errors identified as transient by `is_transient_db_error`.
/// Logs each retry attempt via the provided logger with the operation name.
pub(crate) async fn with_db_retry<T, F, Fut>(
    op_name: &str,
    logger: &StdoutLogger,
    f: F,
) -> Result<T, MemoryError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, MemoryError>>,
{
    let mut attempt = 0u32;
    let timeout = Duration::from_secs(DEFAULT_DB_QUERY_TIMEOUT_SECS);
    loop {
        match tokio::time::timeout(timeout, f()).await {
            Ok(Ok(value)) => return Ok(value),
            Ok(Err(err)) => {
                attempt += 1;
                if attempt >= DEFAULT_DB_RETRY_ATTEMPTS || !is_transient_db_error(&err) {
                    return Err(err);
                }
                let delay_ms =
                    DEFAULT_DB_RETRY_INITIAL_DELAY_MS << attempt.saturating_sub(1).min(6);
                let delay = Duration::from_millis(delay_ms);
                logger.log(
                    HashMap::from([
                        (
                            "op".to_string(),
                            serde_json::Value::String(format!("db.{op_name}.retry")),
                        ),
                        (
                            "attempt".to_string(),
                            serde_json::Value::Number(serde_json::Number::from(attempt)),
                        ),
                        (
                            "delay_ms".to_string(),
                            serde_json::Value::Number(serde_json::Number::from(delay_ms)),
                        ),
                        (
                            "max_attempts".to_string(),
                            serde_json::Value::Number(serde_json::Number::from(
                                DEFAULT_DB_RETRY_ATTEMPTS,
                            )),
                        ),
                        (
                            "error".to_string(),
                            serde_json::Value::String(err.to_string()),
                        ),
                    ]),
                    LogLevel::Warn,
                );
                tokio::time::sleep(delay).await;
            }
            Err(_elapsed) => {
                // Timeout elapsed — treat as a transient error for retry purposes
                attempt += 1;
                if attempt >= DEFAULT_DB_RETRY_ATTEMPTS {
                    return Err(MemoryError::Storage(format!(
                        "db.{op_name}: timed out after {DEFAULT_DB_QUERY_TIMEOUT_SECS}s ({DEFAULT_DB_RETRY_ATTEMPTS} attempts)"
                    )));
                }
                let delay_ms =
                    DEFAULT_DB_RETRY_INITIAL_DELAY_MS << attempt.saturating_sub(1).min(6);
                let delay = Duration::from_millis(delay_ms);
                logger.log(
                    HashMap::from([
                        (
                            "op".to_string(),
                            serde_json::Value::String(format!("db.{op_name}.timeout")),
                        ),
                        (
                            "attempt".to_string(),
                            serde_json::Value::Number(serde_json::Number::from(attempt)),
                        ),
                        (
                            "delay_ms".to_string(),
                            serde_json::Value::Number(serde_json::Number::from(delay_ms)),
                        ),
                        (
                            "max_attempts".to_string(),
                            serde_json::Value::Number(serde_json::Number::from(
                                DEFAULT_DB_RETRY_ATTEMPTS,
                            )),
                        ),
                    ]),
                    LogLevel::Warn,
                );
                tokio::time::sleep(delay).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    fn logger() -> StdoutLogger {
        StdoutLogger::new("error")
    }

    #[tokio::test]
    async fn succeeds_on_first_attempt() {
        let result = with_db_retry("test_op", &logger(), || async { Ok::<_, MemoryError>(42) })
            .await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn retries_on_transient_then_succeeds() {
        let calls = Arc::new(AtomicU32::new(0));
        let counter = calls.clone();
        let result = with_db_retry("test_op", &logger(), || {
            let counter = counter.clone();
            async move {
                let n = counter.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Err(MemoryError::Storage("Transaction conflict".into()))
                } else {
                    Ok(7)
                }
            }
        })
        .await;
        assert_eq!(result.unwrap(), 7);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn does_not_retry_non_transient() {
        let calls = Arc::new(AtomicU32::new(0));
        let counter = calls.clone();
        let result: Result<i32, MemoryError> = with_db_retry("test_op", &logger(), || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Err(MemoryError::Storage("connection refused".into()))
            }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "a non-transient failure must not be retried"
        );
    }

    #[tokio::test]
    async fn exhaustion_keeps_the_last_error() {
        let result: Result<i32, MemoryError> = with_db_retry("test_op", &logger(), || async {
            Err(MemoryError::Storage("Resource busy".into()))
        })
        .await;
        match result {
            Err(MemoryError::Storage(message)) => assert_eq!(message, "Resource busy"),
            other => panic!("expected the last storage error, got {other:?}"),
        }
    }
}
