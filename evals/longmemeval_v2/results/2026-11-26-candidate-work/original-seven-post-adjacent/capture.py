"""One local original-protocol replay; private stores and owned servers are cleaned."""
from pathlib import Path
import hashlib
import json
import os
import platform
import signal
import shutil
import socket
import subprocess
import tempfile
import tarfile
import time
import urllib.request

repo = Path.cwd()
root = Path(__file__).resolve().parent
native = repo / "target/c4-native"
profile_path = repo / "evals/longmemeval_v2/small-web-native-profile.json"
profile = json.loads(profile_path.read_text())
assert platform.system() == "Darwin" and platform.machine() == "arm64"
assert not (root / "registration.json").exists(), "A capture is never overwritten or retried"


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(8 * 1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def save(name, value):
    (root / name).write_text(json.dumps(value, indent=2) + "\n")


checks = json.loads((root.parent / "verification/adjacent-groups-current/commands.json").read_text())
assert {x["name"] for x in checks if x["exit_code"] == 0} >= {
    "workspace-clippy", "diagnostic-clippy", "fmt", "diff", "scoped-semantic", "harness"
}
assert any(x["name"] in {"production", "production-reconciled"} and x["exit_code"] == 0 for x in checks)
comparison = json.loads((root.parent / "native-gate-adjacent/comparison.json").read_text())
assert comparison["all_cases_pass"] and comparison["no_regressions"]
assert comparison["counts"] == {"passed": 8, "failed": 0, "invalid": 0, "unobserved": 0}
save("native-gate-reference.json", comparison)
binary_info = json.loads((root.parent / "verification/adjacent-groups-current/release-build.json").read_text())
binary = Path(binary_info["binary"])
assert binary_info["exit_code"] == 0 and digest(binary) == binary_info["sha256"]
capture = json.loads((root.parent / "adjacent-groups-final/registration.json").read_text())
source = dict(capture["candidate_source_sha256"])
assert all(digest(repo / p) == h for p, h in source.items())
for p in ["evals/longmemeval_v2/scored_coverage.py", "evals/longmemeval_v2/memory_mcp_backend.py",
          "evals/longmemeval_v2/reviewed_span_coverage.py", "evals/longmemeval_v2/text_cohort.py"]:
    source[p] = digest(repo / p)
upstream = native / "upstream"
# This staged source is an archive, not a git checkout. Never resolve parent HEAD.
upstream_commit = profile["upstream_commit"]
archive = native / "upstream.tar.gz"
assert digest(archive) == "b03f1eb15b590f801d15084b637f3e35acc58e2431a0a3bc7d5990ca19e8e1c1"
prefix = f"LongMemEval-V2-{upstream_commit}/"
upstream_hashes = {}
with tarfile.open(archive) as bundle:
    for member in bundle.getmembers():
        assert member.name == prefix.rstrip("/") or member.name.startswith(prefix)
        if not member.isfile():
            continue
        name = member.name[len(prefix):]
        expected = hashlib.sha256(bundle.extractfile(member).read()).hexdigest()
        assert digest(upstream / name) == expected, f"Upstream source mismatch: {name}"
        upstream_hashes[name] = expected
assert len(upstream_hashes) == 53
assert {str(p.relative_to(upstream)) for p in upstream.rglob("*.py")} == {
    name for name in upstream_hashes if name.endswith(".py")}
save("upstream-provenance.json", {"commit": upstream_commit, "archive_sha256": digest(archive),
     "verification": "all 53 staged source files match the retained commit-named archive; no extra Python modules",
     "prior_independent_checkout_audit": "evals/longmemeval_v2/results/2026-11-25-c3-final-query-fix/RECHECK.md",
     "file_sha256": upstream_hashes})
model_paths = {role: Path.home() / ".cache/lm-studio/models" / definition["checkpoint"]
               for role, definition in profile["models"].items()}
model_hashes = {}
for role, definition in profile["models"].items():
    actual = {name: digest(model_paths[role] / name) for name in definition["files"]}
    assert actual == definition["files"], f"Pinned {role} model mismatch"
    model_hashes[role] = actual
processor = profile["processor"]
processor_cache = native / "hf/hub/models--Qwen--Qwen3.5-9B"
assert (processor_cache / "refs/main").read_text().strip() == processor["revision"]
processor_hashes = {name: digest(processor_cache / "snapshots" / processor["revision"] / name)
                    for name in processor["files"]}
assert processor_hashes == processor["files"]
snapshot = native / "build/memory_state/store"
snapshot_hashes = {str(p.relative_to(snapshot)): digest(p) for p in sorted(snapshot.rglob("*")) if p.is_file()}
assert snapshot_hashes == capture["snapshot_file_sha256"]
inputs = {str(p.relative_to(repo)): digest(p) for p in [
    profile_path, repo / "evals/longmemeval_v2/small-web-text-cohort.json",
    native / "cohort/questions.json", native / "cohort/haystack.json", native / "cohort/trajectories.json",
    native / "build/build-coverage.json"]}
env = {k: v for k, v in os.environ.items() if k in {"PATH", "HOME", "TMPDIR"}}
env.update(PYTHONPATH=f"{repo}:{upstream}", HF_HOME=str(native / "hf"),
           HF_HUB_OFFLINE="1", TRANSFORMERS_OFFLINE="1", PYTHONUNBUFFERED="1")
python = native / "env/bin/python"
mlx_python = native / "mlx-env/bin/python"
versions = {}
for label, executable, packages in [
    ("scorer", python, ["torch", "torchvision", "transformers", "huggingface_hub", "openai"]),
    ("local_runtime", mlx_python, ["mlx", "mlx-lm"])]:
    code = "import json,platform,importlib.metadata as m; print(json.dumps({'python':platform.python_version(),'packages':{p:m.version(p) for p in " + repr(packages) + "}}))"
    versions[label] = json.loads(subprocess.check_output([str(executable), "-c", code], env=env, text=True, timeout=30))
registration = {
    "schema": "original-seven-replay-registration/v1", "native_only": True,
    "provider": "standalone MLX-LM loopback; cached weight paths only, no LM Studio runtime",
    "purpose": "One replay of the seven exposed original questions after bounded tab-group repair, deterministic original2/2 and native8 gates; not independent confirmation",
    "binary": binary_info, "source_sha256": source, "input_sha256": inputs,
    "upstream_commit": upstream_commit, "upstream_file_sha256": upstream_hashes,
    "model_sha256": model_hashes, "processor_sha256": processor_hashes,
    "snapshot_file_sha256": snapshot_hashes,
    "runtime_versions": versions, "runtime_contract": profile["runtime"],
    "driver_sha256": digest(Path(__file__)), "no_reruns": True,
    "question_edits": False, "prompt_edits": False, "reformation": False,
    "read_worker_count": 3, "reader_concurrency": 1,
}
save("registration.json", registration)
commands = []


def run(name, command, timeout):
    start = time.monotonic()
    with (root / (name + ".log")).open("w") as log:
        child = subprocess.Popen([str(p) for p in command], env=env, cwd=repo,
                                 stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            code, timed_out = child.wait(timeout=timeout), False
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGTERM)
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=10)
            code, timed_out = child.returncode, True
    row = {"name": name, "command": [str(p) for p in command], "exit_code": code,
           "external_timeout": timed_out, "elapsed_seconds": time.monotonic() - start}
    commands.append(row)
    save("commands.json", commands)
    print(json.dumps(row), flush=True)
    return code == 0


assert run("processor-preflight", [python, "-c", "from evaluation.harness import get_memory_context_processor; get_memory_context_processor()"], 60)
servers = []
logs = []
try:
    for role, port in [("reader", 18767), ("judge", 18766)]:
        with socket.socket() as s:
            s.bind(("127.0.0.1", port))
        command = [str(mlx_python), "-m", "mlx_lm.server", "--host", "127.0.0.1", "--port", str(port),
                   "--model", str(model_paths[role]), "--temp", "0", "--chat-template-args", '{"enable_thinking":false}']
        log = (root / (role + "-server.log")).open("w")
        logs.append(log)
        server = subprocess.Popen(command, env=env, cwd=repo, stdout=log, stderr=subprocess.STDOUT)
        servers.append((role, server))
        save("owned-servers.json", [{"role": r, "pid": s.pid} for r, s in servers])
        deadline = time.monotonic() + 180
        while True:
            assert server.poll() is None, f"{role} server exited during startup"
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{port}/v1/models", timeout=3) as response:
                    assert response.status == 200
                    break
            except OSError:
                if time.monotonic() >= deadline:
                    raise TimeoutError(f"{role} startup deadline")
                time.sleep(1)
    with tempfile.TemporaryDirectory(prefix="candidate-original-seven-") as temp:
        build = Path(temp) / "build"
        state = build / "memory_state"
        state.mkdir(parents=True)
        shutil.copytree(snapshot, state / "store")
        shutil.copy2(native / "build/build-coverage.json", build / "build-coverage.json")
        config = json.loads((native / "build/memory_state/memory_config.json").read_text())
        config["memory_params"].update(binary=str(binary), db_path=str(build / "working-store"))
        (state / "memory_config.json").write_text(json.dumps(config, indent=2) + "\n")
        save("memory-config-reference.json", config)
        run("scored-pass", [python, "-m", "evals.longmemeval_v2.scored_coverage", "--domain", "web",
            "--questions", native / "cohort/questions.json", "--haystack", native / "cohort/haystack.json",
            "--trajectories", native / "cohort/trajectories.json", "--memory-state", state,
            "--reader-url", "http://127.0.0.1:18767/v1", "--reader-model", model_paths["reader"],
            "--judge-url", "http://127.0.0.1:18766/v1", "--judge-model", model_paths["judge"],
            "--output", root / "scores", "--prompt-workers", "3"], 2700)
        if (root / "scores/per_question.jsonl").exists():
            run("reviewed-span", [python, "-m", "evals.longmemeval_v2.reviewed_span_coverage",
                "--cohort", repo / "evals/longmemeval_v2/small-web-text-cohort.json",
                "--trajectories", native / "cohort/trajectories.json",
                "--per-question", root / "scores/per_question.jsonl", "--output", root / "reviewed-span-coverage.json"], 60)
finally:
    for role, server in servers:
        if server.poll() is None:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=10)
    for log in logs:
        log.close()
    save("server-cleanup.json", [{"role": role, "pid": server.pid, "exit_code": server.returncode} for role, server in servers])
    save("final-integrity.json", {
        "binary_unchanged": digest(binary) == binary_info["sha256"],
        "source_unchanged": all(digest(repo / p) == h for p, h in source.items()),
        "snapshot_unchanged": {str(p.relative_to(snapshot)): digest(p) for p in sorted(snapshot.rglob("*")) if p.is_file()} == snapshot_hashes,
        "model_assets_unchanged": all(digest(model_paths[role] / name) == h for role, hashes in model_hashes.items() for name, h in hashes.items()),
        "processor_assets_unchanged": all(digest(processor_cache / "snapshots" / processor["revision"] / name) == h for name, h in processor_hashes.items()),
        "inputs_unchanged": all(digest(repo / p) == h for p, h in inputs.items()),
        "upstream_unchanged": all(digest(upstream / p) == h for p, h in upstream_hashes.items()),
    })
