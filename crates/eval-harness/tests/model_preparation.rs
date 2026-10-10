use eval_harness::model_preparation::prepare_model;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

struct Mirror {
    url: String,
    stop: Arc<AtomicBool>,
    thread: thread::JoinHandle<()>,
}
impl Mirror {
    fn new(payload: &'static [u8]) -> Self {
        Self::with_allowed_paths(payload, Vec::new())
    }
    fn with_allowed_paths(payload: &'static [u8], allowed: Vec<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                let (mut socket, _) = listener.accept().expect("accept");
                if stopping.load(Ordering::Acquire) {
                    break;
                }
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                    .expect("bounded read");
                socket
                    .set_write_timeout(Some(std::time::Duration::from_secs(3)))
                    .expect("bounded write");
                let mut request = [0u8; 4096];
                let count = socket.read(&mut request).expect("request");
                assert!(count > 0);
                let raw = std::str::from_utf8(&request[..count]).expect("HTTP request");
                let requested = raw.split_whitespace().nth(1).expect("request path");
                if !allowed.is_empty() && !allowed.iter().any(|path| path == requested) {
                    socket.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").expect("reject wrong immutable source");
                    continue;
                }
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                )
                .expect("headers");
                socket.write_all(payload).expect("body");
            }
        });
        Self {
            url: format!("http://{address}"),
            stop,
            thread: worker,
        }
    }
    fn close(self) {
        self.stop.store(true, Ordering::Release);
        if let Err(error) =
            TcpStream::connect(self.url.strip_prefix("http://").expect("local address"))
        {
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::ConnectionRefused,
                "wake listener"
            );
        }
        self.thread.join().expect("join mirror");
    }
}
fn manifest(root: &std::path::Path) -> std::path::PathBuf {
    let hash = hex::encode(Sha256::digest(b"model"));
    let files = ["config.json", "tokenizer.json", "model.safetensors"]
        .map(|path| serde_json::json!({"path":path,"bytes":5,"sha256":hash}));
    let path = root.join("model.json");
    std::fs::write(&path,serde_json::to_vec(&serde_json::json!({"model":"test/model","revision":"0123456789012345678901234567890123456789","license":"MIT","files":files})).expect("encode")).expect("manifest");
    path
}

#[tokio::test]
async fn publishes_only_a_fully_verified_model_bundle() {
    let root = tempfile::tempdir().expect("temp");
    let path = manifest(root.path());
    let mirror = Mirror::new(b"model");
    let result = prepare_model(&path, &root.path().join("cache"), Some(&mirror.url)).await;
    mirror.close();
    let cache = result.expect("verified bundle");
    assert_eq!(
        std::fs::read(cache.join("model.safetensors")).expect("weights"),
        b"model"
    );
    assert_eq!(
        std::fs::read(cache.join("tokenizer.json")).expect("tokenizer"),
        b"model"
    );
}

#[tokio::test]
async fn same_size_wrong_hash_is_never_published() {
    let root = tempfile::tempdir().expect("temp");
    let path = manifest(root.path());
    let mirror = Mirror::new(b"wrong");
    let result = prepare_model(&path, &root.path().join("cache"), Some(&mirror.url)).await;
    mirror.close();
    assert!(
        result
            .expect_err("wrong checksum")
            .to_string()
            .contains("checksum")
    );
    assert!(
        !root
            .path()
            .join("cache/test--model/0123456789012345678901234567890123456789")
            .exists()
    );
}

#[tokio::test]
async fn a_verified_cache_is_usable_without_a_network_connection() {
    let root = tempfile::tempdir().expect("temp");
    let path = manifest(root.path());
    let mirror = Mirror::new(b"model");
    let first = prepare_model(&path, &root.path().join("cache"), Some(&mirror.url)).await;
    mirror.close();
    let first = first.expect("initial bundle");
    let reused = prepare_model(
        &path,
        &root.path().join("cache"),
        Some("http://127.0.0.1:9"),
    )
    .await
    .expect("cached bundle");
    assert_eq!(reused, first);
}

#[tokio::test]
async fn valid_dotted_hugging_face_model_names_can_be_prepared() {
    let root = tempfile::tempdir().expect("temp");
    let path = manifest(root.path());
    let mut lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("manifest")).expect("JSON");
    lock["model"] = "Qwen/Qwen3-Reranker-0.6B".into();
    std::fs::write(&path, serde_json::to_vec(&lock).expect("encode")).expect("lock");
    let mirror = Mirror::new(b"model");
    let result = prepare_model(&path, &root.path().join("cache"), Some(&mirror.url)).await;
    mirror.close();
    assert!(result.is_ok(), "valid pinned model name: {result:?}");
}

#[tokio::test]
async fn a_corrupted_cached_bundle_is_rejected_without_silent_reuse() {
    let root = tempfile::tempdir().expect("temp");
    let path = manifest(root.path());
    let mirror = Mirror::new(b"model");
    let result = prepare_model(&path, &root.path().join("cache"), Some(&mirror.url)).await;
    mirror.close();
    let cache = result.expect("initial bundle");
    std::fs::write(cache.join("model.safetensors"), b"wrong").expect("corrupt same-sized cache");
    let error = prepare_model(
        &path,
        &root.path().join("cache"),
        Some("http://127.0.0.1:9"),
    )
    .await
    .expect_err("corrupt cache");
    assert!(error.to_string().contains("checksum"), "{error}");
}

#[tokio::test]
async fn path_traversal_manifest_is_refused_before_creating_a_cache() {
    let root = tempfile::tempdir().expect("temp");
    let path = manifest(root.path());
    let mut lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("manifest")).expect("JSON");
    lock["model"] = "../escaped".into();
    std::fs::write(&path, serde_json::to_vec(&lock).expect("encode")).expect("lock");
    let error = prepare_model(
        &path,
        &root.path().join("cache"),
        Some("http://127.0.0.1:9"),
    )
    .await
    .expect_err("unsafe path");
    assert!(error.to_string().contains("safe owner/repository"));
    assert!(!root.path().join("cache").exists());
}

#[tokio::test]
async fn a_truncated_download_never_leaves_a_published_or_staged_bundle() {
    let root = tempfile::tempdir().expect("temp");
    let path = manifest(root.path());
    let mirror = Mirror::new(b"mod");
    let result = prepare_model(&path, &root.path().join("cache"), Some(&mirror.url)).await;
    mirror.close();
    let error = result.expect_err("truncated file");
    assert!(error.to_string().contains("wrong size"));
    let mut entries =
        std::fs::read_dir(root.path().join("cache/test--model")).expect("cache parent");
    assert!(
        entries.next().is_none(),
        "owned staging directory must be removed"
    );
}

#[tokio::test]
async fn quantized_bundle_can_pin_tokenizer_and_weights_to_separate_immutable_sources() {
    let root = tempfile::tempdir().expect("temp");
    let path = manifest(root.path());
    let mut lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("lock")).expect("JSON");
    lock["files"][2]["path"] = "model.gguf".into();
    lock["files"][2]["source"] = serde_json::json!({"model":"quantizer/model", "revision":"abcdefabcdefabcdefabcdefabcdefabcdefabcd", "path":"Model.Q8_0.gguf", "license":"Apache-2.0"});
    std::fs::write(&path, serde_json::to_vec(&lock).expect("encode")).expect("lock");
    let mirror = Mirror::with_allowed_paths(
        b"model",
        vec![
            "/test/model/resolve/0123456789012345678901234567890123456789/config.json".into(),
            "/test/model/resolve/0123456789012345678901234567890123456789/tokenizer.json".into(),
            "/quantizer/model/resolve/abcdefabcdefabcdefabcdefabcdefabcdefabcd/Model.Q8_0.gguf"
                .into(),
        ],
    );
    let result = prepare_model(&path, &root.path().join("cache"), Some(&mirror.url)).await;
    mirror.close();
    let bundle = result.expect("verified composite bundle");
    assert_eq!(
        std::fs::read(bundle.join("model.gguf")).expect("GGUF"),
        b"model"
    );
    assert!(
        !bundle.join("model.safetensors").exists(),
        "unused full precision weights must not be downloaded"
    );
}

#[tokio::test]
async fn unsafe_file_source_is_refused_before_any_download_or_cache_creation() {
    let root = tempfile::tempdir().expect("temp");
    let path = manifest(root.path());
    let mut lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("lock")).expect("JSON");
    lock["files"][2]["source"] = serde_json::json!({"model":"../escape", "revision":"abcdefabcdefabcdefabcdefabcdefabcdefabcd", "path":"model.safetensors", "license":"Apache-2.0"});
    std::fs::write(&path, serde_json::to_vec(&lock).expect("encode")).expect("lock");
    let error = prepare_model(
        &path,
        &root.path().join("cache"),
        Some("http://127.0.0.1:9"),
    )
    .await
    .expect_err("unsafe source");
    assert!(
        error.to_string().contains("safe owner/repository"),
        "{error}"
    );
    assert!(!root.path().join("cache").exists());
}
