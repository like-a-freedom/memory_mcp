//! Regression verdict vs baseline
//!
//! Task 9 wires the runner-level verdict (budget, evidence health); baseline
//! gate evaluation arrives in Task 10.

use super::artifact::PerfArtifact;
use super::profile::PerfProfile;

/// Outcome of a perf run: pass, regression against a baseline, or an
/// invalid run (broken evidence — budget, missing samples, host mismatch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PerfVerdict {
    Passed,
    Regressed(Vec<String>),
    Invalid(Vec<String>),
}

impl PerfVerdict {
    pub fn reasons(&self) -> &[String] {
        match self {
            Self::Passed => &[],
            Self::Regressed(reasons) | Self::Invalid(reasons) => reasons,
        }
    }
}

/// Verdict against an optional baseline artifact. Without a baseline the
/// gates are skipped — the first run defines the baseline (Task 10 adds
/// host-tag and gate evaluation).
pub fn evaluate(
    _profile: &PerfProfile,
    _artifact: &PerfArtifact,
    _baseline: Option<&PerfArtifact>,
) -> PerfVerdict {
    PerfVerdict::Passed
}
