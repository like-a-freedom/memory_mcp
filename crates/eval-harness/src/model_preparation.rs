//! Verified, atomic provisioning of immutable native model bundles.
use crate::error::EvalError;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelManifest {
    model: String,
    revision: String,
    license: String,
    files: Vec<ModelFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelFile {
    path: String,
    bytes: u64,
    sha256: String,
    #[serde(default)]
    source: Option<ModelSource>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelSource {
    model: String,
    revision: String,
    path: String,
    license: String,
}

struct StagingDirectory(PathBuf);
impl Drop for StagingDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn invalid(message: impl Into<String>) -> EvalError {
    EvalError::InvalidInput(message.into())
}

fn io(path: &Path, source: std::io::Error) -> EvalError {
    EvalError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn validate_source(model: &str, revision: &str, license: &str) -> Result<(), EvalError> {
    let components: Vec<_> = model.split('/').collect();
    let safe = |name: &str| {
        !name.is_empty()
            && name != "."
            && name != ".."
            && name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b'.')
    };
    if components.len() != 2 || !components.iter().all(|name| safe(name)) {
        return Err(invalid("model must name a safe owner/repository"));
    }
    if revision.len() != 40 || !revision.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(invalid(
            "model revision must be an immutable 40-digit commit",
        ));
    }
    if !matches!(license, "MIT" | "Apache-2.0") {
        return Err(invalid("model license must be MIT or Apache-2.0"));
    }
    Ok(())
}

fn validate(manifest: &ModelManifest) -> Result<(), EvalError> {
    validate_source(&manifest.model, &manifest.revision, &manifest.license)?;
    let mut names = BTreeSet::new();
    for file in &manifest.files {
        if let Some(source) = &file.source {
            validate_source(&source.model, &source.revision, &source.license)?;
            if source.path.is_empty()
                || source.path == "."
                || source.path == ".."
                || !source
                    .path
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
            {
                return Err(invalid("model source must have a safe leaf path"));
            }
        }
        if !matches!(
            file.path.as_str(),
            "config.json" | "tokenizer.json" | "model.safetensors" | "model.gguf" | "README.md"
        ) || !names.insert(file.path.as_str())
        {
            return Err(invalid("model files must have unique supported leaf names"));
        }
        if file.bytes == 0
            || file.bytes > 2 * 1024 * 1024 * 1024
            || file.sha256.len() != 64
            || !file.sha256.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err(invalid(
                "model files require bounded sizes and sha256 checksums",
            ));
        }
    }
    if !["config.json", "tokenizer.json"]
        .iter()
        .all(|name| names.contains(name))
        || names.contains("model.safetensors") == names.contains("model.gguf")
    {
        return Err(invalid(
            "model bundle must include config, tokenizer and exactly one weights format",
        ));
    }
    Ok(())
}

fn permitted_url(url: &reqwest::Url) -> bool {
    let loopback = url.host_str().is_some_and(|host| {
        host.parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    });
    url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && (url.scheme() == "https" || (url.scheme() == "http" && loopback))
}

fn verify_file(path: &Path, expected: &ModelFile) -> Result<(), EvalError> {
    let mut file = File::open(path).map_err(|e| io(path, e))?;
    if file.metadata().map_err(|e| io(path, e))?.len() != expected.bytes {
        return Err(invalid(format!(
            "model file {} has the wrong size",
            expected.path
        )));
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer).map_err(|e| io(path, e))?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    if hex::encode(hash.finalize()) != expected.sha256.to_ascii_lowercase() {
        return Err(invalid(format!(
            "model file {} checksum mismatch",
            expected.path
        )));
    }
    Ok(())
}

fn verify_bundle(path: &Path, manifest: &ModelManifest) -> Result<(), EvalError> {
    for file in &manifest.files {
        verify_file(&path.join(&file.path), file)?;
    }
    Ok(())
}

/// Resolve a lock manifest to a local cache only after every file is verified.
/// Downloads stream into an owned staging directory; a failure never publishes it.
pub async fn prepare_model(
    manifest_path: &Path,
    root: &Path,
    mirror: Option<&str>,
) -> Result<PathBuf, EvalError> {
    let bytes = std::fs::read(manifest_path).map_err(|e| io(manifest_path, e))?;
    let manifest: ModelManifest = serde_json::from_slice(&bytes)?;
    validate(&manifest)?;
    let base = reqwest::Url::parse(mirror.unwrap_or("https://huggingface.co"))
        .map_err(|_| invalid("invalid model mirror URL"))?;
    if !permitted_url(&base) {
        return Err(invalid(
            "model mirror requires HTTPS or loopback HTTP without credentials",
        ));
    }
    let parent = root.join(manifest.model.replace('/', "--"));
    let destination = parent.join(&manifest.revision);
    if destination.exists() {
        verify_bundle(&destination, &manifest)?;
        return Ok(destination);
    }
    std::fs::create_dir_all(&parent).map_err(|e| io(&parent, e))?;
    static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);
    let stage = loop {
        let path = parent.join(format!(
            ".prepare-{}-{}",
            std::process::id(),
            NEXT_STAGE.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::create_dir(&path) {
            Ok(()) => break StagingDirectory(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io(&path, e)),
        }
    };
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(600))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            // Hugging Face uses signed HTTPS CDN URLs. Do not constrain their query parameters.
            let url = attempt.url();
            let transport_ok = url.scheme() == "https"
                || (url.scheme() == "http"
                    && url.host_str().is_some_and(|h| {
                        h.parse::<std::net::IpAddr>()
                            .is_ok_and(|ip| ip.is_loopback())
                    }));
            if attempt.previous().len() >= 10
                || !transport_ok
                || !url.username().is_empty()
                || url.password().is_some()
            {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|e| invalid(format!("model download client: {e}")))?;
    for file in &manifest.files {
        let (model, revision, remote_path) = file.source.as_ref().map_or(
            (
                manifest.model.as_str(),
                manifest.revision.as_str(),
                file.path.as_str(),
            ),
            |source| {
                (
                    source.model.as_str(),
                    source.revision.as_str(),
                    source.path.as_str(),
                )
            },
        );
        let url = format!(
            "{}/{model}/resolve/{revision}/{remote_path}",
            base.as_str().trim_end_matches('/')
        );
        let mut response = client
            .get(url)
            .send()
            .await
            .map_err(|e| invalid(format!("model download {}: {e}", file.path)))?
            .error_for_status()
            .map_err(|e| invalid(format!("model download {}: {e}", file.path)))?;
        let path = stage.0.join(&file.path);
        let mut output = File::create(&path).map_err(|e| io(&path, e))?;
        let mut count = 0u64;
        let mut hash = Sha256::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| invalid(format!("model download {}: {e}", file.path)))?
        {
            count = count
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| invalid("model download size overflow"))?;
            if count > file.bytes {
                return Err(invalid(format!(
                    "model file {} exceeds its locked size",
                    file.path
                )));
            }
            hash.update(&chunk);
            output.write_all(&chunk).map_err(|e| io(&path, e))?;
        }
        if count != file.bytes {
            return Err(invalid(format!(
                "model file {} has the wrong size",
                file.path
            )));
        }
        if hex::encode(hash.finalize()) != file.sha256.to_ascii_lowercase() {
            return Err(invalid(format!(
                "model file {} checksum mismatch",
                file.path
            )));
        }
        output.sync_all().map_err(|e| io(&path, e))?;
    }
    match std::fs::rename(&stage.0, &destination) {
        Ok(()) => Ok(destination),
        Err(_) if destination.exists() => {
            verify_bundle(&destination, &manifest)?;
            Ok(destination)
        }
        Err(e) => Err(io(&destination, e)),
    }
}
