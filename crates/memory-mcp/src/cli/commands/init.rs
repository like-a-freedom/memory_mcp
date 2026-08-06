//! Output-only host setup command.

use crate::cli::args::InitArgs;
use crate::service::MemoryError;

/// Runs the host setup command.
pub fn run(_args: InitArgs) -> Result<(), MemoryError> {
    Err(MemoryError::Validation(
        "init renderer is implemented in the next task".to_string(),
    ))
}
