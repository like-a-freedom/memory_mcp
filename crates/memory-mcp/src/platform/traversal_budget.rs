//! Bounds on a graph traversal, shared by every caller.
//!
//! The struct caps how much of the graph one query may expand. It names
//! no context and no store, so it sits in the platform layer rather than
//! in either the read path or the write path that use it.

/// Budget controls for graph traversal to prevent query explosion in different contexts.
#[derive(Debug, Clone, Copy)]
pub struct GraphTraversalBudget {
    pub max_hub_scan: usize,
    pub max_node_expansions: usize,
    pub max_neighbor_queries: usize,
    pub max_results: usize,
}

/// The widest hub scan any traversal may run.
const MAX_HUB_CANDIDATE_SCAN: usize = 64;
const MAX_SURPRISING_CONNECTION_NODE_EXPANSIONS: usize = 64;
const MAX_SURPRISING_CONNECTION_NEIGHBOR_QUERIES: usize = 128;
const MAX_SURPRISING_CONNECTION_RESULTS: usize = 12;

impl GraphTraversalBudget {
    /// Full budget — used by dedicated graph exploration (open_app, context views).
    pub const FULL: Self = Self {
        max_hub_scan: MAX_HUB_CANDIDATE_SCAN,
        max_node_expansions: MAX_SURPRISING_CONNECTION_NODE_EXPANSIONS,
        max_neighbor_queries: MAX_SURPRISING_CONNECTION_NEIGHBOR_QUERIES,
        max_results: MAX_SURPRISING_CONNECTION_RESULTS,
    };

    /// Reduced budget — used by inline `explain` calls to avoid per-item query explosion.
    pub const EXPLAIN: Self = Self {
        max_hub_scan: 24,
        max_node_expansions: 16,
        max_neighbor_queries: 32,
        max_results: 5,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `EXPLAIN` exists to stop an inline call from expanding as much as a
    /// dedicated one, so every field must be strictly smaller. A field that
    /// matched `FULL` would leave that call unprotected while still looking
    /// like it had a budget.
    #[test]
    fn explain_budget_is_stricter_than_full() {
        const {
            assert!(
                GraphTraversalBudget::EXPLAIN.max_hub_scan
                    < GraphTraversalBudget::FULL.max_hub_scan
            );
            assert!(
                GraphTraversalBudget::EXPLAIN.max_node_expansions
                    < GraphTraversalBudget::FULL.max_node_expansions
            );
            assert!(
                GraphTraversalBudget::EXPLAIN.max_neighbor_queries
                    < GraphTraversalBudget::FULL.max_neighbor_queries
            );
            assert!(
                GraphTraversalBudget::EXPLAIN.max_results < GraphTraversalBudget::FULL.max_results
            );
        }
    }

    /// The budget is threaded through awaits, so it has to be `Copy`: moving
    /// it would force every caller to clone before each loop iteration.
    #[test]
    fn graph_traversal_budget_is_copy() {
        let a = GraphTraversalBudget::FULL;
        let b = a; // Copy, not move
        assert_eq!(a.max_hub_scan, b.max_hub_scan);
    }

    /// A zero in any field would silently disable that bound rather than
    /// tightening it, so every field is checked for being a real limit.
    #[test]
    fn graph_traversal_budget_constants_are_nonzero() {
        for budget in [GraphTraversalBudget::FULL, GraphTraversalBudget::EXPLAIN] {
            assert!(budget.max_hub_scan > 0);
            assert!(budget.max_node_expansions > 0);
            assert!(budget.max_neighbor_queries > 0);
            assert!(budget.max_results > 0);
        }
    }
}
