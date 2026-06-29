-- Migration 023: Add composite indexes on the edge table for graph traversal
-- and bi-temporal query optimization.
--
-- Existing single-column indexes (edge_in, edge_out, edge_relation) remain.
-- These composite indexes accelerate common multi-column WHERE patterns.

-- Composite index for graph traversal: find all edges between two nodes.
DEFINE INDEX edge_from_to_idx ON edge COLUMNS in, out;

-- Composite index for bi-temporal graph queries: find active edges
-- from a specific node within a time range.
DEFINE INDEX edge_temporal_idx ON edge COLUMNS in, out, t_valid, t_invalid;
