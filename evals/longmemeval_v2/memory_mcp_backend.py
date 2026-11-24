#!/usr/bin/env python3
"""LongMemEval-V2 memory backend adapter for memory_mcp.

Implements the official memory backend interface:

    class MemoryMcpBackend:
        def insert(self, trajectory):
            ...
        def query(self, query, query_image=None):
            ...

The adapter invokes the existing public ingest/inline extract and
assemble_context surfaces. It does not seed facts directly.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
from tempfile import TemporaryDirectory
from threading import RLock
import time
from typing import Any, Callable, Optional

try:
    from memory_modules.memory import Memory, register_memory
except ModuleNotFoundError as error:  # standalone smoke without upstream checkout
    if error.name not in {"memory_modules", "memory_modules.memory"}:
        raise
    class Memory:  # type: ignore[no-redef]
        def __init__(self, memory_params):
            self.memory_params = dict(memory_params)

    def register_memory(cls):  # type: ignore[no-redef]
        return cls


UNDATED_TRAJECTORY_REFERENCE = "2000-01-01T00:00:00Z"


@register_memory
class MemoryMcpBackend(Memory):
    """Memory backend for LongMemEval-V2 backed by memory_mcp."""

    memory_type = "memory_mcp"

    def __init__(self, memory_params: Optional[dict] = None, *, binary: str = "memory_mcp", runner: Optional[Callable[..., str]] = None) -> None:
        memory_params = memory_params or {}
        super().__init__(memory_params)
        binary = memory_params.get("binary", binary)
        self.db_path = memory_params.get("db_path")
        self.binary = binary
        self._store_lock = RLock()
        self._restored_store: Optional[TemporaryDirectory] = None
        self._runner = runner or self._run_cli
        self._insert_count = 0
        self._query_count = 0
        self._insert_time_secs = 0.0
        self._query_time_secs = 0.0

    def insert(self, trajectory: Any) -> None:
        """Insert a trajectory into memory via ingest + extract."""
        with self._store_lock:
            self._insert(trajectory)

    def _insert(self, trajectory: Any) -> None:
        start = time.monotonic()
        content = json.dumps(trajectory, ensure_ascii=False, sort_keys=True)
        digest = hashlib.sha256(content.encode("utf-8")).hexdigest()
        source_id = f"longmemeval_v2_trajectory_{digest}"

        # Step 1: ingest the raw trajectory content.
        ingest_output = self._runner(
            "ingest",
            "--source-type", "other",
            "--source-id", source_id,
            "--content", content,
            "--t-ref", UNDATED_TRAJECTORY_REFERENCE,
        )

        # Step 2: extract facts from the server-issued episode ID.
        payload = json.loads(ingest_output)
        result = payload.get("result")
        episode_id = payload.get("episode_id")
        if episode_id is None and isinstance(result, str):
            episode_id = result
        elif episode_id is None and isinstance(result, dict):
            episode_id = result.get("episode_id")
        if not episode_id:
            raise RuntimeError("ingest response did not contain an episode_id")
        self._runner(
            "extract",
            "--episode-id", episode_id,
        )

        self._insert_count += 1
        self._insert_time_secs += time.monotonic() - start

    def query(self, query: str, query_image: Optional[Any] = None) -> list[dict[str, str]]:
        """Query memory for relevant context."""
        if query_image is not None:
            raise NotImplementedError(
                "Image queries are not supported; full multimodal LongMemEval-V2 "
                "requires an explicit image representation design."
            )

        start = time.monotonic()
        result = self._runner(
            "assemble-context",
            "--query", query,
        )
        self._query_count += 1
        self._query_time_secs += time.monotonic() - start
        payload = json.loads(result)
        nested = payload.get("result")
        items = payload.get("items")
        if items is None and isinstance(nested, dict):
            items = nested.get("items")
        if items is None:
            items = nested
        if not isinstance(items, list):
            raise RuntimeError("assemble-context response did not contain a list")
        return [{"type": "text", "value": item.get("content", str(item)) if isinstance(item, dict) else str(item)} for item in items]

    @property
    def stats(self) -> dict:
        return {
            "insert_count": self._insert_count,
            "query_count": self._query_count,
            "insert_time_secs": round(self._insert_time_secs, 3),
            "query_time_secs": round(self._query_time_secs, 3),
        }

    def _save_backend(self, output_dir: Path) -> None:
        """Snapshot the dedicated, quiescent CLI store into the upstream save."""
        with self._store_lock:
            if not self.db_path or not Path(self.db_path).is_dir():
                raise RuntimeError("saving memory_mcp requires a built dedicated CLI store")
            store = Path(self.db_path).resolve()
            destination = output_dir.resolve() / "store"
            if destination.is_relative_to(store):
                raise ValueError("memory snapshot must be outside the active CLI store")
            if destination.exists():
                raise FileExistsError(destination)
            with TemporaryDirectory(prefix=".snapshot-", dir=output_dir) as staging:
                staged_store = Path(staging) / "store"
                shutil.copytree(store, staged_store)
                staged_store.rename(destination)

    def _load_backend(self, input_dir: Path) -> None:
        """Clone the snapshot so each restore gets its own mutable store."""
        with self._store_lock:
            source = input_dir / "store"
            if not source.is_dir():
                raise RuntimeError("saved memory_mcp state is missing its store snapshot")
            workspace = TemporaryDirectory(prefix="longmemeval-mcp-restore-")
            try:
                store = Path(workspace.name) / "store"
                shutil.copytree(source, store)
            except BaseException:
                workspace.cleanup()
                raise
            if self._restored_store is not None:
                self._restored_store.cleanup()
            self._restored_store = workspace
            self.db_path = str(store)

    def _run_cli(self, subcommand: str, *args: str) -> str:
        with self._store_lock:
            cmd = [self.binary]
            if not self.db_path:
                raise ValueError("memory_mcp requires an explicit dedicated db_path")
            env = {
                name: os.environ[name]
                for name in ("PATH", "SYSTEMROOT", "WINDIR", "TEMP", "TMP", "TMPDIR")
                if name in os.environ
            }
            env.update({
                "SURREALDB_EMBEDDED": "true",
                "SURREALDB_DATA_DIR": self.db_path,
                "EMBEDDINGS_ENABLED": "false",
                "RUST_LOG": "error",
            })
            argv = list(args)
            stdin_content = None
            if subcommand == "ingest":
                content_index = argv.index("--content")
                stdin_content = argv[content_index + 1].encode("utf-8")
                argv[content_index:content_index + 2] = ["--content-stdin"]
            cmd += [subcommand, *argv]
            result = subprocess.run(
                cmd, input=stdin_content, capture_output=True, check=True, env=env
            )
            return result.stdout.decode("utf-8")


def _smoke_test() -> None:
    calls = []
    responses = iter([
        json.dumps({"result": {"episode_id": "episode:server-issued"}}),
        json.dumps({"result": {"ok": True}}),
        json.dumps({"result": {"items": [{"content": "remembered fact"}]}}),
    ])

    def fake_runner(subcommand: str, *args: str) -> str:
        calls.append((subcommand, args))
        return next(responses)

    backend = MemoryMcpBackend({"db_path": "/tmp/longmemeval-v2-smoke"}, runner=fake_runner)
    backend.insert({"role": "user", "content": "I prefer tea"})
    result = backend.query("What do I prefer?")
    assert result == [{"type": "text", "value": "remembered fact"}]
    assert calls[0][0] == "ingest" and "--scope" not in calls[0][1]
    assert "other" in calls[0][1]
    assert calls[1] == ("extract", ("--episode-id", "episode:server-issued"))
    print("LongMemEval-V2 adapter smoke: PASS")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--smoke-test", action="store_true")
    args = parser.parse_args()
    if args.smoke_test:
        _smoke_test()
