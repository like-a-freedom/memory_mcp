//! HTTP-profile embedding maintenance.
//!
//! Two maintenance operations live here because they are deliberately
//! separate. *Backfill* fills `embedding IS NONE` in place and never touches an
//! existing vector or the HNSW index; it is automatic, because a fact with no
//! vector is a gap rather than a disagreement, and a deployment that can fill it
//! should not need an operator. *Reembed* rewrites every vector and owns index
//! replacement; it is reachable only through an operator-triggered
//! control-plane request, because a namespace whose vectors were written by a
//! different provider is a Class B mismatch that only a rewrite can exit.
//!
//! The activation-path index reconcile is the third thing, and its boundary is
//! the whole point of this module's shape: it may re-declare a tenant's index
//! only for a namespace that holds no vectors (see
//! `http::runtime::storage::reconcile_tenant_index_dimension`).

pub mod backfill_scheduler;
