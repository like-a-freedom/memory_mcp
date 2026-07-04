use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Debug, Clone)]
pub(super) struct InferenceGate {
    permits: Arc<Semaphore>,
}

impl InferenceGate {
    pub(super) fn new(max_concurrency: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(max_concurrency)),
        }
    }

    pub(super) async fn acquire(
        &self,
    ) -> Result<(OwnedSemaphorePermit, Duration), tokio::sync::AcquireError> {
        let started = Instant::now();
        let permit = self.permits.clone().acquire_owned().await?;
        Ok((permit, started.elapsed()))
    }

    pub(super) fn available_permits(&self) -> usize {
        self.permits.available_permits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn second_caller_waits_until_the_only_permit_is_released() {
        let gate = InferenceGate::new(1);
        let (first, _) = gate.acquire().await.expect("first permit");
        assert!(
            tokio::time::timeout(Duration::from_millis(10), gate.acquire())
                .await
                .is_err()
        );
        drop(first);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), gate.acquire())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn configured_parallelism_is_available() {
        let gate = InferenceGate::new(2);
        let (_first, _) = gate.acquire().await.expect("first permit");
        let (_second, _) = gate.acquire().await.expect("second permit");
        assert_eq!(gate.available_permits(), 0);
    }
}
