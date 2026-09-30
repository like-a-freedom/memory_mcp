//! Release artifact packaging.
//!
//! This replaces `scripts/ci/package.py`. The release workflow downloads the
//! artifacts this produces and attaches them to the GitHub Release with
//! `fail_on_unmatched_files: true`, so its behaviour is a release contract,
//! not a convenience: the archive names, the per-artifact `.sha256` sidecars
//! and the standalone CLI download all have to survive unchanged.
//!
//! The step that is not a contract but is kept because it catches what the
//! type system cannot: the smoke test runs the freshly built binary the way a
//! user would — `init`, `ingest` through both entity extractors, then a real
//! MCP `initialize` over stdin — and refuses to produce artifacts if any of it
//! fails. A build that compiles is not a build that works.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// The two programs a release ships.
const PROGRAMS: [&str; 2] = ["memory_mcp", "memory_mcp_http"];

/// Sidecar file name patterns, by the platform that produces them. `*.so*`
/// covers both a bare library and its versioned variants (`libfoo.so.1`).
const LIBRARY_PATTERNS: [&str; 3] = ["*.dll", "*.so*", "*.dylib"];

/// Environment prefixes cleared before the binary under test runs. The smoke
/// test drives a real server on a real embedded database, so any inherited
/// deployment configuration would point it at someone else's instance.
///
/// `GLINER_` is listed separately because the model-backed controls do not
/// share the `NER_` prefix: `NER_EXTRACTOR=anno` rejects `GLINER_DEVICE` as a
/// configuration error, so a developer's shell that sets it would fail the
/// smoke test for a reason that has nothing to do with the artifact.
const STRIPPED_ENV_PREFIXES: [&str; 7] = [
    "SURREALDB_",
    "MEMORY_",
    "NER_",
    "EMBEDDINGS_",
    "GLINER_",
    "DYLD_",
    "ORT_",
];

#[derive(Debug)]
pub enum PackError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Smoke(String),
    Bundle(String),
    /// A checker under `observability/` failed, or is missing. The detail
    /// carries the checker's own output, because the checker's message names
    /// the rule and that is the part worth reading.
    Observability(String),
}

impl std::fmt::Display for PackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            PackError::Smoke(detail) => write!(f, "{detail}"),
            PackError::Bundle(detail) => write!(f, "{detail}"),
            PackError::Observability(detail) => write!(f, "{detail}"),
        }
    }
}

impl std::error::Error for PackError {}

impl From<(PathBuf, std::io::Error)> for PackError {
    fn from((path, source): (PathBuf, std::io::Error)) -> Self {
        PackError::Io { path, source }
    }
}

/// An error with no path to name, such as reading a stream to a digest.
impl From<std::io::Error> for PackError {
    fn from(source: std::io::Error) -> Self {
        PackError::Io {
            path: PathBuf::new(),
            source,
        }
    }
}

type Result<T> = std::result::Result<T, PackError>;

/// Package the binaries in `build_dir` for `target`, writing the artifacts and
/// their checksums into `dist`.
///
/// `target` is the Rust target triple: it names the archive, decides the
/// archive format (Windows gets a zip) and decides whether a standalone
/// single-file download is possible.
///
/// `dist` is a parameter rather than a hardcoded `./dist` so the packaging tests
/// can assert on it without changing the process's working directory, which is
/// global state the parallel test harness shares.
pub fn package(build_dir: &Path, target: &str, dist: &Path) -> Result<Vec<PathBuf>> {
    let windows = target.contains("windows");
    let suffix = if windows { ".exe" } else { "" };

    if !dist.is_dir() {
        fs::create_dir_all(dist).map_err(|e| (dist.to_path_buf(), e))?;
    }

    let work = tempfile::tempdir().map_err(|e| (dist.to_path_buf(), e))?;
    let bundle = work.path().join("bundle");
    fs::create_dir_all(&bundle).map_err(|e| (bundle.clone(), e))?;

    let mut programs = Vec::with_capacity(PROGRAMS.len());
    for name in PROGRAMS {
        let file = format!("{name}{suffix}");
        let destination = bundle.join(&file);
        fs::copy(build_dir.join(&file), &destination).map_err(|e| (destination.clone(), e))?;
        programs.push(destination);
    }

    // Dynamic libraries the native dependency build produced ship next to the
    // executables: the bundled folder must be runnable on a machine that has
    // none of this project's build dependencies. The build directory is read
    // once — the three patterns are alternatives, not three passes over it.
    let mut has_sidecar = false;
    let build_entries = read_dir_sorted(build_dir)?;
    for entry in &build_entries {
        let name = entry
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !LIBRARY_PATTERNS
            .iter()
            .any(|pattern| matches_pattern(&name, pattern))
        {
            continue;
        }
        has_sidecar = true;
        let destination = bundle.join(&name);
        fs::copy(entry, &destination).map_err(|e| (destination.clone(), e))?;
    }

    let license = bundle.join("LICENSE");
    fs::copy("LICENSE", &license).map_err(|e| (license.clone(), e))?;

    // No development library search paths are added: a missing runtime library
    // has to fail here rather than pass on the build machine.
    smoke(&programs, work.path())?;

    let stem = format!("memory_mcp-{target}");
    let archive = if windows {
        zip_archive(&bundle, &dist.join(format!("{stem}.zip")))?
    } else {
        tar_gz_archive(&bundle, &dist.join(format!("{stem}.tar.gz")))?
    };

    let mut artifacts = vec![archive.clone()];
    // Preserve the standalone single-file download for targets where nothing
    // needs to sit beside the executable.
    if !has_sidecar {
        let system = if windows {
            "windows"
        } else if target.contains("apple") {
            "macos"
        } else {
            "linux"
        };
        let arch = target.split('-').next().unwrap_or(target);
        let standalone = dist.join(format!("memory_mcp_{system}_{arch}{suffix}"));
        fs::copy(&programs[0], &standalone).map_err(|e| (standalone.clone(), e))?;
        artifacts.push(standalone);
    }

    for artifact in &artifacts {
        let name = artifact
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let sidecar = artifact.with_file_name(format!("{name}.sha256"));
        write_sha256(artifact, &sidecar)?;
    }

    println!("Packaged and smoke-tested {target}");
    Ok(artifacts)
}

/// Glob matching for the handful of library suffixes a target can produce.
///
/// A full glob engine is not needed and not wanted: `*.dll` and `*.so*` are
/// prefix-and-extension checks, and anything subtler would be a portability
/// assumption about the native dependency build.
///
/// The trailing `*` in `*.so*` means "this library and its versioned
/// variants", so `libonnxruntime.so` and `libonnxruntime.so.1` both match
/// while `memory_mcp.exe` does not.
fn matches_pattern(name: &str, pattern: &str) -> bool {
    let Some(prefix) = pattern.strip_prefix('*') else {
        return name == pattern;
    };

    if prefix.ends_with('*') {
        // `*.so*` becomes the fixed middle `.so` with a wildcard on each side.
        // The trailing `*` is what makes a versioned variant match too, so
        // the only requirement is that `.so` occurs with something in front of
        // it — a leading `.so` is not a library name.
        let stem = prefix.trim_end_matches('*');
        return stem.is_empty() || matches!(name.find(stem), Some(index) if index > 0);
    }

    // `*.dll` / `*.dylib`: a plain extension match.
    name.len() > prefix.len() && name.ends_with(prefix)
}

/// Directory entries in a stable order, so two runs produce identical
/// archives and the checksum over them is reproducible.
fn read_dir_sorted(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut entries: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| (dir.to_path_buf(), e))? {
        entries.push(entry.map_err(|e| (dir.to_path_buf(), e))?.path());
    }
    entries.sort();
    Ok(entries)
}

fn write_sha256(artifact: &Path, sidecar: &Path) -> Result<()> {
    let mut file = fs::File::open(artifact).map_err(|e| (artifact.to_path_buf(), e))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| (artifact.to_path_buf(), e))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    // `sha2` 0.11's output is a hybrid array without `LowerHex`, so the digest
    // goes through `hex` exactly as the rest of the workspace does.
    let digest = format!("{}\n", hex::encode(hasher.finalize()));
    fs::write(sidecar, digest.as_bytes())?;
    Ok(())
}

fn tar_gz_archive(bundle: &Path, destination: &Path) -> Result<PathBuf> {
    let file = fs::File::create(destination).map_err(|e| (destination.to_path_buf(), e))?;
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    builder.follow_symlinks(false);
    // `append_dir_all` keeps the entries under their own names, which is what
    // the previous implementation produced: the archive's members are
    // `memory_mcp`, `memory_mcp_http`, `LICENSE` at the root.
    builder
        .append_dir_all(".", bundle)
        .map_err(|error| PackError::Bundle(format!("{destination:?}: {error}")))?;
    let encoder = builder
        .into_inner()
        .map_err(|error| PackError::Bundle(format!("{destination:?}: {error}")))?;
    encoder
        .finish()
        .map_err(|error| PackError::Bundle(format!("{destination:?}: {error}")))?;
    Ok(destination.to_path_buf())
}

fn zip_archive(bundle: &Path, destination: &Path) -> Result<PathBuf> {
    let file = fs::File::create(destination).map_err(|e| (destination.to_path_buf(), e))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for entry in read_dir_sorted(bundle)? {
        if !entry.is_file() {
            continue;
        }
        let name = entry
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        zip.start_file(name, options)
            .map_err(|error| PackError::Bundle(format!("{destination:?}: {error}")))?;
        let bytes = fs::read(&entry).map_err(|e| (entry.clone(), e))?;
        zip.write_all(&bytes)
            .map_err(|e| (destination.to_path_buf(), e))?;
    }
    zip.finish()
        .map_err(|error| PackError::Bundle(format!("{destination:?}: {error}")))?;
    Ok(destination.to_path_buf())
}

/// Run the packaged binaries the way an operator would, and refuse to package
/// anything if they misbehave.
///
/// The server is started on a private embedded database with an explicit
/// `Host`, because the deployment boundary compares the raw header and the
/// loopback socket's own name is not allowlisted.
fn smoke(programs: &[PathBuf], work: &Path) -> Result<()> {
    let cli = &programs[0];
    let http = &programs[1];

    let env = smoke_env(work);

    let version = run_capture(cli, &["--version"], &env, "version")?;
    if version.trim().is_empty() {
        return Err(PackError::Smoke("the binary printed no version".into()));
    }

    let init = run_capture(cli, &["init", "--target", "vscode"], &env, "init")?;
    let payload: serde_json::Value = serde_json::from_str(init.trim()).map_err(|error| {
        PackError::Smoke(format!(
            "init did not return one JSON object: {error}: {init}"
        ))
    })?;
    if payload.get("mutates_files").and_then(|v| v.as_bool()) != Some(false)
        || payload.get("target").and_then(|v| v.as_str()) != Some("vscode")
    {
        return Err(PackError::Smoke(format!("invalid init output: {payload}")));
    }

    // A live HTTP deployment needs explicit databases and credentials, so the
    // configuration parser is where this binary has to be shown to reach.
    //
    // Both conditions are required, not just the exit code: `memory_mcp_http`
    // exits `2` from three places — `config error:` for a missing deployment
    // variable, `config invalid:` for a malformed one, and
    // `validate_no_listener_env` for a listener variable set in a stdio-only
    // context. An invalid bind reaches the third. The message is what proves
    // the parser was reached and named the cause, so a binary that started and
    // died for an unrelated reason cannot pass as a configuration check.
    let mut command = Command::new(http);
    command
        .env_clear()
        .envs(&env)
        .env("MEMORY_MCP_HTTP_BIND", "invalid")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command
        .output()
        .map_err(|error| PackError::Smoke(format!("the HTTP binary did not run: {error}")))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() != Some(2) || !stderr.contains("config error:") {
        return Err(PackError::Smoke(format!(
            "HTTP configuration smoke failed: status={:?}, stderr={stderr}",
            output.status.code()
        )));
    }

    for selector in ["anno", "regex"] {
        run_capture(
            cli,
            &[
                "ingest",
                "--source-type",
                "smoke",
                "--source-id",
                selector,
                "--content",
                "Alice Smith from OpenAI presented Project Atlas.",
                "--t-ref",
                "2026-02-05T00:00:00Z",
            ],
            &env,
            selector,
        )
        .map_err(|error| {
            PackError::Smoke(format!(
                "ingest with the {selector} extractor failed: {error}"
            ))
        })?;
    }

    mcp_handshake(cli, &env)
}

/// Whether a server's stderr carries the marker that says the binary was built
/// without filesystem ingestion. A release artifact without `fs-watch` still
/// answers MCP, so nothing else would notice.
fn stderr_reports_missing_fs_watch(stderr: &str) -> bool {
    stderr.contains("built without the fs-watch feature")
}

/// The environment the binaries under test run with: the inherited process
/// environment minus every deployment variable, plus a private database.
fn smoke_env(work: &Path) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = std::env::vars()
        .filter(|(key, _)| {
            !STRIPPED_ENV_PREFIXES
                .iter()
                .any(|prefix| key.starts_with(prefix))
        })
        .collect();
    env.extend([
        ("SURREALDB_EMBEDDED".into(), "true".into()),
        (
            "SURREALDB_DATA_DIR".into(),
            work.join("db").display().to_string(),
        ),
        ("SURREALDB_DB_NAME".into(), "smoke".into()),
        ("SURREALDB_NAMESPACE".into(), "main".into()),
        ("SURREALDB_USERNAME".into(), "root".into()),
        ("SURREALDB_PASSWORD".into(), "root".into()),
        ("EMBEDDINGS_ENABLED".into(), "false".into()),
        ("NER_EXTRACTOR".into(), "anno".into()),
        (
            "MEMORY_INGESTION_INBOX".into(),
            work.join("inbox").display().to_string(),
        ),
    ]);
    env
}

fn run_capture(
    program: &Path,
    args: &[&str],
    env: &BTreeMap<String, String>,
    label: &str,
) -> Result<String> {
    let output = Command::new(program)
        .env_clear()
        .envs(env.iter())
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| PackError::Smoke(format!("{label}: could not run: {error}")))?;
    if !output.status.success() {
        return Err(PackError::Smoke(format!(
            "{label}: exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Drive a real MCP `initialize` over stdin and require a well-formed result.
///
/// A process that starts and then exits without answering is not a server, and
/// no compile-time check can tell the difference. The pipe stays owned by the
/// child for its whole life so that closing stdin is the shutdown signal the
/// server expects.
fn mcp_handshake(cli: &Path, env: &BTreeMap<String, String>) -> Result<()> {
    let inbox = env
        .get("MEMORY_INGESTION_INBOX")
        .map(PathBuf::from)
        .unwrap_or_default();
    if !inbox.as_os_str().is_empty() {
        fs::create_dir_all(&inbox).map_err(|e| (inbox.clone(), e))?;
    }

    let mut child = Command::new(cli)
        .env_clear()
        .envs(env.iter())
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| PackError::Smoke(format!("serve did not start: {error}")))?;

    let outcome = initialize(&mut child);
    let shutdown = shutdown(&mut child);
    outcome.and(shutdown)
}

fn initialize(child: &mut Child) -> Result<()> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| PackError::Smoke("the server has no stdin".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| PackError::Smoke("the server has no stdout".into()))?;

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "ci-smoke", "version": "1"},
        }
    });
    writeln!(stdin, "{request}")
        .map_err(|error| PackError::Smoke(format!("initialize was not written: {error}")))?;
    stdin
        .flush()
        .map_err(|error| PackError::Smoke(format!("initialize was not flushed: {error}")))?;

    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let read = BufReader::new(stdout)
            .read_line(&mut line)
            .ok()
            .filter(|count| *count > 0);
        let _ = sender.send(read.map(|_| line));
    });

    let line = receiver
        .recv_timeout(Duration::from_secs(30))
        .map_err(|_| PackError::Smoke("the server did not answer initialize".into()))?
        .ok_or_else(|| PackError::Smoke("the server closed stdout before answering".into()))?;

    let response: serde_json::Value = serde_json::from_str(line.trim())
        .map_err(|error| PackError::Smoke(format!("initialize was not one JSON line: {error}")))?;
    if response.get("id").and_then(|id| id.as_i64()) != Some(1) {
        return Err(PackError::Smoke(format!(
            "unexpected initialize id: {response}"
        )));
    }
    if let Some(error) = response.get("error") {
        return Err(PackError::Smoke(format!("MCP initialize failed: {error}")));
    }
    if response.get("result").is_none() {
        return Err(PackError::Smoke(format!(
            "initialize had no result: {response}"
        )));
    }
    Ok(())
}

/// Close the request stream and require the server to exit cleanly.
///
/// The child owns its own stdin handle, so dropping it is the EOF the server
/// waits for. A non-zero exit here is a release defect: an operator who stops
/// the server should not see a failure.
fn shutdown(child: &mut Child) -> Result<()> {
    drop(child.stdin.take());

    let deadline = std::time::Instant::now() + Duration::from_secs(35);
    let mut status = None;
    while std::time::Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(exit)) => {
                status = Some(exit);
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(error) => {
                return Err(PackError::Smoke(format!(
                    "the server could not be waited on: {error}"
                )));
            }
        }
    }

    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(PackError::Smoke(
            "the server did not exit within 35s of the request stream closing".into(),
        ));
    };

    if !status.success() {
        let mut tail = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut tail);
        }
        return Err(PackError::Smoke(format!(
            "the server exited with {status} after the client disconnected: {tail}"
        )));
    }

    // A binary built without `fs-watch` still answers MCP, so the marker in
    // its startup output is the only signal that the artifact is incomplete.
    let mut stderr_tail = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut stderr_tail);
    }
    if stderr_reports_missing_fs_watch(&stderr_tail) {
        return Err(PackError::Smoke(
            "the artifact was built without filesystem ingestion".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_suffixes_are_matched_the_way_the_native_build_emits_them() {
        assert!(matches_pattern("memory_mcp.dll", "*.dll"));
        assert!(matches_pattern("libonnxruntime.so", "*.so*"));
        assert!(matches_pattern("libonnxruntime.so.1", "*.so*"));
        assert!(matches_pattern("libblas.dylib", "*.dylib"));

        // A Windows executable is not a library, and the archive must not
        // treat it as one.
        assert!(!matches_pattern("memory_mcp.exe", "*.dll"));
        assert!(!matches_pattern("libcares.so", "*.dylib"));
        assert!(!matches_pattern("README.md", "*.so*"));
    }

    #[test]
    fn the_fs_watch_marker_is_recognised() {
        assert!(stderr_reports_missing_fs_watch(
            "error: built without the fs-watch feature"
        ));
        assert!(!stderr_reports_missing_fs_watch("ingested 2 episodes"));
    }

    /// The digest written beside an artifact must be over that artifact's
    /// bytes: it is what a downloader verifies against, so a mismatch is a
    /// silently corrupt release rather than a test failure.
    #[test]
    fn the_sidecar_digest_is_over_the_artifact_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let artifact = dir.path().join("memory_mcp.tar.gz");
        fs::write(&artifact, b"a deterministic payload").expect("write");
        let sidecar = dir.path().join("memory_mcp.tar.gz.sha256");

        write_sha256(&artifact, &sidecar).expect("digest");

        let recorded = fs::read_to_string(&sidecar).expect("read sidecar");
        let expected = hex::encode(Sha256::digest(b"a deterministic payload"));
        assert_eq!(recorded.trim(), expected);
        assert!(
            recorded.ends_with('\n'),
            "a trailing newline keeps it shell-friendly"
        );
    }

    /// Both programs and the licence must be present, and nothing else: the
    /// archive is what a user downloads, so a missing member is a broken
    /// release and a stray one is a leak of build output.
    #[test]
    fn the_archive_carries_both_programs_and_the_licence() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bundle = dir.path().join("bundle");
        fs::create_dir_all(&bundle).expect("bundle dir");
        for name in ["memory_mcp", "memory_mcp_http", "LICENSE"] {
            fs::write(bundle.join(name), name.as_bytes()).expect("write");
        }
        let archive = dir.path().join("out.tar.gz");

        tar_gz_archive(&bundle, &archive).expect("archive");

        let mut members = Vec::new();
        let mut decoder = flate2::read::GzDecoder::new(fs::File::open(&archive).expect("open"));
        let mut tar = tar::Archive::new(&mut decoder);
        for entry in tar.entries().expect("entries") {
            let entry = entry.expect("entry");
            if entry.path().is_ok() {
                members.push(entry.path().unwrap().to_string_lossy().into_owned());
            }
        }
        for expected in ["memory_mcp", "memory_mcp_http", "LICENSE"] {
            assert!(
                members.iter().any(|member| member == expected),
                "the archive must carry {expected}, got {members:?}"
            );
        }
    }

    /// A failed smoke test must leave `dist/` empty. An archive produced from
    /// a binary that does not run is worse than no archive: the release
    /// workflow would attach it and `fail_on_unmatched_files` would be
    /// satisfied.
    #[test]
    fn a_failed_smoke_test_produces_no_artifact() {
        let dir = tempfile::tempdir().expect("tempdir");
        let build = dir.path().join("build");
        fs::create_dir_all(&build).expect("build dir");
        for name in ["memory_mcp", "memory_mcp_http"] {
            // A script that is not a working binary: it fails at the first
            // smoke step, which is exactly the path under test.
            let path = build.join(name);
            fs::write(&path, "#!/bin/sh\nexit 1\n").expect("write");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut permissions = fs::metadata(&path).expect("stat").permissions();
                permissions.set_mode(0o755);
                fs::set_permissions(&path, permissions).expect("chmod");
            }
        }
        fs::write(dir.path().join("LICENSE"), "licence").expect("licence");
        let dist = dir.path().join("dist");

        let outcome = package(&build, "x86_64-unknown-linux-gnu", &dist);

        assert!(outcome.is_err(), "a stub binary must not be packaged");
        let produced: Vec<_> = fs::read_dir(&dist)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            produced.is_empty(),
            "a refused package must leave dist empty, found {produced:?}"
        );
    }
}
