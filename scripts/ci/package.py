#!/usr/bin/env python3
"""Package and smoke-test native binaries without recompiling a Rust test harness."""

import hashlib
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import sys
import tempfile
import threading


def run(args, **kwargs):
    return subprocess.run(args, check=True, text=True, capture_output=True,
                          timeout=60, **kwargs).stdout


def smoke(cli, http, work):
    # Preserve the Windows loader environment, but isolate application state.
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("SURREALDB_", "MEMORY_", "NER_", "EMBEDDINGS_", "DYLD_", "ORT_"))}
    env.update(SURREALDB_EMBEDDED="true", SURREALDB_DATA_DIR=str(work / "db"),
               SURREALDB_DB_NAME="smoke", SURREALDB_NAMESPACE="main",
               SURREALDB_USERNAME="root", SURREALDB_PASSWORD="root",
               EMBEDDINGS_ENABLED="false", NER_EXTRACTOR="anno")
    print(run([str(cli), "--version"], env=env).strip())
    payload = json.loads(run([str(cli), "init", "--target", "vscode"], env=env))
    if payload.get("mutates_files") is not False or payload.get("target") != "vscode":
        raise RuntimeError("invalid init output")
    # Verify that the HTTP executable loads and reaches its configuration parser.
    # A live HTTP deployment requires explicit databases and credentials.
    result = subprocess.run([str(http)], env={**env, "MEMORY_MCP_HTTP_BIND": "invalid"},
                            text=True, capture_output=True, timeout=30)
    if result.returncode != 2 or "config error:" not in result.stderr:
        raise RuntimeError(f"HTTP configuration smoke failed: {result.stderr}")
    for selector in ("anno", "regex"):
        run([str(cli), "ingest", "--source-type", "smoke", "--source-id", selector,
             "--content", "Alice Smith from OpenAI presented Project Atlas.",
             "--t-ref", "2026-02-05T00:00:00Z"], env={**env, "NER_EXTRACTOR": selector})
    inbox = work / "inbox"
    inbox.mkdir()
    env["MEMORY_INGESTION_INBOX"] = str(inbox)
    with tempfile.TemporaryFile(mode="w+t") as errors:
        child = subprocess.Popen([str(cli), "serve"], env=env, stdin=subprocess.PIPE,
                                 stdout=subprocess.PIPE, stderr=errors, text=True)
        try:
            responses = queue.Queue()
            reader = threading.Thread(target=lambda: responses.put(child.stdout.readline()), daemon=True)
            reader.start()
            child.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                              "params": {"protocolVersion": "2025-11-25", "capabilities": {},
                                         "clientInfo": {"name": "ci-smoke", "version": "1"}}}) + "\n")
            child.stdin.flush()
            response = json.loads(responses.get(timeout=30))
            if response.get("id") != 1 or "error" in response or "result" not in response:
                raise RuntimeError(f"MCP initialization failed: {response}")
            child.stdin.close()
            if child.wait(timeout=35) != 0:
                raise RuntimeError("MCP shutdown failed")
            errors.seek(0)
            if "built without the fs-watch feature" in errors.read():
                raise RuntimeError("filesystem ingestion missing from artifact")
        except Exception:
            errors.seek(0)
            print(errors.read(), file=sys.stderr)
            raise
        finally:
            if child.poll() is None:
                child.kill()
            child.wait()
            child.stdout.close()
            if not child.stdin.closed:
                child.stdin.close()
            reader.join(timeout=5)


def package(build_dir, target):
    suffix = ".exe" if "windows" in target else ""
    dist = Path("dist")
    dist.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory() as temp:
        work = Path(temp)
        bundle = work / "bundle"
        bundle.mkdir()
        binaries = []
        for name in ("memory_mcp", "memory_mcp_http"):
            binary = bundle / (name + suffix)
            shutil.copy2(build_dir / binary.name, binary)
            binaries.append(binary)
        # Include any dynamic libraries supplied by the native dependency build.
        for pattern in ("*.dll", "*.so*", "*.dylib"):
            for library in build_dir.glob(pattern):
                shutil.copy2(library, bundle / library.name)
        if target == "x86_64-apple-darwin":
            runtime = Path(".ci/onnxruntime")
            libraries = list((runtime / "lib").glob("*.dylib"))
            if not libraries:
                raise RuntimeError("Intel macOS requires the bundled ONNX Runtime")
            for library in libraries:
                shutil.copy2(library, bundle / library.name)
            for notice in ("LICENSE", "ThirdPartyNotices.txt"):
                shutil.copy2(runtime / notice, bundle / ("ONNX-" + notice))
            for binary in binaries:
                run(["install_name_tool", "-add_rpath", "@executable_path", str(binary)])
                run(["codesign", "--force", "--sign", "-", str(binary)])
        shutil.copy2("LICENSE", bundle / "LICENSE")
        # No DYLD_LIBRARY_PATH: catch missing runtime libraries in the shipped folder.
        smoke(*binaries, work)
        archive = shutil.make_archive(str(dist / ("memory_mcp-" + target)),
                                      "zip" if suffix else "gztar", bundle)
        artifacts = [Path(archive)]
        # Preserve existing standalone CLI download names where no sidecar is needed.
        if not any(bundle.glob("*.dylib")) and not any(bundle.glob("*.dll")) and not any(bundle.glob("*.so*")):
            system = "windows" if suffix else ("macos" if "apple" in target else "linux")
            standalone = dist / f"memory_mcp_{system}_{target.split('-')[0]}{suffix}"
            shutil.copy2(binaries[0], standalone)
            artifacts.append(standalone)
        for artifact in artifacts:
            with artifact.open("rb") as source:
                digest = hashlib.file_digest(source, "sha256").hexdigest()
            artifact.with_name(artifact.name + ".sha256").write_text(digest + "\n")
        print(f"Packaged and smoke-tested {target}")


if __name__ == "__main__":
    package(Path(sys.argv[1]).resolve(), sys.argv[2])
