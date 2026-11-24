use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::EvalError;

pub(super) const REPLAY_SCHEMA_V1: &str = "memory-decisions-replay/v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayIndex {
    pub(super) schema_version: String,
    pub(super) identity: ReplayIdentity,
    pub(super) cases: Vec<ReplayCase>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReplayIdentity {
    pub(super) suite_id: String,
    pub(super) product_revision: String,
    pub(super) product_binary_sha256: String,
    pub(super) corpus_sha256: String,
    pub(super) config_sha256: String,
    pub(super) input_schema_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReplayCase {
    pub(super) case_id: String,
    pub(super) source_order: Vec<String>,
    pub(super) roles: Vec<String>,
    pub(super) disposition: String,
    pub(super) evidence_success: Option<bool>,
    pub(super) decision_success: Option<bool>,
    pub(super) probe_success: Option<bool>,
    #[serde(default)]
    pub(super) score_observation_source: Option<String>,
    #[serde(default)]
    pub(super) score_observation: Option<Value>,
    #[serde(default)]
    pub(super) temporal_observation: Option<Value>,
    #[serde(default)]
    pub(super) temporal_score: Option<Value>,
    #[serde(default)]
    pub(super) evidence_enabled_temporal_observation: Option<Value>,
    #[serde(default)]
    pub(super) evidence_enabled_temporal_score: Option<Value>,
    pub(super) reason_codes: Vec<String>,
    pub(super) receipt_path: String,
    pub(super) receipt_sha256: String,
}

pub(super) fn write_receipt(
    root: &Path,
    case_id: &str,
    value: &Value,
) -> Result<(PathBuf, String), EvalError> {
    let path = root.join(format!("receipt-{case_id}.json"));
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|source| EvalError::Io {
            path: path.clone(),
            source,
        })?;
    file.write_all(&bytes).map_err(|source| EvalError::Io {
        path: path.clone(),
        source,
    })?;
    file.write_all(b"\n").map_err(|source| EvalError::Io {
        path: path.clone(),
        source,
    })?;
    file.sync_all().map_err(|source| EvalError::Io {
        path: path.clone(),
        source,
    })?;
    let digest = sha256(&bytes);
    Ok((path, digest))
}

pub(super) fn write_index(
    root: &Path,
    index: &ReplayIndex,
) -> Result<(PathBuf, String), EvalError> {
    let path = root.join("index.json");
    let bytes = serde_json::to_vec_pretty(index)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|source| EvalError::Io {
            path: path.clone(),
            source,
        })?;
    file.write_all(&bytes).map_err(|source| EvalError::Io {
        path: path.clone(),
        source,
    })?;
    file.write_all(b"\n").map_err(|source| EvalError::Io {
        path: path.clone(),
        source,
    })?;
    file.sync_all().map_err(|source| EvalError::Io {
        path: path.clone(),
        source,
    })?;
    let digest = sha256(&bytes);
    Ok((path, digest))
}

pub(super) fn hash_file(path: &Path) -> Result<String, EvalError> {
    let bytes = std::fs::read(path).map_err(|source| EvalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(sha256(&bytes))
}

pub(super) fn hash_corpus(root: &Path) -> Result<String, EvalError> {
    let mut hash = Sha256::new();
    for file in ["events.json", "probes.json", "oracle.json"] {
        let path = root.join(file);
        let bytes = std::fs::read(&path).map_err(|source| EvalError::Io {
            path: path.clone(),
            source,
        })?;
        hash.update(file.as_bytes());
        hash.update([0]);
        hash.update((bytes.len() as u64).to_be_bytes());
        hash.update(bytes);
    }
    Ok(hex::encode(hash.finalize()))
}

pub(super) fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
