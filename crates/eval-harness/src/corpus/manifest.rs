use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::EvalError;

pub const CORPUS_MANIFEST_SCHEMA: &str = "memory-mcp-corpus/v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorpusManifest {
    pub schema_version: String,
    pub corpus_id: String,
    pub source_url: String,
    pub revision: String,
    pub sha256: String,
    pub license: String,
    pub byte_size: u64,
    pub case_count: usize,
    pub adapter_version: String,
    pub data_file: PathBuf,
    #[serde(default)]
    pub auxiliary_files: Vec<CorpusFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorpusFile {
    pub source_url: String,
    pub revision: String,
    pub sha256: String,
    pub byte_size: u64,
    pub data_file: PathBuf,
}

#[derive(Debug)]
pub struct PreparedCorpus {
    pub manifest: CorpusManifest,
    pub data_path: PathBuf,
}

impl CorpusManifest {
    pub fn parse(raw: &str) -> Result<Self, EvalError> {
        let manifest: CorpusManifest =
            serde_json::from_str(raw).map_err(|e| EvalError::InvalidConfig(e.to_string()))?;
        manifest.validate_fields()?;
        Ok(manifest)
    }

    fn validate_fields(&self) -> Result<(), EvalError> {
        if self.schema_version != CORPUS_MANIFEST_SCHEMA {
            return Err(EvalError::InvalidConfig(format!(
                "unsupported corpus schema version: {}",
                self.schema_version
            )));
        }
        if self.corpus_id.trim().is_empty() {
            return Err(EvalError::InvalidConfig(
                "corpus_id must not be empty".into(),
            ));
        }
        validate_identifier(&self.corpus_id, "corpus_id")?;
        if self.revision.trim().is_empty() {
            return Err(EvalError::InvalidConfig(
                "revision must not be empty".into(),
            ));
        }
        let symbolic = ["main", "master", "latest", "HEAD", "head"];
        if symbolic.contains(&self.revision.as_str()) {
            return Err(EvalError::InvalidConfig(format!(
                "revision must be an immutable commit/hash, not symbolic '{}'",
                self.revision
            )));
        }
        validate_identifier(&self.revision, "revision")?;
        if self.license.trim().is_empty() {
            return Err(EvalError::InvalidConfig("license must not be empty".into()));
        }
        if self.byte_size == 0 {
            return Err(EvalError::InvalidConfig(
                "byte_size must be positive".into(),
            ));
        }
        if self.case_count == 0 {
            return Err(EvalError::InvalidConfig(
                "case_count must be positive".into(),
            ));
        }
        if self.sha256.len() != 64 || !self.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(EvalError::InvalidConfig(format!(
                "sha256 must be 64 lowercase hex characters, got {}",
                self.sha256
            )));
        }
        if self.data_file.components().any(|c| c.as_os_str() == "..") {
            return Err(EvalError::InvalidConfig(
                "data_file must not contain parent traversal".into(),
            ));
        }
        validate_corpus_file_path(&self.data_file, "data_file")?;
        for auxiliary in &self.auxiliary_files {
            validate_corpus_file(auxiliary)?;
            if auxiliary.data_file == self.data_file {
                return Err(EvalError::InvalidConfig(
                    "auxiliary data_file must differ from data_file".into(),
                ));
            }
        }
        for (index, left) in self.auxiliary_files.iter().enumerate() {
            if self
                .auxiliary_files
                .iter()
                .skip(index + 1)
                .any(|right| right.data_file == left.data_file)
            {
                return Err(EvalError::InvalidConfig(
                    "auxiliary data_file paths must be unique".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn resolve_source_url(&self) -> Result<String, EvalError> {
        validate_source_url(&self.source_url)?;
        Ok(self.source_url.clone())
    }

    pub fn validate_case_count(&self, actual: usize) -> Result<(), EvalError> {
        if actual != self.case_count {
            return Err(EvalError::InvalidInput(format!(
                "case count mismatch for {}: expected {}, got {}",
                self.corpus_id, self.case_count, actual
            )));
        }
        Ok(())
    }

    pub fn validate_at(&self, root: &Path) -> Result<PreparedCorpus, EvalError> {
        let data_path = root.join(&self.data_file);
        if !data_path.exists() {
            return Err(EvalError::InvalidInput(format!(
                "data file not found: {}",
                data_path.display()
            )));
        }

        let file = std::fs::File::open(&data_path).map_err(|source| EvalError::Io {
            path: data_path.clone(),
            source,
        })?;
        let mut reader = std::io::BufReader::new(file);
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 8192];
        let mut total_bytes: u64 = 0;

        loop {
            let bytes_read = reader.read(&mut buffer).map_err(|source| EvalError::Io {
                path: data_path.clone(),
                source,
            })?;
            if bytes_read == 0 {
                break;
            }
            hasher.update(&buffer[..bytes_read]);
            total_bytes += bytes_read as u64;
        }

        let computed_hash = hex::encode(hasher.finalize());

        if computed_hash != self.sha256 {
            return Err(EvalError::InvalidInput(format!(
                "sha-256 mismatch for {}: expected {}, got {}",
                self.data_file.display(),
                self.sha256,
                computed_hash
            )));
        }

        if total_bytes != self.byte_size {
            return Err(EvalError::InvalidInput(format!(
                "byte size mismatch for {}: expected {}, got {}",
                self.data_file.display(),
                self.byte_size,
                total_bytes
            )));
        }

        for auxiliary in &self.auxiliary_files {
            let path = root.join(&auxiliary.data_file);
            validate_file_digest(&path, auxiliary.byte_size, &auxiliary.sha256)?;
        }

        let prepared = PreparedCorpus {
            manifest: self.clone(),
            data_path,
        };
        if let Some(kind) = crate::corpus::adapters::DatasetKind::parse_name(&self.corpus_id) {
            let adapter = crate::corpus::adapters::adapter_for(kind);
            if self.adapter_version != adapter.adapter_version() {
                return Err(EvalError::InvalidConfig(format!(
                    "unsupported adapter version for {}: {}",
                    self.corpus_id, self.adapter_version
                )));
            }
            let cases = crate::corpus::adapters::load_and_normalize(kind, &prepared)?;
            self.validate_case_count(cases.len())?;
        }
        Ok(prepared)
    }
}

fn validate_source_url(source_url: &str) -> Result<(), EvalError> {
    if source_url.contains("/main/") || source_url.contains("/master/") {
        return Err(EvalError::InvalidConfig(format!(
            "source_url contains mutable branch path: {source_url}"
        )));
    }
    Ok(())
}

fn validate_identifier(value: &str, field: &str) -> Result<(), EvalError> {
    let path = Path::new(value);
    let mut components = path.components();
    if path.is_absolute()
        || !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(EvalError::InvalidConfig(format!(
            "{field} must be a single path-safe component"
        )));
    }
    Ok(())
}

fn validate_corpus_file_path(path: &Path, field: &str) -> Result<(), EvalError> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| component.as_os_str() == "..")
        || path == Path::new(".")
        || path == Path::new("manifest.json")
    {
        return Err(EvalError::InvalidConfig(format!(
            "{field} must be a relative data path"
        )));
    }
    Ok(())
}

fn validate_corpus_file(file: &CorpusFile) -> Result<(), EvalError> {
    if file.revision.trim().is_empty()
        || ["main", "master", "latest", "HEAD", "head"].contains(&file.revision.as_str())
    {
        return Err(EvalError::InvalidConfig(
            "auxiliary revision must be immutable".into(),
        ));
    }
    validate_identifier(&file.revision, "auxiliary revision")?;
    if file.byte_size == 0 {
        return Err(EvalError::InvalidConfig(
            "auxiliary byte_size must be positive".into(),
        ));
    }
    if file.sha256.len() != 64 || !file.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(EvalError::InvalidConfig(
            "auxiliary sha256 must be 64 hex characters".into(),
        ));
    }
    validate_source_url(&file.source_url)?;
    validate_corpus_file_path(&file.data_file, "auxiliary data_file")
}

fn validate_file_digest(
    path: &Path,
    expected_size: u64,
    expected_hash: &str,
) -> Result<(), EvalError> {
    let file = std::fs::File::open(path).map_err(|source| EvalError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut reader = std::io::BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    let mut total_bytes = 0u64;
    loop {
        let bytes_read = reader.read(&mut buffer).map_err(|source| EvalError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
        total_bytes += bytes_read as u64;
    }
    let computed_hash = hex::encode(hasher.finalize());
    if total_bytes != expected_size || computed_hash != expected_hash {
        return Err(EvalError::InvalidInput(format!(
            "auxiliary file integrity mismatch for {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_manifest_json() -> String {
        serde_json::json!({
            "schema_version": "memory-mcp-corpus/v1",
            "corpus_id": "test-corpus",
            "source_url": "https://example.com/data.json",
            "revision": "abc123",
            "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "license": "MIT",
            "byte_size": 11,
            "case_count": 1,
            "adapter_version": "1",
            "data_file": "data.json"
        })
        .to_string()
    }

    #[test]
    fn valid_manifest_parses() {
        let manifest = CorpusManifest::parse(&valid_manifest_json()).unwrap();
        assert_eq!(manifest.corpus_id, "test-corpus");
        assert_eq!(manifest.case_count, 1);
    }

    #[test]
    fn digest_mismatch_invalidates_the_corpus() {
        let dir = tempfile::tempdir().unwrap();
        let data_path = dir.path().join("data.json");
        std::fs::write(&data_path, b"actual content").unwrap();

        let manifest_json = serde_json::json!({
            "schema_version": "memory-mcp-corpus/v1",
            "corpus_id": "test",
            "source_url": "https://example.com",
            "revision": "rev1",
            "sha256": "0".repeat(64),
            "license": "MIT",
            "byte_size": 14,
            "case_count": 1,
            "adapter_version": "1",
            "data_file": "data.json"
        })
        .to_string();

        let manifest = CorpusManifest::parse(&manifest_json).unwrap();
        let result = manifest.validate_at(dir.path());
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("sha-256 mismatch"),
            "expected sha-256 mismatch error"
        );
    }

    #[test]
    fn unknown_manifest_fields_are_rejected() {
        let raw = valid_manifest_json().replace("\"license\"", "\"unexpected\":1,\"license\"");
        assert!(CorpusManifest::parse(&raw).is_err());
    }

    #[test]
    fn empty_revision_is_rejected() {
        let raw = valid_manifest_json().replace("\"abc123\"", "\"\"");
        assert!(CorpusManifest::parse(&raw).is_err());
    }

    #[test]
    fn symbolic_revision_is_rejected() {
        for sym in ["main", "master", "latest", "HEAD"] {
            let raw = valid_manifest_json().replace("\"abc123\"", &format!("\"{sym}\""));
            assert!(
                CorpusManifest::parse(&raw).is_err(),
                "symbolic revision '{sym}' should be rejected"
            );
        }
    }

    #[test]
    fn mutable_url_is_rejected() {
        let manifest = CorpusManifest {
            schema_version: CORPUS_MANIFEST_SCHEMA.to_string(),
            corpus_id: "test".into(),
            source_url: "https://raw.githubusercontent.com/org/repo/main/data.json".into(),
            revision: "abc123".into(),
            sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
            license: "MIT".into(),
            byte_size: 11,
            case_count: 1,
            adapter_version: "1".into(),
            data_file: "data.json".into(),
            auxiliary_files: vec![],
        };
        assert!(manifest.resolve_source_url().is_err());
    }

    #[test]
    fn case_count_mismatch_is_rejected() {
        let manifest = CorpusManifest::parse(&valid_manifest_json()).unwrap();
        let error = manifest.validate_case_count(2).unwrap_err();
        assert!(error.to_string().contains("case count mismatch"));
    }

    #[test]
    fn empty_license_is_rejected() {
        let raw = valid_manifest_json().replace("\"MIT\"", "\"\"");
        assert!(CorpusManifest::parse(&raw).is_err());
    }

    #[test]
    fn zero_byte_size_is_rejected() {
        let raw = valid_manifest_json().replace("\"byte_size\":11", "\"byte_size\":0");
        assert!(CorpusManifest::parse(&raw).is_err());
    }

    #[test]
    fn zero_case_count_is_rejected() {
        let raw = valid_manifest_json().replace("\"case_count\":1", "\"case_count\":0");
        assert!(CorpusManifest::parse(&raw).is_err());
    }

    #[test]
    fn parent_traversal_in_data_file_is_rejected() {
        let raw = valid_manifest_json().replace(
            "\"data_file\":\"data.json\"",
            "\"data_file\":\"../data.json\"",
        );
        assert!(CorpusManifest::parse(&raw).is_err());
    }

    #[test]
    fn manifest_paths_cannot_escape_or_overwrite_metadata() {
        for (field, value) in [
            ("corpus_id", "../escape"),
            ("revision", "/tmp/escape"),
            ("data_file", "/tmp/data.json"),
            ("data_file", "manifest.json"),
            ("data_file", "."),
        ] {
            let mut raw: serde_json::Value = serde_json::from_str(&valid_manifest_json()).unwrap();
            raw[field] = value.into();
            assert!(
                CorpusManifest::parse(&raw.to_string()).is_err(),
                "accepted {field}={value}"
            );
        }
    }

    #[test]
    fn wrong_schema_version_is_rejected() {
        let raw = valid_manifest_json().replace("\"memory-mcp-corpus/v1\"", "\"wrong-version\"");
        assert!(CorpusManifest::parse(&raw).is_err());
    }

    #[test]
    fn non_hex_sha256_is_rejected() {
        let valid_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let bad_hash = format!("g{}", &valid_hash[1..]);
        let raw = valid_manifest_json().replace(valid_hash, &bad_hash);
        assert!(CorpusManifest::parse(&raw).is_err());
    }

    #[test]
    fn too_short_sha256_is_rejected() {
        let valid_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let short_hash = &valid_hash[..32];
        let raw = valid_manifest_json().replace(valid_hash, short_hash);
        assert!(CorpusManifest::parse(&raw).is_err());
    }

    #[test]
    fn valid_manifest_with_correct_digest_passes() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let data_path = dir.path().join("data.json");
        let content = b"test content here";
        let mut file = std::fs::File::create(&data_path).unwrap();
        file.write_all(content).unwrap();
        file.sync_all().unwrap();

        let mut hasher = Sha256::new();
        hasher.update(content);
        let hash = hex::encode(hasher.finalize());

        let manifest_json = serde_json::json!({
            "schema_version": "memory-mcp-corpus/v1",
            "corpus_id": "test",
            "source_url": "https://example.com",
            "revision": "rev1",
            "sha256": hash,
            "license": "MIT",
            "byte_size": content.len(),
            "case_count": 1,
            "adapter_version": "1",
            "data_file": "data.json"
        })
        .to_string();

        let manifest = CorpusManifest::parse(&manifest_json).unwrap();
        let prepared = manifest.validate_at(dir.path()).unwrap();
        assert!(prepared.data_path.exists());
    }
}
