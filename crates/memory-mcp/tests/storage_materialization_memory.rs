#[path = "common/storage_memory.rs"]
#[allow(dead_code)]
mod storage_memory;

use std::error::Error;

use storage_memory::{StorageMemoryFixture, run_remote_allocation_case};

#[tokio::test(flavor = "current_thread")]
#[ignore = "remote SurrealDB allocation regression gate; run explicitly on Linux amd64"]
async fn remote_300_rows_does_not_allocate_a_second_tagged_tree() -> Result<(), Box<dyn Error>> {
    let fixture = StorageMemoryFixture::start(300, 2048).await?;
    let report = run_remote_allocation_case(&fixture, 300, 2048)?;
    println!("STORAGE_ALLOCATION_REPORT={report:?}");

    assert_eq!(report.rows, 300, "{report:?}");
    assert_eq!(report.dimension, 2048, "{report:?}");
    assert!(
        report.peak_delta_bytes < 1_073_741_824,
        "legacy tagged-tree materialization exceeded the 1 GiB budget: {report:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "invoked only as a child process by the allocation regression gate"]
async fn allocation_child_worker() -> Result<(), Box<dyn Error>> {
    if std::env::var_os(storage_memory::CHILD_MODE_ENV).is_none() {
        return Ok(());
    }

    let report = storage_memory::measure_remote_query_from_environment().await?;
    println!(
        "{}{}",
        storage_memory::REPORT_PREFIX,
        serde_json::to_string(&report)?
    );

    Ok(())
}
