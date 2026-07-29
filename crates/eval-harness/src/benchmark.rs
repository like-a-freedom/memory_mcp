use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentionObservation {
    pub clients: usize,
    pub operations: usize,
    pub elapsed: Duration,
}

impl ContentionObservation {
    pub fn new(clients: usize, operations: usize, elapsed: Duration) -> Self {
        Self {
            clients,
            operations,
            elapsed,
        }
    }

    pub fn ops_per_second(&self) -> f64 {
        let secs = self.elapsed.as_secs_f64();
        if secs <= 0.0 {
            return 0.0;
        }
        self.operations as f64 / secs
    }

    pub fn latency_per_operation(&self) -> Duration {
        if self.operations == 0 {
            return Duration::ZERO;
        }
        self.elapsed / self.operations as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contention_normalizes_by_completed_operations() {
        let observation = ContentionObservation::new(4, 12, Duration::from_millis(300));
        assert_eq!(observation.ops_per_second(), 40.0);
        assert_eq!(
            observation.latency_per_operation(),
            Duration::from_millis(25)
        );
    }

    #[test]
    fn zero_operations_returns_zero_latency() {
        let observation = ContentionObservation::new(1, 0, Duration::from_millis(100));
        assert_eq!(observation.latency_per_operation(), Duration::ZERO);
    }

    #[test]
    fn zero_elapsed_returns_zero_throughput() {
        let observation = ContentionObservation::new(1, 10, Duration::ZERO);
        assert_eq!(observation.ops_per_second(), 0.0);
    }
}
