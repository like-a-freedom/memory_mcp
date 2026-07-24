//! Configuration and outcome types for the reembed maintenance command.

/// Options controlling reembed behavior.
///
/// Passed from CLI flags into `MemoryService::reembed_all_facts`.
#[derive(Debug, Clone)]
pub struct ReembedOptions {
    /// Maximum number of failed facts before aborting the run.
    ///
    /// `None` means use the default quota (10% of total, minimum 10).
    /// `Some(0)` means fail-fast on the first error (legacy behavior).
    pub max_failures: Option<usize>,
    /// If true, retry only facts marked as failed in a previous run.
    pub retry_failed: bool,
}

impl Default for ReembedOptions {
    fn default() -> Self {
        Self {
            max_failures: None,
            retry_failed: false,
        }
    }
}

impl ReembedOptions {
    /// Returns the effective max failures cap, applying the default quota
    /// (10% of total, minimum 10) when `max_failures` is `None`.
    ///
    /// # Examples
    ///
    /// ```
    /// use memory_mcp::service::reembed_options::ReembedOptions;
    ///
    /// let opts = ReembedOptions::default();
    /// assert_eq!(opts.effective_max_failures(100), 10);
    /// assert_eq!(opts.effective_max_failures(1000), 100);
    /// ```
    pub fn effective_max_failures(&self, total_facts: usize) -> usize {
        match self.max_failures {
            Some(0) => 0,
            Some(n) => n,
            None => {
                let ten_percent = total_facts / 10;
                ten_percent.max(10)
            }
        }
    }
}

/// Final outcome of a reembed run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReembedOutcome {
    /// All facts processed successfully, no failures.
    Completed,
    /// All facts processed, but some failures occurred within quota.
    CompletedWithErrors,
    /// Aborted because failure count exceeded the quota.
    Failed,
    /// Interrupted by the user (Ctrl+C).
    Interrupted,
    /// Nothing to do — all embeddings already match the target signature
    /// and no failed facts to retry.
    NothingToDo,
}

impl ReembedOutcome {
    /// Returns the process exit code for this outcome.
    ///
    /// `0` for success states, `1` for failure, `130` for SIGINT (Ctrl+C).
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Completed | Self::CompletedWithErrors | Self::NothingToDo => 0,
            Self::Failed => 1,
            Self::Interrupted => 130,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_quota_is_ten_percent_minimum_ten() {
        let opts = ReembedOptions::default();
        assert_eq!(opts.effective_max_failures(100), 10);
        assert_eq!(opts.effective_max_failures(50), 10);
        assert_eq!(opts.effective_max_failures(1000), 100);
        assert_eq!(opts.effective_max_failures(0), 10);
    }

    #[test]
    fn explicit_max_failures_overrides_default() {
        let opts = ReembedOptions {
            max_failures: Some(5),
            retry_failed: false,
        };
        assert_eq!(opts.effective_max_failures(1000), 5);
    }

    #[test]
    fn zero_max_failures_means_fail_fast() {
        let opts = ReembedOptions {
            max_failures: Some(0),
            retry_failed: false,
        };
        assert_eq!(opts.effective_max_failures(1000), 0);
    }

    #[test]
    fn exit_codes_match_conventions() {
        assert_eq!(ReembedOutcome::Completed.exit_code(), 0);
        assert_eq!(ReembedOutcome::CompletedWithErrors.exit_code(), 0);
        assert_eq!(ReembedOutcome::NothingToDo.exit_code(), 0);
        assert_eq!(ReembedOutcome::Failed.exit_code(), 1);
        assert_eq!(ReembedOutcome::Interrupted.exit_code(), 130);
    }
}
