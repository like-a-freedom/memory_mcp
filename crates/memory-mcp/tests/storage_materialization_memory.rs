#[path = "common/storage_memory.rs"]
#[allow(dead_code)]
mod storage_memory;

use std::error::Error;

use storage_memory::{
    StorageMemoryFixture, run_remote_allocation_case, run_remote_bound_scan_case,
    run_remote_no_embedding_allocation_case,
};

macro_rules! dimension_gate_case {
    ($name:ident, $rows:literal, $dimension:literal, $budget:literal) => {
        #[tokio::test(flavor = "current_thread")]
        #[ignore = "remote SurrealDB allocation regression gate; run explicitly on Linux amd64"]
        async fn $name() -> Result<(), Box<dyn Error>> {
            let paired_small = if $rows == 1000 {
                let fixture = StorageMemoryFixture::start(300, $dimension).await?;
                Some(run_remote_allocation_case(&fixture, 300, $dimension)?)
            } else {
                None
            };
            let fixture = StorageMemoryFixture::start($rows, $dimension).await?;
            let report = run_remote_allocation_case(&fixture, $rows, $dimension)?;
            println!("STORAGE_ALLOCATION_REPORT={report:?}");

            assert_eq!(report.rows, $rows, "{report:?}");
            assert_eq!(report.dimension, $dimension, "{report:?}");
            assert!(
                report.peak_delta_bytes < $budget,
                "dimension {} with {} rows exceeded its G2 budget {}: {report:?}",
                $dimension,
                $rows,
                $budget
            );
            if let Some(small) = paired_small {
                assert_eq!(small.rows, 300, "{small:?}");
                assert_eq!(small.dimension, $dimension, "{small:?}");
                let small_budget = if $dimension == 3072 {
                    100_663_296
                } else {
                    67_108_864
                };
                assert!(
                    small.peak_delta_bytes < small_budget,
                    "300-row D={} baseline exceeded its G2 budget {}: {small:?}",
                    $dimension,
                    small_budget
                );
                assert!(
                    report.peak_delta_bytes <= 4 * small.peak_delta_bytes,
                    "peak allocation scaled superlinearly at D={}: small={small:?}, large={report:?}",
                    $dimension
                );
            }

            Ok(())
        }
    };
}

macro_rules! no_embedding_gate_case {
    ($name:ident, $rows:literal, $budget:literal) => {
        #[tokio::test(flavor = "current_thread")]
        #[ignore = "remote SurrealDB allocation regression gate; run explicitly on Linux amd64"]
        async fn $name() -> Result<(), Box<dyn Error>> {
            let paired_small = if $rows == 1000 {
                Some(run_remote_no_embedding_allocation_case(300).await?)
            } else {
                None
            };
            let report = run_remote_no_embedding_allocation_case($rows).await?;
            println!("STORAGE_ALLOCATION_REPORT={report:?}");

            assert_eq!(report.rows, $rows, "{report:?}");
            assert_eq!(report.dimension, 0, "zero denotes absent vectors in this test report");
            assert!(
                report.peak_delta_bytes < $budget,
                "no-vector mode with {} rows exceeded its G2 budget {}: {report:?}",
                $rows,
                $budget
            );
            if let Some(small) = paired_small {
                assert_eq!(small.rows, 300, "{small:?}");
                assert_eq!(small.dimension, 0, "{small:?}");
                assert!(
                    small.peak_delta_bytes < 16_777_216,
                    "300-row no-vector baseline exceeded its G2 budget: {small:?}"
                );
                assert!(
                    report.peak_delta_bytes <= 4 * small.peak_delta_bytes,
                    "no-vector peak allocation scaled superlinearly: small={small:?}, large={report:?}"
                );
            }

            Ok(())
        }
    };
}

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
#[ignore = "remote SurrealDB allocation regression gate; run explicitly on Linux amd64"]
async fn remote_300_rows_materializes_within_64_mib() -> Result<(), Box<dyn Error>> {
    let fixture = StorageMemoryFixture::start(300, 2048).await?;
    let report = run_remote_allocation_case(&fixture, 300, 2048)?;
    println!("STORAGE_ALLOCATION_REPORT={report:?}");

    assert_eq!(report.rows, 300, "{report:?}");
    assert_eq!(report.dimension, 2048, "{report:?}");
    assert!(
        report.peak_delta_bytes < 67_108_864,
        "typed-value materialization exceeded the 64 MiB budget: {report:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "remote SurrealDB allocation regression gate; run explicitly on Linux amd64"]
async fn owner_scoped_active_fact_read_does_not_copy_plain_rows() -> Result<(), Box<dyn Error>> {
    let report = run_remote_bound_scan_case(300, 65_536).await?;
    println!("STORAGE_ALLOCATION_REPORT={report:?}");

    assert_eq!(report.rows, 300, "{report:?}");
    assert_eq!(
        report.dimension, 0,
        "zero marks an absent vector in this test report"
    );
    assert!(
        report.peak_delta_bytes < 8_388_608,
        "BoundDbClient copied the ready fact rows during owner-store handoff: {report:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "remote SurrealDB allocation regression gate; run explicitly on Linux amd64"]
async fn remote_1000_rows_materializes_within_192_mib() -> Result<(), Box<dyn Error>> {
    let small_fixture = StorageMemoryFixture::start(300, 2048).await?;
    let small = run_remote_allocation_case(&small_fixture, 300, 2048)?;
    let large_fixture = StorageMemoryFixture::start(1000, 2048).await?;
    let large = run_remote_allocation_case(&large_fixture, 1000, 2048)?;
    println!("STORAGE_ALLOCATION_SCALE_REPORT_SMALL={small:?}");
    println!("STORAGE_ALLOCATION_SCALE_REPORT_LARGE={large:?}");

    assert_eq!(small.rows, 300, "{small:?}");
    assert_eq!(large.rows, 1000, "{large:?}");
    assert_eq!(small.dimension, 2048, "{small:?}");
    assert_eq!(large.dimension, 2048, "{large:?}");
    assert!(
        large.peak_delta_bytes < 201_326_592,
        "1000-row materialization exceeded the 192 MiB budget: {large:?}"
    );
    assert!(
        large.peak_delta_bytes <= 4 * small.peak_delta_bytes,
        "peak allocation scaled superlinearly from 300 to 1000 rows: small={small:?}, large={large:?}"
    );

    Ok(())
}

dimension_gate_case!(
    remote_300_rows_dimension_17_stays_within_g2,
    300,
    17,
    67_108_864
);
dimension_gate_case!(
    remote_1000_rows_dimension_17_stays_within_g2,
    1000,
    17,
    201_326_592
);
dimension_gate_case!(
    remote_300_rows_dimension_384_stays_within_g2,
    300,
    384,
    67_108_864
);
dimension_gate_case!(
    remote_1000_rows_dimension_384_stays_within_g2,
    1000,
    384,
    201_326_592
);
dimension_gate_case!(
    remote_300_rows_dimension_768_stays_within_g2,
    300,
    768,
    67_108_864
);
dimension_gate_case!(
    remote_1000_rows_dimension_768_stays_within_g2,
    1000,
    768,
    201_326_592
);
dimension_gate_case!(
    remote_300_rows_dimension_1536_stays_within_g2,
    300,
    1536,
    67_108_864
);
dimension_gate_case!(
    remote_1000_rows_dimension_1536_stays_within_g2,
    1000,
    1536,
    201_326_592
);
dimension_gate_case!(
    remote_300_rows_dimension_2048_stays_within_g2,
    300,
    2048,
    67_108_864
);
dimension_gate_case!(
    remote_1000_rows_dimension_2048_stays_within_g2,
    1000,
    2048,
    201_326_592
);
dimension_gate_case!(
    remote_300_rows_dimension_3072_stays_within_g2,
    300,
    3072,
    100_663_296
);
dimension_gate_case!(
    remote_1000_rows_dimension_3072_stays_within_g2,
    1000,
    3072,
    301_989_888
);
no_embedding_gate_case!(
    remote_300_rows_without_embedding_stays_within_g2,
    300,
    16_777_216
);
no_embedding_gate_case!(
    remote_1000_rows_without_embedding_stays_within_g2,
    1000,
    50_331_648
);

#[tokio::test(flavor = "current_thread")]
#[ignore = "invoked only as a child process by the allocation regression gate"]
async fn allocation_child_worker() -> Result<(), Box<dyn Error>> {
    if std::env::var_os(storage_memory::CHILD_MODE_ENV).is_none() {
        return Ok(());
    }

    let report = match std::env::var("MEMORY_MCP_STORAGE_ALLOCATION_MODE").as_deref() {
        Ok("bound_active_fact_query") => {
            storage_memory::measure_remote_bound_scan_from_environment().await?
        }
        _ => storage_memory::measure_remote_query_from_environment().await?,
    };
    println!(
        "{}{}",
        storage_memory::REPORT_PREFIX,
        serde_json::to_string(&report)?
    );

    Ok(())
}
