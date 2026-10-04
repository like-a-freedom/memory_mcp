//! Integration tests for corpus publication, validation, rollback and caching.

use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use eval_harness::corpus::{CORPUS_MANIFEST_SCHEMA, CorpusFetcher, CorpusFile, CorpusManifest};
use eval_harness::{EvalError, corpus::prepare_corpus};
use sha2::Digest;

const CONTENT: &[u8] = b"hello world";
const CONTENT_SHA256: &str = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";

struct BytesFetcher {
    bytes: Vec<u8>,
    calls: AtomicUsize,
}

impl BytesFetcher {
    fn new(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.to_vec(),
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl CorpusFetcher for BytesFetcher {
    async fn fetch(&self, _url: &str, _revision: &str) -> Result<Vec<u8>, EvalError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.bytes.clone())
    }
}

struct NoFetch {
    calls: AtomicUsize,
}

impl NoFetch {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl CorpusFetcher for NoFetch {
    async fn fetch(&self, _url: &str, _revision: &str) -> Result<Vec<u8>, EvalError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(EvalError::Suite("unexpected corpus fetch".into()))
    }
}

struct MainAndCorruptAuxiliaryFetcher;

#[async_trait]
impl CorpusFetcher for MainAndCorruptAuxiliaryFetcher {
    async fn fetch(&self, _url: &str, revision: &str) -> Result<Vec<u8>, EvalError> {
        Ok(match revision {
            "rev1" => CONTENT.to_vec(),
            "aux-rev" => b"auxiliary Bites".to_vec(),
            _ => return Err(EvalError::Suite("unexpected revision".into())),
        })
    }
}

struct FailedFetcher {
    calls: AtomicUsize,
}

impl FailedFetcher {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl CorpusFetcher for FailedFetcher {
    async fn fetch(&self, _url: &str, _revision: &str) -> Result<Vec<u8>, EvalError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(EvalError::Suite("fetch failed".into()))
    }
}

fn manifest(content: &[u8]) -> CorpusManifest {
    let hash = if content == CONTENT {
        CONTENT_SHA256.to_owned()
    } else {
        hex::encode(sha2::Sha256::digest(content))
    };
    CorpusManifest::parse(
        &serde_json::json!({
            "schema_version": CORPUS_MANIFEST_SCHEMA,
            "corpus_id": "test-corpus",
            "source_url": "https://example.com/data.json",
            "revision": "rev1",
            "sha256": hash,
            "license": "MIT",
            "byte_size": content.len(),
            "case_count": 1,
            "adapter_version": "1",
            "data_file": "data.json"
        })
        .to_string(),
    )
    .expect("valid manifest")
}

#[tokio::test]
async fn preparation_publishes_verified_bytes_and_manifest_then_uses_the_cache_without_fetching() {
    let dir = tempfile::tempdir().expect("temp dir");
    let manifest = manifest(CONTENT);
    let fetcher = BytesFetcher::new(CONTENT);

    let prepared = prepare_corpus(&manifest, dir.path(), &fetcher)
        .await
        .expect("prepare corpus");

    assert_eq!(fetcher.calls(), 1);
    assert_eq!(
        prepared.data_path,
        dir.path().join("test-corpus/rev1/data.json")
    );
    assert_eq!(
        std::fs::read(&prepared.data_path).expect("published data"),
        CONTENT
    );
    let published_manifest_path = prepared
        .data_path
        .parent()
        .expect("corpus revision directory")
        .join("manifest.json");
    let published_manifest_raw =
        std::fs::read_to_string(&published_manifest_path).expect("published manifest");
    let published_manifest_json: serde_json::Value =
        serde_json::from_str(&published_manifest_raw).expect("parse manifest JSON");
    let expected_manifest_json = serde_json::to_value(&manifest).expect("serialize input manifest");
    assert_eq!(published_manifest_json, expected_manifest_json);
    let published_manifest =
        CorpusManifest::parse(&published_manifest_raw).expect("parse published manifest");
    assert_eq!(published_manifest.corpus_id, "test-corpus");
    assert_eq!(published_manifest.revision, "rev1");
    assert_eq!(published_manifest.sha256, CONTENT_SHA256);
    assert_eq!(published_manifest.byte_size, CONTENT.len() as u64);
    assert_eq!(published_manifest.case_count, 1);
    let validated = manifest
        .validate_at(prepared.data_path.parent().expect("revision directory"))
        .expect("read and validate published corpus");
    assert_eq!(validated.data_path, prepared.data_path);

    let no_fetch = NoFetch::new();
    let cached = prepare_corpus(&manifest, dir.path(), &no_fetch)
        .await
        .expect("validated cache hit");

    assert_eq!(no_fetch.calls(), 0);
    assert_eq!(cached.data_path, prepared.data_path);
    assert_eq!(
        std::fs::read(cached.data_path).expect("cached data"),
        CONTENT
    );
}

#[tokio::test]
async fn a_wrong_download_size_is_rejected_before_publication() {
    let dir = tempfile::tempdir().expect("temp dir");
    let manifest = manifest(CONTENT);
    let fetcher = BytesFetcher::new(b"short");

    let error = prepare_corpus(&manifest, dir.path(), &fetcher)
        .await
        .expect_err("wrong-size data must be refused");

    assert!(error.to_string().contains("fetched 5 bytes"));
    assert_eq!(fetcher.calls(), 1);
    assert!(!dir.path().join("test-corpus/rev1").exists());
}

#[tokio::test]
async fn a_same_size_download_with_the_wrong_hash_is_rejected_before_publication() {
    let dir = tempfile::tempdir().expect("temp dir");
    let manifest = manifest(CONTENT);
    let fetcher = BytesFetcher::new(b"jello world");

    let error = prepare_corpus(&manifest, dir.path(), &fetcher)
        .await
        .expect_err("wrong-hash data must be refused");

    assert!(error.to_string().contains("sha-256 mismatch"));
    assert_eq!(fetcher.calls(), 1);
    assert!(!dir.path().join("test-corpus/rev1").exists());
}

#[tokio::test]
async fn a_corrupted_cached_corpus_is_rejected_without_refetching() {
    let dir = tempfile::tempdir().expect("temp dir");
    let manifest = manifest(CONTENT);
    let fetcher = BytesFetcher::new(CONTENT);
    let prepared = prepare_corpus(&manifest, dir.path(), &fetcher)
        .await
        .expect("initial preparation");
    std::fs::write(&prepared.data_path, b"jello world").expect("corrupt cached bytes");
    let no_fetch = NoFetch::new();

    let error = prepare_corpus(&manifest, dir.path(), &no_fetch)
        .await
        .expect_err("corrupt cache must be refused");

    assert!(error.to_string().contains("sha-256 mismatch"));
    assert_eq!(no_fetch.calls(), 0);
}

#[tokio::test]
async fn an_invalid_immutable_source_url_is_rejected_before_fetching() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut manifest = manifest(CONTENT);
    manifest.source_url = "https://example.com/main/data.json".into();
    let no_fetch = NoFetch::new();

    let error = prepare_corpus(&manifest, dir.path(), &no_fetch)
        .await
        .expect_err("mutable source URL must be refused");

    assert!(error.to_string().contains("mutable branch"));
    assert_eq!(no_fetch.calls(), 0);
    assert!(!dir.path().join("test-corpus").exists());
}

#[tokio::test]
async fn an_auxiliary_hash_failure_removes_the_unpublished_staging_directory() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut manifest = manifest(CONTENT);
    manifest.auxiliary_files.push(CorpusFile {
        source_url: "https://example.com/auxiliary.json".into(),
        revision: "aux-rev".into(),
        sha256: hex::encode(sha2::Sha256::digest(b"auxiliary bytes")),
        byte_size: b"auxiliary bytes".len() as u64,
        data_file: "auxiliary.json".into(),
    });

    let error = prepare_corpus(&manifest, dir.path(), &MainAndCorruptAuxiliaryFetcher)
        .await
        .expect_err("bad auxiliary download must be refused");

    assert!(error.to_string().contains("auxiliary sha-256 mismatch"));
    let corpus_parent = dir.path().join("test-corpus");
    assert!(!corpus_parent.join("rev1").exists());
    let remaining_staging_entries = std::fs::read_dir(corpus_parent)
        .expect("read staging parent")
        .map(|entry| entry.expect("read staging entry").file_name())
        .collect::<Vec<_>>();
    assert!(
        remaining_staging_entries.is_empty(),
        "failed preparation must remove its private staging directory"
    );
}

#[tokio::test]
async fn a_failed_fetch_allows_a_later_preparation_to_succeed() {
    let dir = tempfile::tempdir().expect("temp dir");
    let manifest = manifest(CONTENT);
    let failing_fetcher = FailedFetcher::new();

    let failure = prepare_corpus(&manifest, dir.path(), &failing_fetcher).await;

    assert!(failure.is_err());
    assert_eq!(failing_fetcher.calls(), 1);
    assert!(!dir.path().join("test-corpus").exists());
    let retry_fetcher = BytesFetcher::new(CONTENT);
    let retried = prepare_corpus(&manifest, dir.path(), &retry_fetcher)
        .await
        .expect("retry after fetch failure");
    assert_eq!(retry_fetcher.calls(), 1);
    assert_eq!(
        std::fs::read(retried.data_path).expect("retried data"),
        CONTENT
    );
}

#[tokio::test]
async fn an_invalid_case_count_is_not_published_for_a_known_corpus() {
    let dir = tempfile::tempdir().expect("temp dir");
    let content = b"[]";
    let mut manifest = manifest(content);
    manifest.corpus_id = "longmemeval-cleaned".into();
    let fetcher = BytesFetcher::new(content);

    let error = prepare_corpus(&manifest, dir.path(), &fetcher)
        .await
        .expect_err("declared count must match normalized corpus");

    assert!(error.to_string().contains("case count mismatch"));
    assert_eq!(fetcher.calls(), 1);
    assert!(!dir.path().join("longmemeval-cleaned/rev1").exists());
}

#[tokio::test]
async fn a_cached_known_corpus_with_an_invalid_case_count_is_refused_without_fetching() {
    let dir = tempfile::tempdir().expect("temp dir");
    let content = b"[]";
    let mut manifest = manifest(content);
    manifest.corpus_id = "longmemeval-cleaned".into();
    let cache = dir.path().join("longmemeval-cleaned/rev1");
    std::fs::create_dir_all(&cache).expect("cache directory");
    std::fs::write(cache.join("data.json"), content).expect("cached data");
    let no_fetch = NoFetch::new();

    let error = prepare_corpus(&manifest, dir.path(), &no_fetch)
        .await
        .expect_err("invalid cached case count must be refused");

    assert!(error.to_string().contains("case count mismatch"));
    assert_eq!(no_fetch.calls(), 0);
}
