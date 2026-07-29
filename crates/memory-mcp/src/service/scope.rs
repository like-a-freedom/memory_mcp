use crate::config::LifecycleConfig;
use crate::service::MemoryError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemoryScope {
    Personal,
    Team,
    Org,
    PrivateDomain,
}

impl MemoryScope {
    pub(crate) fn parse(raw: &str) -> Result<Self, MemoryError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "personal" => Ok(Self::Personal),
            "team" => Ok(Self::Team),
            "org" => Ok(Self::Org),
            "private-domain" | "private_domain" | "private" => Ok(Self::PrivateDomain),
            other => Err(MemoryError::Validation(format!("unknown scope: {other}"))),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Personal => "personal",
            Self::Team => "team",
            Self::Org => "org",
            Self::PrivateDomain => "private-domain",
        }
    }

    pub(crate) fn namespace(self, namespaces: &[String]) -> Result<String, MemoryError> {
        let candidates = match self {
            Self::Personal => &["personal"][..],
            Self::Team => &["team", "org"][..],
            Self::Org => &["org"][..],
            Self::PrivateDomain => &["private-domain", "private"][..],
        };

        candidates
            .iter()
            .find_map(|candidate| namespaces.iter().find(|ns| ns.as_str() == *candidate))
            .cloned()
            .ok_or_else(|| {
                MemoryError::Validation(format!(
                    "no namespace configured for scope {}",
                    self.as_str()
                ))
            })
    }
}

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
    fn parse_scope_accepts_team_and_private_domain() {
        assert_eq!(MemoryScope::parse("team").unwrap().as_str(), "team");
        assert_eq!(
            MemoryScope::parse("private-domain").unwrap().as_str(),
            "private-domain"
        );
        assert_eq!(
            MemoryScope::parse("private").unwrap().as_str(),
            "private-domain"
        );
        assert_eq!(
            MemoryScope::parse("private_domain").unwrap().as_str(),
            "private-domain"
        );
    }

    #[test]
    fn parse_scope_rejects_unknown_scope() {
        let err = MemoryScope::parse("org-typo").unwrap_err();
        assert!(matches!(err, MemoryError::Validation(_)));
    }

    #[test]
    fn lifecycle_policy_matches_config_defaults() {
        let policy = LifecyclePolicy::default();
        assert_eq!(policy.archival_age_days, 90);
        assert_eq!(policy.decay_confidence_threshold, 0.3);
        assert_eq!(policy.decay_half_life_days, 365.0);
    }
}
