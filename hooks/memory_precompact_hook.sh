#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
HOOK_INPUT=""
if [[ ! -t 0 ]]; then
    HOOK_INPUT="$(cat)"
fi

export MEMORY_HOOK_EVENT="precompact"
export MEMORY_HOOK_DEFAULT_POLICY_TAGS="hook:precompact,session_summary,emergency_save"
export MEMORY_HOOK_REPO_ROOT="${REPO_ROOT}"
export MEMORY_HOOK_INPUT_JSON="${HOOK_INPUT}"

python3 - <<'PY'
import datetime as dt
import hashlib
import json
import os
import pathlib
import subprocess
import sys

PROTOCOL_VERSION = "2025-06-18"
REQUEST_TIMEOUT_SECS = 15


def utc_now_rfc3339() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z")


def load_hook_input() -> tuple[dict, str]:
    raw = os.environ.get("MEMORY_HOOK_INPUT_JSON", "")
    if not raw.strip():
        return {}, ""
    try:
        parsed = json.loads(raw)
    except json.JSONDecodeError:
        return {}, raw.strip()
    if isinstance(parsed, dict):
        return parsed, ""
    return {"raw_input": parsed}, ""


def transcript_excerpt(payload: dict) -> str:
    transcript_path = payload.get("transcript_path") or payload.get("transcriptPath")
    if not transcript_path:
        return ""

    path = pathlib.Path(str(transcript_path)).expanduser()
    if not path.exists() or not path.is_file():
        return ""

    max_lines = int(os.environ.get("MEMORY_HOOK_MAX_TRANSCRIPT_LINES", "80"))
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return ""
    excerpt = lines[-max_lines:]
    if not excerpt:
        return ""
    return "\n".join(excerpt)


def build_content(payload: dict, raw_fallback: str, event_name: str) -> str:
    explicit_content = os.environ.get("MEMORY_HOOK_CONTENT", "").strip()
    excerpt = transcript_excerpt(payload)

    header = [
        f"Hook event: {event_name}",
        f"Captured at: {utc_now_rfc3339()}",
    ]
    for key in ("session_id", "conversation_id", "status", "trigger", "hook_event_name", "custom_instructions"):
        value = payload.get(key)
        if value not in (None, ""):
            header.append(f"{key}: {value}")

    if explicit_content:
        body = explicit_content
    elif excerpt:
        body = f"Transcript excerpt:\n{excerpt}"
    elif raw_fallback:
        body = raw_fallback
    elif payload:
        body = json.dumps(payload, ensure_ascii=False, indent=2, sort_keys=True)
    else:
        body = "No hook payload was provided."

    return "\n".join(header) + "\n\n" + body


def source_id_for(content: str, payload: dict, event_name: str) -> str:
    seed = "::".join(
        [
            event_name,
            str(payload.get("session_id") or payload.get("conversation_id") or ""),
            content,
        ]
    )
    digest = hashlib.sha256(seed.encode("utf-8")).hexdigest()
    return f"{event_name}-{digest[:24]}"


def send(proc: subprocess.Popen[str], message: dict) -> None:
    assert proc.stdin is not None
    proc.stdin.write(json.dumps(message, ensure_ascii=False, separators=(",", ":")) + "\n")
    proc.stdin.flush()


def recv_response(proc: subprocess.Popen[str], request_id: int) -> dict:
    assert proc.stdout is not None
    while True:
        line = proc.stdout.readline()
        if line == "":
            raise RuntimeError("memory_mcp server closed stdout before responding")
        line = line.strip()
        if not line:
            continue
        message = json.loads(line)
        if message.get("id") != request_id:
            continue
        if "error" in message:
            raise RuntimeError(json.dumps(message["error"], ensure_ascii=False))
        return message.get("result", {})


def main() -> int:
    event_name = os.environ.get("MEMORY_HOOK_EVENT", "precompact")
    payload, raw_fallback = load_hook_input()
    content = build_content(payload, raw_fallback, event_name)
    source_type = os.environ.get("MEMORY_HOOK_SOURCE_TYPE", "session_summary").strip() or "session_summary"
    policy_tags = [
        tag.strip()
        for tag in os.environ.get("MEMORY_HOOK_POLICY_TAGS", os.environ.get("MEMORY_HOOK_DEFAULT_POLICY_TAGS", "session_summary")).split(",")
        if tag.strip()
    ]

    ingest_arguments = {
        "source_type": source_type,
        "source_id": source_id_for(content, payload, event_name),
        "content": content,
        "t_ref": utc_now_rfc3339(),
        "policy_tags": policy_tags,
    }

    server_cmd = os.environ.get("MEMORY_MCP_SERVER_CMD", "cargo run --quiet --bin memory_mcp")
    server_cwd = os.environ.get("MEMORY_MCP_SERVER_CWD", os.environ.get("MEMORY_HOOK_REPO_ROOT", str(pathlib.Path.cwd())))
    env = os.environ.copy()
    env.setdefault("SURREALDB_DB_NAME", "memory")
    env.setdefault("SURREALDB_EMBEDDED", "true")
    env.setdefault("SURREALDB_NAMESPACE", "main")
    env.setdefault("SURREALDB_USERNAME", "root")
    env.setdefault("SURREALDB_PASSWORD", "root")
    env.setdefault("SURREALDB_DATA_DIR", str(pathlib.Path(server_cwd) / "data" / "surrealdb"))
    env.setdefault("RUST_LOG", "error")

    proc = subprocess.Popen(
        server_cmd,
        cwd=server_cwd,
        env=env,
        shell=True,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=sys.stderr,
        text=True,
        bufsize=1,
    )

    try:
        send(
            proc,
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "memory-hook", "version": "1.0.0"},
                },
            },
        )
        recv_response(proc, 1)
        send(proc, {"jsonrpc": "2.0", "method": "notifications/initialized"})
        send(
            proc,
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "ingest",
                    "arguments": ingest_arguments,
                },
            },
        )
        recv_response(proc, 2)
    finally:
        if proc.stdin is not None and not proc.stdin.closed:
            proc.stdin.close()
        try:
            proc.wait(timeout=REQUEST_TIMEOUT_SECS)
        except subprocess.TimeoutExpired:
            proc.terminate()
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                proc.kill()

    if os.environ.get("MEMORY_HOOK_VERBOSE", "") == "1":
        print("memory-precompact-hook ingested summary")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
PY
