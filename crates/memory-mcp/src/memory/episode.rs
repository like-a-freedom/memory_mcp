//! Episode operations - extraction and record parsing.

mod communities;
mod edges;
mod entity_extraction;
pub(crate) mod fact_extraction;
mod statement_detection;
mod summary_parser;
pub(crate) mod triples;

pub use crate::knowledge::fact_parsing::fact_from_record;
pub use crate::memory::record_parsing::episode_from_record;
pub(crate) use edges::store_edge;
pub(crate) use fact_extraction::{build_extract_log_result, extract_from_episode};
