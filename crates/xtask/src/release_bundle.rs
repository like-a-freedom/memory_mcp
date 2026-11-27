//! Release-only qualification receipt for immutable packaged binaries.
use crate::image_handoff::file_digest;
use crate::pack::PackError;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

fn refused(error: impl std::fmt::Display) -> PackError {
    PackError::Bundle(format!("release qualification refused: {error}"))
}

fn manifest(directory: &Path, source: &str, run: &str, version: &str) -> Result<Value, PackError> {
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(directory).map_err(refused)? {
        let entry = entry.map_err(refused)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| refused("non-UTF8 filename"))?;
        if name == "release.json" {
            continue;
        }
        if !name.starts_with("memory_mcp")
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(refused("unexpected artifact filename"));
        }
        files.insert(name, file_digest(&entry.path())?);
    }
    for target in [
        "x86_64-unknown-linux-gnu.tar.gz",
        "aarch64-apple-darwin.tar.gz",
        "x86_64-pc-windows-msvc.zip",
        "aarch64-pc-windows-msvc.zip",
    ] {
        if !files.contains_key(&format!("memory_mcp-{target}")) {
            return Err(refused(format!("missing platform: {target}")));
        }
    }
    for (name, digest) in &files {
        if let Some(payload) = name.strip_suffix(".sha256") {
            if !files.contains_key(payload) {
                return Err(refused("orphan checksum"));
            }
        } else {
            let sidecar = format!("{name}.sha256");
            if !files.contains_key(&sidecar)
                || fs::read_to_string(directory.join(&sidecar))
                    .map_err(refused)?
                    .trim()
                    != digest
            {
                return Err(refused(format!("invalid checksum for {name}")));
            }
        }
    }
    Ok(
        json!({"schema_version":1,"source_sha":source,"run_id":run,"version":version,"profile":"release","features":["fs-watch","mcp-apps","streamable-http"],"files":files}),
    )
}

pub fn seal(directory: &Path, source: &str, run: &str, version: &str) -> Result<(), PackError> {
    let bytes = serde_json::to_vec(&manifest(directory, source, run, version)?).map_err(refused)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join("release.json"))
        .map_err(refused)?;
    file.write_all(&bytes).map_err(refused)
}

pub fn verify(directory: &Path, source: &str, run: &str, version: &str) -> Result<(), PackError> {
    let path = directory.join("release.json");
    file_digest(&path)?;
    let actual: Value =
        serde_json::from_slice(&fs::read(path).map_err(refused)?).map_err(refused)?;
    if actual != manifest(directory, source, run, version)? {
        return Err(refused(
            "artifact bytes, version, features or provenance changed",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod filesystem_integration_tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::fs;
    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const TARGETS: [&str; 4] = [
        "x86_64-unknown-linux-gnu.tar.gz",
        "aarch64-apple-darwin.tar.gz",
        "x86_64-pc-windows-msvc.zip",
        "aarch64-pc-windows-msvc.zip",
    ];
    fn files() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temp");
        for target in TARGETS {
            let name = format!("memory_mcp-{target}");
            fs::write(dir.path().join(&name), b"packaged bytes").expect("file");
            fs::write(
                dir.path().join(format!("{name}.sha256")),
                format!("{}\n", hex::encode(Sha256::digest(b"packaged bytes"))),
            )
            .expect("sidecar");
        }
        dir
    }
    fn sealed() -> tempfile::TempDir {
        let d = files();
        seal(d.path(), SHA, "42", "2.1.0").expect("seal");
        d
    }
    #[test]
    fn accepts_unchanged_complete_qualified_release() {
        let d = sealed();
        assert!(verify(d.path(), SHA, "42", "2.1.0").is_ok());
    }
    #[test]
    fn rejects_changed_binary() {
        let d = sealed();
        fs::write(
            d.path().join(format!("memory_mcp-{}", TARGETS[0])),
            b"changed",
        )
        .expect("mutate");
        assert!(verify(d.path(), SHA, "42", "2.1.0").is_err());
    }
    #[test]
    fn rejects_missing_platform() {
        let d = files();
        fs::remove_file(d.path().join(format!("memory_mcp-{}", TARGETS[0]))).expect("remove");
        assert!(seal(d.path(), SHA, "42", "2.1.0").is_err());
    }
    #[test]
    fn rejects_different_version() {
        let d = sealed();
        assert!(verify(d.path(), SHA, "42", "2.2.0").is_err());
    }
    #[test]
    fn rejects_different_source() {
        let d = sealed();
        assert!(
            verify(
                d.path(),
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "42",
                "2.1.0"
            )
            .is_err()
        );
    }
    #[test]
    fn rejects_added_unqualified_file() {
        let d = sealed();
        fs::write(d.path().join("extra"), b"unqualified").expect("extra");
        assert!(verify(d.path(), SHA, "42", "2.1.0").is_err());
    }
    #[test]
    fn rejects_wrong_sidecar() {
        let d = files();
        fs::write(
            d.path().join(format!("memory_mcp-{}.sha256", TARGETS[0])),
            "0".repeat(64),
        )
        .expect("mutate");
        assert!(seal(d.path(), SHA, "42", "2.1.0").is_err());
    }
}
