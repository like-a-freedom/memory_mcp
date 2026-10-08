//! Runtime pool and tenant-bound lifecycle management.
pub mod bootstrap;
pub mod guard;
pub mod lifecycle;
#[cfg(feature = "test-fixtures")]
pub mod memory_snapshot;
#[cfg(not(feature = "test-fixtures"))]
pub(crate) mod memory_snapshot;
pub mod pool;
pub mod signal;
pub mod storage;
