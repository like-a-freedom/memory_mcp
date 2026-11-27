//! Provenance checks for explicitly selected release candidate runs.
use crate::pack::PackError;
use serde_json::Value;

/// Qualify run identity and completion before downloading its release artifacts.
/// The caller must also verify the release-only qualification artifact.
pub fn verify(
    run: &Value,
    source_sha: &str,
    repository: &str,
    workflow_id: u64,
    run_id: u64,
) -> Result<(), PackError> {
    let reject =
        |field: &str| PackError::Bundle(format!("unqualified release candidate: {field} mismatch"));
    if run_id == 0 || run.get("id").and_then(Value::as_u64) != Some(run_id) {
        return Err(reject("run ID"));
    }
    if workflow_id == 0 || run.get("workflow_id").and_then(Value::as_u64) != Some(workflow_id) {
        return Err(reject("workflow ID"));
    }
    for (field, expected) in [
        ("head_sha", source_sha),
        ("path", ".github/workflows/ci.yml"),
        ("event", "workflow_dispatch"),
        ("head_branch", "master"),
        ("status", "completed"),
        ("conclusion", "success"),
    ] {
        if run.get(field).and_then(Value::as_str) != Some(expected) {
            return Err(reject(field));
        }
    }
    if run
        .pointer("/head_repository/full_name")
        .and_then(Value::as_str)
        != Some(repository)
    {
        return Err(reject("repository"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::verify;
    use serde_json::{Value, json};

    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn candidate() -> Value {
        json!({
            "id": 123, "head_sha": SHA,
            "head_repository": {"full_name": "owner/repo"},
            "workflow_id": 456, "path": ".github/workflows/ci.yml",
            "event": "workflow_dispatch", "head_branch": "master",
            "status": "completed", "conclusion": "success"
        })
    }

    fn check(run: &Value) -> Result<(), crate::pack::PackError> {
        verify(run, SHA, "owner/repo", 456, 123)
    }

    #[test]
    fn accepts_completed_trusted_manual_master_candidate() {
        assert!(check(&candidate()).is_ok());
    }

    #[test]
    fn rejects_another_run() {
        let mut run = candidate();
        run["id"] = json!(124);
        assert!(check(&run).is_err());
    }

    #[test]
    fn rejects_another_source_commit() {
        let mut run = candidate();
        run["head_sha"] = json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        assert!(check(&run).is_err());
    }

    #[test]
    fn rejects_fork_repository() {
        let mut run = candidate();
        run["head_repository"]["full_name"] = json!("attacker/repo");
        assert!(check(&run).is_err());
    }

    #[test]
    fn rejects_another_workflow() {
        let mut run = candidate();
        run["workflow_id"] = json!(457);
        assert!(check(&run).is_err());
    }

    #[test]
    fn rejects_wrong_workflow_path() {
        let mut run = candidate();
        run["path"] = json!(".github/workflows/other.yml");
        assert!(check(&run).is_err());
    }

    #[test]
    fn rejects_pull_request_event() {
        let mut run = candidate();
        run["event"] = json!("pull_request");
        assert!(check(&run).is_err());
    }

    #[test]
    fn rejects_feature_branch() {
        let mut run = candidate();
        run["head_branch"] = json!("feature");
        assert!(check(&run).is_err());
    }

    #[test]
    fn rejects_incomplete_run() {
        let mut run = candidate();
        run["status"] = json!("in_progress");
        assert!(check(&run).is_err());
    }

    #[test]
    fn rejects_failed_run() {
        let mut run = candidate();
        run["conclusion"] = json!("failure");
        assert!(check(&run).is_err());
    }

    #[test]
    fn rejects_missing_repository() {
        let mut run = candidate();
        run["head_repository"] = Value::Null;
        assert!(check(&run).is_err());
    }

    #[test]
    fn rejects_non_object_response() {
        assert!(check(&json!([])).is_err());
    }

    #[test]
    fn rejects_string_run_identity() {
        let mut run = candidate();
        run["id"] = json!("123; echo injected");
        assert!(check(&run).is_err());
    }

    #[test]
    fn rejects_zero_requested_run_id() {
        let mut run = candidate();
        run["id"] = json!(0);
        assert!(verify(&run, SHA, "owner/repo", 456, 0).is_err());
    }

    #[test]
    fn rejects_zero_requested_workflow_id() {
        let mut run = candidate();
        run["workflow_id"] = json!(0);
        assert!(verify(&run, SHA, "owner/repo", 0, 123).is_err());
    }
}
