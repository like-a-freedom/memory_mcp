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
/// Maximum attempts for an attempt that hits the per-query timeout.
///
/// Deliberately below `DEFAULT_DB_RETRY_ATTEMPTS`: a timed-out query is re-run
/// against a server that is already saturated, so each retry deepens the stall
/// it is waiting on. Two attempts still covers a single dropped connection.
pub(crate) const DEFAULT_DB_TIMEOUT_ATTEMPTS: u32 = 2;

/// Attempt counts and timings for one database operation.
///
/// A timeout and a transient conflict are not the same failure and are not
/// retried the same number of times. A conflict is contention the next attempt
/// may win, so it keeps [`DEFAULT_DB_RETRY_ATTEMPTS`]. A timeout means the
/// server did not answer within the per-attempt budget; re-running it against
/// an already overloaded server re-reads the same data and multiplies the load
/// that caused the stall, so it gets two attempts — enough to ride out a
/// single dropped WebSocket, which is what the per-attempt timeout exists for.
#[derive(Debug, Clone)]
pub(crate) struct DbRetryPolicy {
    /// Attempts allowed for an error classified as transient.
    pub(crate) attempts: u32,
    /// Attempts allowed for an attempt that hits the per-attempt timeout.
    pub(crate) timeout_attempts: u32,
    /// First backoff delay; doubles per attempt up to a 64x ceiling.
    pub(crate) initial_delay: Duration,
    /// Wall-clock budget for a single attempt.
    pub(crate) per_attempt_timeout: Duration,
}

impl Default for DbRetryPolicy {
    fn default() -> Self {
        Self {
            attempts: DEFAULT_DB_RETRY_ATTEMPTS,
            timeout_attempts: DEFAULT_DB_TIMEOUT_ATTEMPTS,
            initial_delay: Duration::from_millis(DEFAULT_DB_RETRY_INITIAL_DELAY_MS),
            per_attempt_timeout: Duration::from_secs(DEFAULT_DB_QUERY_TIMEOUT_SECS),
        }
    }
}

/// Runs a fallible database operation with exponential backoff retry and a per-query timeout.
///
/// Delegates to [`with_db_retry_policy`] with the default policy.
pub(crate) async fn with_db_retry<T, F, Fut>(
    op_name: &str,
    logger: &StdoutLogger,
    f: F,
) -> Result<T, MemoryError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, MemoryError>>,
{
    with_db_retry_policy(op_name, &DbRetryPolicy::default(), logger, f).await
}

/// Runs a fallible database operation under an explicit retry policy.
///
/// Each individual attempt is guarded by `policy.per_attempt_timeout` to
/// prevent hanging indefinitely on stalled connections (e.g. WebSocket to
/// SurrealDB). Errors are retried only when `is_transient_db_error` says so,
/// and timeouts are capped by `policy.timeout_attempts` independently of the
/// transient-error budget. Each retry is logged via the provided logger with
/// the operation name.
pub(crate) async fn with_db_retry_policy<T, F, Fut>(
    op_name: &str,
    policy: &DbRetryPolicy,
    logger: &StdoutLogger,
    f: F,
) -> Result<T, MemoryError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, MemoryError>>,
{
    let mut attempt = 0u32;
    loop {
        match tokio::time::timeout(policy.per_attempt_timeout, f()).await {
            Ok(Ok(value)) => return Ok(value),
            Ok(Err(err)) => {
                attempt += 1;
                if attempt >= policy.attempts || !is_transient_db_error(&err) {
                    return Err(err);
                }
                let delay = policy
                    .initial_delay
                    .saturating_mul(1 << attempt.saturating_sub(1).min(6));
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
                            serde_json::Value::Number(serde_json::Number::from(
                                delay.as_millis().min(u128::from(u64::MAX)) as u64,
                            )),
                        ),
                        (
                            "max_attempts".to_string(),
                            serde_json::Value::Number(serde_json::Number::from(policy.attempts)),
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
                attempt += 1;
                if attempt >= policy.timeout_attempts {
                    return Err(MemoryError::Storage(format!(
                        "db.{op_name}: timed out after {}s ({attempt} attempts)",
                        policy.per_attempt_timeout.as_secs()
                    )));
                }
                let delay = policy
                    .initial_delay
                    .saturating_mul(1 << attempt.saturating_sub(1).min(6));
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
                            serde_json::Value::Number(serde_json::Number::from(
                                delay.as_millis().min(u128::from(u64::MAX)) as u64,
                            )),
                        ),
                        (
                            "max_attempts".to_string(),
                            serde_json::Value::Number(serde_json::Number::from(
                                policy.timeout_attempts,
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn logger() -> StdoutLogger {
        StdoutLogger::new("error")
    }

    #[tokio::test]
    async fn succeeds_on_first_attempt() {
        let result =
            with_db_retry("test_op", &logger(), || async { Ok::<_, MemoryError>(42) }).await;
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

    fn fast_policy(timeout_attempts: u32) -> DbRetryPolicy {
        DbRetryPolicy {
            attempts: 3,
            timeout_attempts,
            initial_delay: Duration::from_millis(1),
            per_attempt_timeout: Duration::from_millis(50),
        }
    }

    #[tokio::test]
    async fn stalled_attempt_fails_after_two_timeout_attempts() {
        let calls = Arc::new(AtomicU32::new(0));
        let counter = calls.clone();
        let result: Result<i32, MemoryError> =
            with_db_retry_policy("test_op", &fast_policy(2), &logger(), || {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    Ok(0)
                }
            })
            .await;
        let message = match result {
            Err(MemoryError::Storage(message)) => message,
            other => panic!("expected a timeout storage error, got {other:?}"),
        };
        assert!(message.contains("timed out"), "{message}");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "timeout must not get a third attempt"
        );
    }

    #[tokio::test]
    async fn timeout_then_success_returns_value() {
        let calls = Arc::new(AtomicU32::new(0));
        let counter = calls.clone();
        let result = with_db_retry_policy("test_op", &fast_policy(2), &logger(), || {
            let counter = counter.clone();
            async move {
                let n = counter.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    Ok(0)
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
    async fn transient_conflicts_still_get_three_attempts() {
        let calls = Arc::new(AtomicU32::new(0));
        let counter = calls.clone();
        let policy = DbRetryPolicy {
            attempts: 3,
            timeout_attempts: 1,
            initial_delay: Duration::from_millis(1),
            per_attempt_timeout: Duration::from_secs(30),
        };
        let result: Result<i32, MemoryError> =
            with_db_retry_policy("test_op", &policy, &logger(), || {
                let counter = counter.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(1)).await;
                    Err(MemoryError::Storage("Resource busy".into()))
                }
            })
            .await;
        match result {
            Err(MemoryError::Storage(message)) => assert_eq!(message, "Resource busy"),
            other => panic!("expected the last storage error, got {other:?}"),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }
}
