//! Claim reconciliation rollout configuration.

use crate::service::MemoryError;

/// Rollout stage for claim reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ClaimRolloutStage {
    /// No claim extraction or reconciliation.
    Disabled,
    /// Extract and store claims but do not expose relations. Default.
    #[default]
    Shadow,
    /// Persist relation decisions without exposing them.
    Relations,
    /// Also expose authorized evidence and context.
    Evidence,
}

impl ClaimRolloutStage {
    pub(crate) const fn projects(self) -> bool {
        !matches!(self, Self::Disabled)
    }
    #[allow(dead_code)]
    pub(crate) const fn evaluates_relations(self) -> bool {
        !matches!(self, Self::Disabled)
    }
    #[allow(dead_code)]
    pub(crate) const fn persists_relations(self) -> bool {
        matches!(self, Self::Relations | Self::Evidence)
    }
    #[allow(dead_code)]
    pub(crate) const fn exposes_evidence(self) -> bool {
        matches!(self, Self::Evidence)
    }
}

impl std::fmt::Display for ClaimRolloutStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => write!(f, "disabled"),
            Self::Shadow => write!(f, "shadow"),
            Self::Relations => write!(f, "relations"),
            Self::Evidence => write!(f, "evidence"),
        }
    }
}

impl std::str::FromStr for ClaimRolloutStage {
    type Err = MemoryError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        match trimmed.to_lowercase().as_str() {
            "disabled" => Ok(Self::Disabled),
            "shadow" => Ok(Self::Shadow),
            "relations" => Ok(Self::Relations),
            "evidence" => Ok(Self::Evidence),
            "lifecycle" => Err(MemoryError::ConfigInvalid(
                "MEMORY_CLAIM_ROLLOUT_STAGE=lifecycle is not shipped. \
                 Automatic lifecycle effects are disabled until a separate safety review. \
                 Use: disabled, shadow, relations, evidence"
                    .to_string(),
            )),
            _ => Err(MemoryError::ConfigInvalid(format!(
                "unknown MEMORY_CLAIM_ROLLOUT_STAGE: '{trimmed}'. Valid: disabled, shadow, relations, evidence"
            ))),
        }
    }
}

/// Configuration for the claim reconciliation pipeline.
#[derive(Debug, Clone)]
pub(crate) struct ClaimConfig {
    pub rollout_stage: ClaimRolloutStage,
    pub candidate_page_size: usize,
    pub inline_candidate_limit: usize,
    pub inline_budget: std::time::Duration,
}

impl Default for ClaimConfig {
    fn default() -> Self {
        Self {
            rollout_stage: ClaimRolloutStage::default(),
            candidate_page_size: 256,
            inline_candidate_limit: 1024,
            inline_budget: std::time::Duration::from_millis(50),
        }
    }
}

impl ClaimConfig {
    /// Parse configuration from environment variables.
    #[allow(dead_code)]
    pub fn from_env() -> Result<Self, MemoryError> {
        let mut config = Self::default();
        if let Ok(stage_str) = std::env::var("MEMORY_CLAIM_ROLLOUT_STAGE") {
            config.rollout_stage = stage_str.parse()?;
        }
        if let Ok(page_size) = std::env::var("MEMORY_CLAIM_CANDIDATE_PAGE_SIZE") {
            config.candidate_page_size = page_size.parse().map_err(|_| {
                MemoryError::ConfigInvalid(format!(
                    "MEMORY_CLAIM_CANDIDATE_PAGE_SIZE must be a positive integer, got '{page_size}'"
                ))
            })?;
        }
        if let Ok(limit) = std::env::var("MEMORY_CLAIM_INLINE_CANDIDATE_LIMIT") {
            config.inline_candidate_limit = limit.parse().map_err(|_| {
                MemoryError::ConfigInvalid(format!(
                    "MEMORY_CLAIM_INLINE_CANDIDATE_LIMIT must be a positive integer, got '{limit}'"
                ))
            })?;
        }
        if let Ok(ms) = std::env::var("MEMORY_CLAIM_INLINE_BUDGET_MS") {
            let millis: u64 = ms.parse().map_err(|_| {
                MemoryError::ConfigInvalid(format!(
                    "MEMORY_CLAIM_INLINE_BUDGET_MS must be a positive integer, got '{ms}'"
                ))
            })?;
            config.inline_budget = std::time::Duration::from_millis(millis);
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_stage_is_shadow() {
        assert_eq!(ClaimRolloutStage::default(), ClaimRolloutStage::Shadow);
    }

    #[test]
    fn parse_all_stages() {
        for (input, expected) in [
            ("disabled", ClaimRolloutStage::Disabled),
            ("shadow", ClaimRolloutStage::Shadow),
            ("relations", ClaimRolloutStage::Relations),
            ("evidence", ClaimRolloutStage::Evidence),
            ("EVIDENCE", ClaimRolloutStage::Evidence),
        ] {
            assert_eq!(input.parse::<ClaimRolloutStage>().unwrap(), expected);
        }
    }

    #[test]
    fn parse_lifecycle_returns_config_invalid() {
        let result = "lifecycle".parse::<ClaimRolloutStage>();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not shipped"));
    }

    #[test]
    fn parse_unknown_stage_returns_error() {
        let result = "banana".parse::<ClaimRolloutStage>();
        assert!(result.is_err());
    }

    #[test]
    fn default_config_values() {
        let config = ClaimConfig::default();
        assert_eq!(config.rollout_stage, ClaimRolloutStage::Shadow);
        assert_eq!(config.candidate_page_size, 256);
        assert_eq!(config.inline_candidate_limit, 1024);
        assert_eq!(config.inline_budget, std::time::Duration::from_millis(50));
    }

    #[test]
    fn stage_display_roundtrip() {
        for stage in [
            ClaimRolloutStage::Disabled,
            ClaimRolloutStage::Shadow,
            ClaimRolloutStage::Relations,
            ClaimRolloutStage::Evidence,
        ] {
            let s = stage.to_string();
            let parsed: ClaimRolloutStage = s.parse().unwrap();
            assert_eq!(parsed, stage);
        }
    }
}
