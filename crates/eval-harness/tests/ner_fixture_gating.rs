//! Public fixture-builder integration: owned checkpoints and observed network
//! effects in separate child processes, never global environment mutation.
use std::io::{Read, Seek, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[tokio::test]
#[ignore = "explicit child fixture executed by each gating scenario"]
async fn fixture_builder_child() {
    use memory_mcp::config::{GlinerDeviceKind, NerExtractorKind};
    let kind = match std::env::var("GATING_KIND").expect("kind").as_str() {
        "onnx" => NerExtractorKind::AnnoOnnx,
        "gliner" => NerExtractorKind::ClassicGliner,
        "vago" => NerExtractorKind::SauerkrautLfm25,
        other => panic!("unknown fixture kind {other}"),
    };
    let root = std::path::PathBuf::from(std::env::var_os("GATING_ROOT").expect("root"));
    let result =
        eval_harness::ner_fixtures::build_extractor_from_root(&root, kind, GlinerDeviceKind::Cpu)
            .await;
    assert!(
        result.is_none(),
        "missing/incomplete checkpoint must not construct"
    );
    println!("GATING_REJECTED");
}

fn reject_fixture(kind: &str, directory: &str, partial: bool) {
    let root = tempfile::tempdir().expect("owned checkpoint root");
    if partial {
        let model = root.path().join(directory);
        std::fs::create_dir(&model).expect("partial directory");
        std::fs::write(model.join("tokenizer.json"), "{}").expect("partial checkpoint");
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("download observer");
    listener.set_nonblocking(true).expect("bounded observer");
    let endpoint = format!(
        "http://{}",
        listener.local_addr().expect("observer address")
    );
    let mut output = tempfile::tempfile().expect("owned child output");
    let mut child = Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--exact",
            "fixture_builder_child",
            "--ignored",
            "--nocapture",
        ])
        .env("GATING_ROOT", root.path())
        .env("GATING_KIND", kind)
        .env("HF_ENDPOINT", &endpoint)
        .env("HTTP_PROXY", &endpoint)
        .env("HTTPS_PROXY", &endpoint)
        .env("http_proxy", &endpoint)
        .env("https_proxy", &endpoint)
        .env("NO_PROXY", "")
        .env("no_proxy", "")
        .stdin(Stdio::null())
        .stdout(Stdio::from(output.try_clone().expect("capture stdout")))
        .stderr(Stdio::from(output.try_clone().expect("capture stderr")))
        .spawn()
        .expect("spawn fixture builder");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut network_attempted = false;
    let status = loop {
        match listener.accept() {
            Ok((mut socket, _)) => {
                network_attempted = true;
                socket
                    .set_write_timeout(Some(Duration::from_millis(100)))
                    .expect("socket bound");
                let _ = socket.write_all(b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\n\r\n");
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("observer failed: {error}"),
        }
        if let Some(status) = child.try_wait().expect("observe child") {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("stop timed-out builder");
            break child.wait().expect("reap timed-out builder");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    network_attempted |= listener.accept().is_ok();
    output.rewind().expect("read child output");
    let mut log = String::new();
    output.read_to_string(&mut log).expect("child log");
    assert!(status.success(), "builder did not reject promptly: {log}");
    assert!(
        log.contains("GATING_REJECTED"),
        "child scenario actually ran: {log}"
    );
    assert!(
        !network_attempted,
        "local fixture gating must not attempt a download"
    );
}

macro_rules! gating_case {
    ($name:ident, $kind:literal, $directory:literal, $partial:literal) => {
        #[test]
        fn $name() {
            reject_fixture($kind, $directory, $partial);
        }
    };
}
gating_case!(absent_onnx, "onnx", "deepanwa--NuNerZero_onnx", false);
gating_case!(incomplete_onnx, "onnx", "deepanwa--NuNerZero_onnx", true);
gating_case!(absent_gliner, "gliner", "urchade--gliner_multi-v2.1", false);
gating_case!(
    incomplete_gliner,
    "gliner",
    "urchade--gliner_multi-v2.1",
    true
);
gating_case!(
    absent_vago,
    "vago",
    "VAGOsolutions--SauerkrautLM-LFM2.5-GLiNER",
    false
);
gating_case!(
    incomplete_vago,
    "vago",
    "VAGOsolutions--SauerkrautLM-LFM2.5-GLiNER",
    true
);
