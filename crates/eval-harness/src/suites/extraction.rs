use async_trait::async_trait;

use crate::domain::*;
use crate::runner::{EvalSuite, RunContext};

pub struct ExtractionSuite {
    expected_ids: Vec<EvalCaseId>,
}

impl ExtractionSuite {
    pub fn new(expected_ids: Vec<EvalCaseId>) -> Self {
        Self { expected_ids }
    }
}

#[async_trait]
impl EvalSuite for ExtractionSuite {
    fn id(&self) -> &str {
        "extraction"
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
