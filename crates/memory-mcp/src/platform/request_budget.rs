//! Ambient per-request time budget.
//!
//! A request-serving path can install an absolute deadline for the duration of
//! its future. The database retry loop bounds each attempt by the time left, so
//! a stalled query answers once within the time the caller actually has instead
//! of running out a fixed per-attempt timeout and then retrying. Outside a
//! request (CLI, embedded, tests) no deadline is set and the configured
//! per-attempt timeout applies unchanged.
//!
//! This is a task-local, so it follows the request's future without being
//! threaded through every port signature — the same mechanism the correlation
//! id uses ([`crate::logging::correlation`]).

use std::time::{Duration, Instant};

tokio::task_local! {
    static REQUEST_DEADLINE: Instant;
}

/// Runs `future` with `deadline` as the ambient request deadline.
///
/// Only a request-serving path installs a deadline, so this is compiled where
/// the HTTP surface is (and for the tests here).
#[cfg(any(test, feature = "streamable-http"))]
pub(crate) async fn scope<F>(deadline: Instant, future: F) -> F::Output
where
    F: std::future::Future,
{
    REQUEST_DEADLINE.scope(deadline, future).await
}

/// The time left before the ambient request deadline, if one is set.
#[must_use]
pub(crate) fn remaining() -> Option<Duration> {
    REQUEST_DEADLINE
        .try_with(|deadline| deadline.saturating_duration_since(Instant::now()))
        .ok()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{remaining, scope};

    #[tokio::test]
    async fn remaining_is_none_outside_a_request_scope() {
        assert!(remaining().is_none());
    }

    #[tokio::test]
    async fn remaining_counts_down_inside_a_scope() {
        let budget = scope(Instant::now() + Duration::from_secs(5), async {
            remaining()
        })
        .await;
        let budget = budget.expect("a deadline is set inside the scope");
        assert!(budget <= Duration::from_secs(5));
        assert!(budget > Duration::from_secs(4));
    }

    #[tokio::test]
    async fn remaining_is_none_again_after_the_scope_ends() {
        let _ = scope(Instant::now() + Duration::from_secs(5), async {}).await;
        assert!(remaining().is_none());
    }
}
