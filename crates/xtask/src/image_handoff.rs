//! Same-run image artifact identity; filesystem integration checks below.
use crate::pack::PackError;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

fn refused(message: impl std::fmt::Display) -> PackError {
    PackError::Bundle(format!("image handoff refused: {message}"))
}
fn lowercase_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn identity(source: &str, run: &str) -> Result<(), PackError> {
    if !lowercase_hex(source, 40)
        || run.is_empty()
        || run.starts_with('0')
        || !run.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(refused("invalid source commit or workflow run identity"));
    }
    Ok(())
}
fn image_identity(image: &str) -> bool {
    image
        .strip_prefix("sha256:")
        .is_some_and(|digest| lowercase_hex(digest, 64))
}
fn regular(path: &Path) -> Result<(), PackError> {
    let meta = fs::symlink_metadata(path).map_err(refused)?;
    if !meta.file_type().is_file() || meta.len() == 0 {
        return Err(refused(format!(
            "{} must be a nonempty regular file",
            path.display()
        )));
    }
    Ok(())
}
/// Digest an actual artifact without loading a potentially multi-GB archive into RAM.
pub(crate) fn file_digest(path: &Path) -> Result<String, PackError> {
    regular(path)?;
    let mut file = fs::File::open(path).map_err(refused)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(refused)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(hex::encode(digest.finalize()))
}

pub fn seal(
    directory: &Path,
    image_id: &str,
    source_sha: &str,
    run_id: &str,
) -> Result<(), PackError> {
    identity(source_sha, run_id)?;
    if !image_identity(image_id) {
        return Err(refused("invalid image content ID"));
    }
    let archive_sha256 = file_digest(&directory.join("image.tar"))?;
    let manifest = json!({"schema_version":1,"source_sha":source_sha,"run_id":run_id,"image_id":image_id,"archive_sha256":archive_sha256});
    let bytes = serde_json::to_vec(&manifest).map_err(refused)?;
    // Refuse an existing path, including a symlink, rather than following it.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join("image.json"))
        .map_err(refused)?;
    file.write_all(&bytes).map_err(refused)?;
    file.write_all(b"\n").map_err(refused)?;
    Ok(())
}
pub fn verify(
    directory: &Path,
    source_sha: &str,
    run_id: &str,
    loaded_image_id: Option<&str>,
) -> Result<String, PackError> {
    identity(source_sha, run_id)?;
    let path = directory.join("image.json");
    regular(&path)?;
    let manifest: Value =
        serde_json::from_slice(&fs::read(path).map_err(refused)?).map_err(refused)?;
    let keys = [
        "schema_version",
        "source_sha",
        "run_id",
        "image_id",
        "archive_sha256",
    ];
    let object = manifest
        .as_object()
        .ok_or_else(|| refused("invalid manifest object"))?;
    if object.len() != keys.len()
        || !keys.iter().all(|key| object.contains_key(*key))
        || manifest["schema_version"].as_u64() != Some(1)
    {
        return Err(refused("unsupported image manifest schema"));
    }
    if manifest["source_sha"].as_str() != Some(source_sha)
        || manifest["run_id"].as_str() != Some(run_id)
    {
        return Err(refused(
            "image belongs to a different commit or workflow run",
        ));
    }
    let image = manifest["image_id"]
        .as_str()
        .filter(|image| image_identity(image))
        .ok_or_else(|| refused("invalid image content ID"))?;
    if loaded_image_id.is_some_and(|loaded| loaded != image) {
        return Err(refused("loaded image differs from the smoke-tested image"));
    }
    if manifest["archive_sha256"].as_str()
        != Some(file_digest(&directory.join("image.tar"))?.as_str())
    {
        return Err(refused("image archive checksum mismatch"));
    }
    Ok(image.to_owned())
}

#[cfg(test)]
mod filesystem_integration_tests {
    use super::*;
    use std::fs;
    const SOURCE: &str = "1111111111111111111111111111111111111111";
    const IMAGE: &str = "sha256:2222222222222222222222222222222222222222222222222222222222222222";
    fn archive() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("scratch directory");
        fs::write(dir.path().join("image.tar"), b"smoke-tested archive").expect("archive");
        dir
    }
    fn sealed() -> tempfile::TempDir {
        let dir = archive();
        seal(dir.path(), IMAGE, SOURCE, "42").expect("seal");
        dir
    }
    #[test]
    fn verifies_same_run_archive_and_loaded_image() {
        let dir = sealed();
        assert_eq!(
            verify(dir.path(), SOURCE, "42", Some(IMAGE)).expect("verify"),
            IMAGE
        );
    }
    #[test]
    fn rejects_corrupted_archive() {
        let dir = sealed();
        fs::write(dir.path().join("image.tar"), b"different bytes").expect("corrupt");
        assert!(verify(dir.path(), SOURCE, "42", None).is_err());
    }
    #[test]
    fn rejects_different_source() {
        let dir = sealed();
        assert!(
            verify(
                dir.path(),
                "3333333333333333333333333333333333333333",
                "42",
                None
            )
            .is_err()
        );
    }
    #[test]
    fn rejects_different_run() {
        let dir = sealed();
        assert!(verify(dir.path(), SOURCE, "43", None).is_err());
    }
    #[test]
    fn rejects_different_loaded_image() {
        let dir = sealed();
        assert!(
            verify(
                dir.path(),
                SOURCE,
                "42",
                Some("sha256:4444444444444444444444444444444444444444444444444444444444444444")
            )
            .is_err()
        );
    }
    #[test]
    fn rejects_unknown_manifest_fields() {
        let dir = sealed();
        let path = dir.path().join("image.json");
        let mut data: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
        data["unexpected"] = serde_json::json!(true);
        fs::write(path, serde_json::to_vec(&data).expect("encode")).expect("write");
        assert!(verify(dir.path(), SOURCE, "42", None).is_err());
    }
    #[test]
    fn rejects_empty_archive() {
        let dir = archive();
        fs::write(dir.path().join("image.tar"), b"").expect("empty");
        assert!(seal(dir.path(), IMAGE, SOURCE, "42").is_err());
    }
    #[test]
    fn rejects_invalid_source_identity() {
        let dir = archive();
        assert!(seal(dir.path(), IMAGE, "invalid", "42").is_err());
    }
    #[test]
    fn rejects_invalid_image_id() {
        let dir = archive();
        assert!(seal(dir.path(), "latest", SOURCE, "42").is_err());
    }
    #[cfg(unix)]
    #[test]
    fn rejects_symlink_archive() {
        let dir = archive();
        fs::rename(dir.path().join("image.tar"), dir.path().join("actual.tar")).expect("rename");
        std::os::unix::fs::symlink("actual.tar", dir.path().join("image.tar")).expect("symlink");
        assert!(seal(dir.path(), IMAGE, SOURCE, "42").is_err());
    }
}
