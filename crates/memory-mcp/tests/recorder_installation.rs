//! The shipping recorder path: `install_recorder` is what a deployment calls.
//!
//! It cannot be covered by a unit test. Installing the Prometheus recorder is a
//! process-global, one-shot operation — `metrics::set_global_recorder` succeeds
//! once per process — so in the shared test binary the recorder is already
//! installed by the time any test could call it, and the call would return the
//! "already installed" error rather than doing the work. Asserting that error
//! would test the recorder crate, not this function.
//!
//! What is left is a process of its own, which is what this file is: its own
//! test binary, with one test, where the recorder is installed before anything
//! else runs. That is also the honest shape of the claim — a deployment
//! installs the recorder through this function, so this is the only place the
//! real installation path is exercised end to end, including the part the
//! in-crate handle cannot cover: `install_recorder` describes the metrics and
//! stamps the build itself, so the exposition it produces is the one a real
//! deployment serves.

#![cfg(all(feature = "prometheus", feature = "streamable-http"))]

/// The whole deployment path, in one test because the order is the point.
///
/// The recorder is process-global and installs once, so the three things worth
/// asserting about `install_recorder` are one sequence rather than three
/// independent facts: it installs, what it installs is described and stamped,
/// and a second attempt is refused rather than panicking. Split across tests they
/// would race — whichever ran first would take the install, and the other would
/// assert against a process state it did not create.
#[test]
fn a_deployment_installs_a_stamped_recorder_and_refuses_a_second_one() {
    use memory_mcp::observability::{METRIC_BUILD_INFO, shared_test_handle};

    // ── Act: the call a deployment makes.
    let handle = memory_mcp::http::metrics::install_recorder()
        .expect("the first install of a process-global recorder succeeds");

    // Stamped here rather than by the test handle: the build behind a dashboard
    // is what an incident review needs, and it has to be the deployment's build.
    // It is also the only description this path can be shown to register on its
    // own — an exporter emits `# HELP` for a family that has a series, so every
    // other family's description is observable only once something records into
    // it, which the in-crate test drives on purpose.
    let exposition = handle.render();
    assert!(
        exposition.contains(&format!("# HELP {METRIC_BUILD_INFO} ")),
        "the exposition a deployment serves must describe the build it came \
         from, or an incident review cannot name the release behind a graph: \
         {exposition}"
    );
    assert!(
        exposition.contains(&format!("{METRIC_BUILD_INFO}{{version=")),
        "and must carry the build as a label: {exposition}"
    );

    // From the other side of the same fact: the crate's own test handle cannot
    // be opened now, because the recorder it would install is already taken.
    // That is the contract that makes this file necessary — one recorder per
    // process, installed once — stated as an observation rather than a comment.
    assert!(
        shared_test_handle().is_none(),
        "a recorder is already installed, so the test handle cannot open a second \
         one; if this ever returns a handle, the recorder stopped being \
         process-global and every metric in this crate is going somewhere \
         unexpected"
    );

    // ── Act: the same call a second time, which is the documented failure.
    let error = memory_mcp::http::metrics::install_recorder()
        .expect_err("a recorder cannot be installed twice in one process");

    assert!(
        error.to_string().contains("recorder"),
        "a refused second install must say what it was, not just fail: {error}"
    );
}
