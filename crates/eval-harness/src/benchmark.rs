use std::time::Duration;

use crate::error::EvalError;

#[derive(Debug, Clone)]
pub struct NerBenchmarkFixture {
    pub model_name: String,
    pub model_digest: String,
    pub labels: Vec<String>,
    pub threshold: f32,
    pub single_window: String,
    pub multi_window: String,
}

impl NerBenchmarkFixture {
    pub fn load() -> Result<Self, EvalError> {
        Ok(Self {
            model_name: "urchade/gliner_multi-v2.1".into(),
            model_digest: "a".repeat(64),
            labels: vec![
                "person".into(),
                "organization".into(),
                "location".into(),
                "date".into(),
            ],
            threshold: 0.5,
            single_window: "Alice Smith from Acme Corp presented the quarterly revenue report showing $5.2M in ARR."
                .into(),
            multi_window: (0..10)
                .map(|i| format!("Window {i}: Alice Smith from Acme Corp reported revenue milestone {i}."))
                .collect::<Vec<_>>()
                .join(". "),
        })
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

    pub fn metadata_only() -> Self {
        Self::load().expect("metadata should always load")
    }
}

#[derive(Debug, Clone)]
pub struct NerOutput {
    pub entities: Vec<NerEntity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NerEntity {
    pub text: String,
    pub label: String,
    pub start: usize,
    pub end: usize,
}

impl NerOutput {
    pub fn canonical(&self) -> Vec<NerEntity> {
        self.entities.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnsupportedDevice {
    Metal,
}

/// Metadata/device capability probe used by benchmark setup.
///
/// The measured NER paths live in the Criterion benches and call the
/// production `MemoryService::extract` pipeline directly.  This helper is
/// intentionally not an inference runner; returning an error prevents tests
/// from accidentally publishing synthetic entity counts as latency evidence.
pub struct NerRunner;

impl NerRunner {
    pub fn cpu(fixture: &NerBenchmarkFixture) -> Result<Self, EvalError> {
        let _ = fixture;
        Ok(Self)
    }

    pub fn metal(_fixture: &NerBenchmarkFixture) -> Result<Self, UnsupportedDevice> {
        if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
            Ok(Self)
        } else {
            Err(UnsupportedDevice::Metal)
        }
    }

    pub fn extract(&self, input: &str) -> Result<NerOutput, EvalError> {
        let _ = input;
        Err(EvalError::Suite(
            "NerRunner is metadata-only; use the production extraction path for measurements"
                .into(),
        ))
    }
}

pub fn assert_candidate_parity(cpu: &[NerEntity], metal: &[NerEntity]) -> Result<(), EvalError> {
    if cpu.len() != metal.len() {
        return Err(EvalError::Suite(format!(
            "parity mismatch: cpu={} metal={}",
            cpu.len(),
            metal.len()
        )));
    }
    for (c, m) in cpu.iter().zip(metal.iter()) {
        if c != m {
            return Err(EvalError::Suite(format!(
                "parity mismatch: cpu={c:?} metal={m:?}"
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct BenchmarkProvenance {
    pub model: String,
    pub model_digest: String,
    pub device: String,
    pub labels: Vec<String>,
    pub threshold: f32,
    pub input_digest: String,
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
    fn ner_fixture_loads_with_required_fields() {
        let fixture = NerBenchmarkFixture::load().unwrap();
        assert_eq!(fixture.model_digest.len(), 64);
        assert!(!fixture.labels.is_empty());
        assert!((0.0..=1.0).contains(&fixture.threshold));
        assert!(!fixture.single_window().is_empty());
        assert!(!fixture.multi_window().is_empty());
    }

    #[test]
    fn multi_window_exceeds_single_window() {
        let fixture = NerBenchmarkFixture::load().unwrap();
        assert!(
            fixture.multi_window_token_count() > fixture.single_window().split_whitespace().count()
        );
    }

    #[test]
    fn parity_check_passes_for_identical_outputs() {
        let entities = vec![NerEntity {
            text: "Alice".into(),
            label: "person".into(),
            start: 0,
            end: 5,
        }];
        assert!(assert_candidate_parity(&entities, &entities).is_ok());
    }

    #[test]
    fn parity_check_fails_for_different_outputs() {
        let cpu = vec![NerEntity {
            text: "Alice".into(),
            label: "person".into(),
            start: 0,
            end: 5,
        }];
        let metal = vec![NerEntity {
            text: "Bob".into(),
            label: "person".into(),
            start: 0,
            end: 3,
        }];
        assert!(assert_candidate_parity(&cpu, &metal).is_err());
    }
}
