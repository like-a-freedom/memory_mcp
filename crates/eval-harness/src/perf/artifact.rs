//! `memory-mcp-perf/v1` artifact: serialization, digest, strict read.

use crate::perf::PerfError;
use crate::perf::domain::{ARTIFACT_SCHEMA, HostFingerprint, ScenarioRecord};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::BTreeMap;
use std::path::Path;

/// A committed perf run: reduced stats, SQL catalog, and resources.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PerfArtifact {
    pub schema: String,
    pub profile: String,
    /// sha256 hex of the profile file the run executed.
    pub profile_digest: String,
    pub commit: String,
    pub host: HostFingerprint,
    pub started_at: String,
    pub duration_ms: u64,
    pub scenarios: BTreeMap<String, ScenarioRecord>,
}

/// Write the artifact as stable camelCase JSON (2-space indent).
pub fn write(artifact: &PerfArtifact, path: &Path) -> Result<(), PerfError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            PerfError::Validation(format!("cannot create {}: {e}", parent.display()))
        })?;
    }
    let json = serde_json::to_string_pretty(artifact)?;
    std::fs::write(path, format!("{json}\n"))
        .map_err(|e| PerfError::Validation(format!("cannot write {}: {e}", path.display())))?;
    Ok(())
}

/// Read an artifact; any schema other than `ARTIFACT_SCHEMA` is rejected.
pub fn read(path: &Path) -> Result<PerfArtifact, PerfError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| PerfError::Validation(format!("cannot read {}: {e}", path.display())))?;
    let artifact: PerfArtifact = serde_json::from_str(&raw)?;
    if artifact.schema != ARTIFACT_SCHEMA {
        return Err(PerfError::Schema(artifact.schema));
    }
    Ok(artifact)
}

/// sha256 of `bytes` as lowercase hex — the profile-digest convention.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perf::domain::{HostFingerprint, ResourceSnapshot};
    use crate::perf::reducer::LatencyStats;

    fn sample_artifact() -> PerfArtifact {
        let mut scenarios = std::collections::BTreeMap::new();
        scenarios.insert(
            "zeta-latency".to_owned(),
            ScenarioRecord {
                latencies: Some(LatencyStats {
                    count: 2,
                    p50_us: 100,
                    p90_us: 150,
                    p95_us: 160,
                    p99_us: 170,
                    mean_us: 125,
                    total_us: 250,
                }),
                throughput_eps: Some(8000.0),
                samples_raw: vec![100, 150],
                sql: vec![],
                resources: Some(ResourceSnapshot {
                    rss_bytes: Some(1 << 20),
                    peak_rss_bytes: Some(2 << 20),
                    disk_read_bytes: None,
                    disk_write_bytes: None,
                    io_read_ops: None,
                    io_write_ops: None,
                }),
                invalid_reasons: vec![],
            },
        );
        scenarios.insert(
            "alpha-scale".to_owned(),
            ScenarioRecord {
                latencies: None,
                throughput_eps: None,
                samples_raw: vec![],
                sql: vec![],
                resources: None,
                invalid_reasons: vec!["sampler unavailable".to_owned()],
            },
        );
        PerfArtifact {
            schema: ARTIFACT_SCHEMA.to_owned(),
            profile: "pr-smoke".to_owned(),
            profile_digest: "00".to_owned(),
            commit: "abc1234".to_owned(),
            host: HostFingerprint {
                host_tag: "dev-m2".to_owned(),
                os: "macos".to_owned(),
                arch: "aarch64".to_owned(),
            },
            started_at: "2026-10-10T00:00:00Z".to_owned(),
            duration_ms: 1234,
            scenarios,
        }
    }

    #[test]
    fn round_trip_preserves_every_field() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("artifact.json");
        let original = sample_artifact();
        write(&original, &path).expect("write");
        let back = read(&path).expect("read");
        assert_eq!(back, original);
    }

    #[test]
    fn writes_are_byte_stable_and_keys_sorted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path_a = dir.path().join("a.json");
        let path_b = dir.path().join("b.json");
        let artifact = sample_artifact();
        write(&artifact, &path_a).expect("write a");
        write(&artifact, &path_b).expect("write b");
        let bytes_a = std::fs::read(&path_a).expect("read a");
        let bytes_b = std::fs::read(&path_b).expect("read b");
        assert_eq!(bytes_a, bytes_b);
        let text = String::from_utf8(bytes_a).expect("utf8");
        let alpha = text.find("\"alpha-scale\"").expect("alpha key");
        let zeta = text.find("\"zeta-latency\"").expect("zeta key");
        assert!(alpha < zeta, "BTreeMap keys must serialize in sorted order");
    }

    #[test]
    fn unknown_artifact_schema_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("artifact.json");
        let mut artifact = sample_artifact();
        artifact.schema = "memory-mcp-perf/v2".to_owned();
        write(&artifact, &path).expect("write");
        let err = read(&path).expect_err("v2 must not load");
        assert!(matches!(err, PerfError::Schema(_)), "got {err:?}");
    }

    #[test]
    fn digest_is_sha256_hex_of_the_bytes() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
