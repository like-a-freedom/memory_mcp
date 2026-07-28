use async_trait::async_trait;

use crate::domain::*;
use crate::runner::{EvalSuite, RunContext};

pub struct ClaimReconciliationSuite {
    expected_ids: Vec<EvalCaseId>,
}

impl ClaimReconciliationSuite {
    pub fn new(expected_ids: Vec<EvalCaseId>) -> Self {
        Self { expected_ids }
    }
}

#[async_trait]
impl EvalSuite for ClaimReconciliationSuite {
    fn id(&self) -> &str {
        "claim-reconciliation"
    }

    fn mode(&self) -> EvalMode {
        EvalMode::EndToEnd
    }

    fn expected_case_ids(&self) -> &[EvalCaseId] {
        &self.expected_ids
    }

    async fn run(&self, _context: &RunContext) -> Vec<EvalCaseOutcome> {
        Vec::new()
    }
}
