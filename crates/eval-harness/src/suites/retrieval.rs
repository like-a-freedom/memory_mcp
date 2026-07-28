use async_trait::async_trait;

use crate::domain::*;
use crate::runner::{EvalSuite, RunContext};

pub struct RetrievalSuite {
    expected_ids: Vec<EvalCaseId>,
}

impl RetrievalSuite {
    pub fn new(expected_ids: Vec<EvalCaseId>) -> Self {
        Self { expected_ids }
    }
}

#[async_trait]
impl EvalSuite for RetrievalSuite {
    fn id(&self) -> &str {
        "local-retrieval"
    }

    fn mode(&self) -> EvalMode {
        EvalMode::RetrievalOnly
    }

    fn expected_case_ids(&self) -> &[EvalCaseId] {
        &self.expected_ids
    }

    async fn run(&self, _context: &RunContext) -> Vec<EvalCaseOutcome> {
        Vec::new()
    }
}
