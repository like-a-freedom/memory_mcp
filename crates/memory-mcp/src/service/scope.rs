use crate::config::LifecycleConfig;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LifecyclePolicy {
    pub(crate) archival_age_days: u32,
    pub(crate) decay_confidence_threshold: f64,
    pub(crate) decay_half_life_days: f64,
}

impl Default for LifecyclePolicy {
    fn default() -> Self {
        Self {
            archival_age_days: 90,
            decay_confidence_threshold: 0.3,
            decay_half_life_days: 365.0,
        }
    }
}

impl From<&LifecycleConfig> for LifecyclePolicy {
    fn from(config: &LifecycleConfig) -> Self {
        Self {
            archival_age_days: config.archival_age_days,
            decay_confidence_threshold: config.decay_confidence_threshold,
            decay_half_life_days: config.decay_half_life_days,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_policy_matches_config_defaults() {
        let policy = LifecyclePolicy::default();
        assert_eq!(policy.archival_age_days, 90);
        assert_eq!(policy.decay_confidence_threshold, 0.3);
        assert_eq!(policy.decay_half_life_days, 365.0);
    }
}
