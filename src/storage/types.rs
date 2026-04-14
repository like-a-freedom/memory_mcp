//! Type definitions for storage operations.

/// Traversal direction for graph neighbor queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphDirection {
    /// Traverse incoming edges pointing to the supplied node.
    Incoming,
    /// Traverse outgoing edges leaving the supplied node.
    Outgoing,
}
