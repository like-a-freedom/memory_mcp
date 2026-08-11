use std::time::Duration;

/// Shared input fixture for the NER latency benches.
///
/// The window texts are deliberately static so bench results stay comparable
/// across runs and machines. Latency measurements themselves live in the
/// Criterion benches, which call the production `EntityExtractor` path
/// directly; this module only supplies inputs and pure helpers.
#[derive(Debug, Clone)]
pub struct NerBenchmarkFixture {
    single_window: String,
    multi_window: String,
}

impl NerBenchmarkFixture {
    pub fn load() -> Self {
        Self {
            single_window: "Alice Smith from Acme Corp presented the quarterly revenue report showing $5.2M in ARR."
                .into(),
            multi_window: (0..10)
                .map(|i| format!("Window {i}: Alice Smith from Acme Corp reported revenue milestone {i}."))
                .collect::<Vec<_>>()
                .join(". "),
        }
    }

    pub fn single_window(&self) -> &str {
        &self.single_window
    }

    pub fn multi_window(&self) -> &str {
        &self.multi_window
    }

    pub fn multi_window_token_count(&self) -> usize {
        self.multi_window.split_whitespace().count()
    }
}

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

    #[test]
    fn ner_fixture_loads_window_texts() {
        let fixture = NerBenchmarkFixture::load();
        assert!(!fixture.single_window().is_empty());
        assert!(!fixture.multi_window().is_empty());
        assert!(fixture.multi_window_token_count() > 0);
    }

    #[test]
    fn multi_window_exceeds_single_window() {
        let fixture = NerBenchmarkFixture::load();
        assert!(
            fixture.multi_window_token_count() > fixture.single_window().split_whitespace().count()
        );
    }
}
