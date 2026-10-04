//! Process-isolated integration contract for the process-global ID source.
use memory_mcp::tools::request_id::next_request_id;

#[test]
fn request_ids_increment_numerically_without_truncating_the_width_boundary() {
    let first = next_request_id();
    let second = next_request_id();
    // Preparation only: advance the actual source to the width boundary.
    (2..9_998).for_each(|_| drop(next_request_id()));
    let before_boundary = next_request_id();
    let after_boundary = next_request_id();
    assert_eq!(
        [first, second, before_boundary, after_boundary],
        ["req_0001", "req_0002", "req_9999", "req_10000"]
    );
}
