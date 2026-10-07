use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::error::MemoryError;

/// A nonblocking, process-wide reservation for requests whose bodies are
/// collected by MCP preflight before ordinary admission runs.
pub struct PreflightBudget {
    requests: Arc<Semaphore>,
    byte_limit: usize,
    reserved_bytes: AtomicUsize,
}

/// The capacity that prevented a preflight reservation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreflightRefusal {
    Requests,
    Bytes,
}

/// Holds one request slot and its currently accounted body bytes until dropped.
pub struct PreflightReservation {
    _request: OwnedSemaphorePermit,
    budget: Arc<PreflightBudget>,
    reserved_bytes: usize,
}

impl PreflightBudget {
    pub fn new(request_limit: usize, byte_limit: usize) -> Result<Self, MemoryError> {
        if request_limit == 0 || request_limit > Semaphore::MAX_PERMITS || byte_limit == 0 {
            return Err(MemoryError::ConfigInvalid(
                "HTTP preflight request and byte limits must be positive and request limit must not exceed Tokio's semaphore maximum"
                    .into(),
            ));
        }

        Ok(Self {
            requests: Arc::new(Semaphore::new(request_limit)),
            byte_limit,
            reserved_bytes: AtomicUsize::new(0),
        })
    }

    pub fn try_reserve(
        self: &Arc<Self>,
        initial_bytes: usize,
    ) -> Result<PreflightReservation, PreflightRefusal> {
        let request = self
            .requests
            .clone()
            .try_acquire_owned()
            .map_err(|_| PreflightRefusal::Requests)?;
        self.try_charge_bytes(initial_bytes)?;

        Ok(PreflightReservation {
            _request: request,
            budget: Arc::clone(self),
            reserved_bytes: initial_bytes,
        })
    }

    fn try_charge_bytes(&self, additional_bytes: usize) -> Result<(), PreflightRefusal> {
        let mut reserved = self.reserved_bytes.load(Ordering::Acquire);
        loop {
            let Some(next) = reserved.checked_add(additional_bytes) else {
                return Err(PreflightRefusal::Bytes);
            };
            if next > self.byte_limit {
                return Err(PreflightRefusal::Bytes);
            }
            match self.reserved_bytes.compare_exchange_weak(
                reserved,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(actual) => reserved = actual,
            }
        }
    }
}

impl PreflightReservation {
    /// Increase this request's aggregate byte reservation through `total_bytes`.
    /// The caller invokes this before copying a newly observed body frame.
    pub(super) fn try_reserve_through(
        &mut self,
        total_bytes: usize,
    ) -> Result<(), PreflightRefusal> {
        if total_bytes <= self.reserved_bytes {
            return Ok(());
        }
        let additional_bytes = total_bytes - self.reserved_bytes;
        self.budget.try_charge_bytes(additional_bytes)?;
        self.reserved_bytes = total_bytes;
        Ok(())
    }
}

impl Drop for PreflightReservation {
    fn drop(&mut self) {
        self.budget
            .reserved_bytes
            .fetch_sub(self.reserved_bytes, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{PreflightBudget, PreflightRefusal};

    #[test]
    fn byte_refusal_returns_acquired_slot() {
        let budget = Arc::new(PreflightBudget::new(2, 10).expect("valid budget"));
        let first = budget.try_reserve(10).expect("first reservation fits");

        assert!(matches!(
            budget.try_reserve(1),
            Err(PreflightRefusal::Bytes)
        ));
        drop(first);
        assert!(budget.try_reserve(10).is_ok());
    }

    #[test]
    fn slot_refusal_does_not_charge_bytes() {
        let budget = Arc::new(PreflightBudget::new(1, 10).expect("valid budget"));
        let first = budget.try_reserve(5).expect("first reservation fits");

        assert!(matches!(
            budget.try_reserve(1),
            Err(PreflightRefusal::Requests)
        ));
        drop(first);
        assert!(budget.try_reserve(10).is_ok());
    }

    #[test]
    fn drop_restores_both_resources() {
        let budget = Arc::new(PreflightBudget::new(1, 10).expect("valid budget"));
        let reservation = budget.try_reserve(10).expect("reservation fits");

        assert!(matches!(
            budget.try_reserve(1),
            Err(PreflightRefusal::Requests)
        ));
        drop(reservation);
        assert!(budget.try_reserve(10).is_ok());
    }

    #[test]
    fn reservation_growth_is_bounded_and_released() {
        let budget = Arc::new(PreflightBudget::new(2, 10).expect("valid budget"));
        let mut first = budget.try_reserve(2).expect("initial bytes fit");
        first
            .try_reserve_through(8)
            .expect("growth within aggregate byte limit fits");

        assert!(matches!(
            budget.try_reserve(3),
            Err(PreflightRefusal::Bytes)
        ));
        drop(first);
        assert!(budget.try_reserve(10).is_ok());
    }

    #[test]
    fn byte_ledger_rejects_checked_overflow_and_recovers() {
        let budget =
            Arc::new(PreflightBudget::new(2, usize::MAX).expect("maximum byte limit is valid"));
        let mut first = budget
            .try_reserve(usize::MAX - 1)
            .expect("first reservation fits");

        assert!(matches!(
            budget.try_reserve(2),
            Err(PreflightRefusal::Bytes)
        ));
        let second = budget
            .try_reserve(1)
            .expect("failed byte reservation must return its request slot");
        drop(second);

        first
            .try_reserve_through(usize::MAX)
            .expect("checked byte-ledger growth fits exactly");
        drop(first);
        assert!(budget.try_reserve(usize::MAX).is_ok());
    }

    #[test]
    fn constructor_rejects_zero_and_excessive_request_limits() {
        assert!(PreflightBudget::new(0, 1).is_err());
        assert!(PreflightBudget::new(1, 0).is_err());
        assert!(PreflightBudget::new(tokio::sync::Semaphore::MAX_PERMITS + 1, 1).is_err());
    }
}
