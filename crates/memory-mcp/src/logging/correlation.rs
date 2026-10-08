//! Ambient request-id correlation.
//!
//! One id per unit of work, carried out-of-band so producers never thread it
//! through their signatures. The formatter reads [`current`] and stamps it on
//! every event that does not carry its own. See
//! [ADR-0080](../../../docs/adr/0080-one-request-identity.md).

tokio::task_local! {
    static REQUEST_ID: String;
}

/// The correlation id of the unit of work running on this task, if any.
pub fn current() -> Option<String> {
    REQUEST_ID.try_with(Clone::clone).ok()
}

/// Run `future` with `id` as the ambient correlation id.
pub async fn scope<F: std::future::Future>(id: impl Into<String>, future: F) -> F::Output {
    REQUEST_ID.scope(id.into(), future).await
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn current_is_none_outside_a_scope() {
        assert_eq!(crate::logging::correlation::current(), None);
    }

    #[tokio::test]
    async fn scope_makes_the_id_visible_inside() {
        let seen = crate::logging::correlation::scope("req_a", async {
            crate::logging::correlation::current()
        })
        .await;
        assert_eq!(seen.as_deref(), Some("req_a"));
    }

    #[tokio::test]
    async fn concurrent_scopes_do_not_cross_tasks() {
        let a = tokio::spawn(crate::logging::correlation::scope("req_a", async {
            tokio::task::yield_now().await;
            crate::logging::correlation::current()
        }));
        let b = tokio::spawn(crate::logging::correlation::scope("req_b", async {
            tokio::task::yield_now().await;
            crate::logging::correlation::current()
        }));
        assert_eq!(a.await.unwrap().as_deref(), Some("req_a"));
        assert_eq!(b.await.unwrap().as_deref(), Some("req_b"));
    }
}
