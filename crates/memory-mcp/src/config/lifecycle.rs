//! Lifecycle configuration for background jobs.

use super::constants::*;
use super::helpers::{parse_bool_env, parse_env};

/// Configuration for background lifecycle jobs.
///
/// Controls confidence decay refresh and episode archival workers.
/// Both workers are disabled by default and must be explicitly enabled.
///
/// # Environment Variables
///
/// | Variable | Default | Description |
/// |----------|---------|-------------|
/// | `LIFECYCLE_ENABLED` | false | Enable background workers |
/// | `LIFECYCLE_DECAY_INTERVAL_SECS` | 3600 | Decay job interval (seconds) |
/// | `LIFECYCLE_ARCHIVAL_INTERVAL_SECS` | 86400 | Archival job interval (seconds) |
/// | `LIFECYCLE_DECAY_THRESHOLD` | 0.3 | Confidence threshold for invalidation |
/// | `LIFECYCLE_ARCHIVAL_AGE_DAYS` | 90 | Days before episode archival |
/// | `LIFECYCLE_DECAY_HALF_LIFE_DAYS` | 365 | Half-life (days) for decay computation |
///
/// # Examples
///
/// ```rust,no_run
/// use memory_mcp::config::LifecycleConfig;
///
/// let config = LifecycleConfig::from_env();
/// if config.enabled {
///     // Start background workers
/// }
/// ```
#[derive(Debug, Clone)]
pub struct LifecycleConfig {
    /// Enable background lifecycle workers.
    pub enabled: bool,
    /// Interval for decay refresh job (seconds).
    pub decay_interval_secs: u64,
    /// Interval for episode archival job (seconds).
    pub archival_interval_secs: u64,
    /// Confidence threshold below which facts are marked invalid.
    pub decay_confidence_threshold: f64,
    /// Days after which episodes are archived (no active facts).
    pub archival_age_days: u32,
    /// Half-life in days for confidence decay computation.
    pub decay_half_life_days: f64,
}

impl Default for LifecycleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            decay_interval_secs: DEFAULT_DECAY_INTERVAL_SECS,
            archival_interval_secs: DEFAULT_ARCHIVAL_INTERVAL_SECS,
            decay_confidence_threshold: DEFAULT_DECAY_THRESHOLD,
            archival_age_days: DEFAULT_ARCHIVAL_AGE_DAYS,
            decay_half_life_days: DEFAULT_DECAY_HALF_LIFE_DAYS,
        }
    }
}

impl LifecycleConfig {
    /// Loads lifecycle configuration from environment variables.
    ///
    /// # Environment Variables
    ///
    /// | Variable | Default | Description |
    /// |----------|---------|-------------|
    /// | `LIFECYCLE_ENABLED` | false | Enable background workers |
    /// | `LIFECYCLE_DECAY_INTERVAL_SECS` | 3600 | Decay job interval |
    /// | `LIFECYCLE_ARCHIVAL_INTERVAL_SECS` | 86400 | Archival job interval |
    /// | `LIFECYCLE_DECAY_THRESHOLD` | 0.3 | Confidence threshold |
    /// | `LIFECYCLE_ARCHIVAL_AGE_DAYS` | 90 | Episode age threshold |
    /// | `LIFECYCLE_DECAY_HALF_LIFE_DAYS` | 365 | Half-life for decay |
    ///
    /// # Examples
    ///
    /// ```rust
    /// use memory_mcp::config::LifecycleConfig;
    ///
    /// // Ensure the doc test is not affected by a developer's shell
    // environment that pre-sets `LIFECYCLE_ENABLED`.
    /// let _ = std::env::var_os("LIFECYCLE_ENABLED").map(|_| {
    ///     // SAFETY: the doc test runs single-threaded and does not
    /// // touch other process-wide state.
    /// unsafe { std::env::remove_var("LIFECYCLE_ENABLED") }
    /// });
    /// let config = LifecycleConfig::from_env();
    /// assert!(!config.enabled); // disabled by default
    /// ```
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            enabled: parse_bool_env("LIFECYCLE_ENABLED").unwrap_or(false),
            decay_interval_secs: parse_env::<u64>("LIFECYCLE_DECAY_INTERVAL_SECS")
                .ok()
                .flatten()
                .unwrap_or(DEFAULT_DECAY_INTERVAL_SECS),
            archival_interval_secs: parse_env::<u64>("LIFECYCLE_ARCHIVAL_INTERVAL_SECS")
                .ok()
                .flatten()
                .unwrap_or(DEFAULT_ARCHIVAL_INTERVAL_SECS),
            decay_confidence_threshold: parse_env::<f64>("LIFECYCLE_DECAY_THRESHOLD")
                .ok()
                .flatten()
                .unwrap_or(DEFAULT_DECAY_THRESHOLD),
            archival_age_days: parse_env::<u32>("LIFECYCLE_ARCHIVAL_AGE_DAYS")
                .ok()
                .flatten()
                .unwrap_or(DEFAULT_ARCHIVAL_AGE_DAYS),
            decay_half_life_days: parse_env::<f64>("LIFECYCLE_DECAY_HALF_LIFE_DAYS")
                .ok()
                .flatten()
                .unwrap_or(DEFAULT_DECAY_HALF_LIFE_DAYS),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::env;

    use super::*;

    #[test]
    fn lifecycle_config_defaults() {
        let config = LifecycleConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.decay_interval_secs, 3600);
        assert_eq!(config.archival_interval_secs, 86400);
        assert_eq!(config.decay_confidence_threshold, 0.3);
        assert_eq!(config.archival_age_days, 90);
        assert_eq!(config.decay_half_life_days, 365.0);
    }

    #[test]
    fn lifecycle_config_from_env() {
        use super::super::env_lock;

        let _guard = env_lock().lock().expect("env lock");

        unsafe {
            env::set_var("LIFECYCLE_ENABLED", "true");
            env::set_var("LIFECYCLE_DECAY_INTERVAL_SECS", "1800");
            env::set_var("LIFECYCLE_ARCHIVAL_INTERVAL_SECS", "43200");
            env::set_var("LIFECYCLE_DECAY_THRESHOLD", "0.5");
            env::set_var("LIFECYCLE_ARCHIVAL_AGE_DAYS", "60");
            env::set_var("LIFECYCLE_DECAY_HALF_LIFE_DAYS", "180");
        }

        let config = LifecycleConfig::from_env();

        unsafe {
            env::remove_var("LIFECYCLE_ENABLED");
            env::remove_var("LIFECYCLE_DECAY_INTERVAL_SECS");
            env::remove_var("LIFECYCLE_ARCHIVAL_INTERVAL_SECS");
            env::remove_var("LIFECYCLE_DECAY_THRESHOLD");
            env::remove_var("LIFECYCLE_ARCHIVAL_AGE_DAYS");
            env::remove_var("LIFECYCLE_DECAY_HALF_LIFE_DAYS");
        }

        assert!(config.enabled);
        assert_eq!(config.decay_interval_secs, 1800);
        assert_eq!(config.archival_interval_secs, 43200);
        assert_eq!(config.decay_confidence_threshold, 0.5);
        assert_eq!(config.archival_age_days, 60);
        assert_eq!(config.decay_half_life_days, 180.0);
    }
}
