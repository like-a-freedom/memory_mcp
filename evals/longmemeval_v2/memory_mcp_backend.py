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

import json
import argparse
import os
import subprocess
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
        self._runner = runner or self._run_cli
        self._insert_count = 0
        self._query_count = 0
        self._insert_time_secs = 0.0
        self._query_time_secs = 0.0

    def insert(self, trajectory: Any) -> None:
        """Insert a trajectory into memory via ingest + extract."""
        start = time.monotonic()
        content = json.dumps(trajectory, ensure_ascii=False)
        source_id = f"longmemeval_v2_trajectory_{self._insert_count}"

        # Step 1: ingest the raw trajectory content.
        ingest_output = self._runner(
            "ingest",
            "--source-type", "other",
            "--source-id", source_id,
            "--content", content,
            "--t-ref", _now_iso(),
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

    def _run_cli(self, subcommand: str, *args: str) -> str:
        cmd = [self.binary]
        env = os.environ.copy()
        if self.db_path:
            # The CLI exposes embedded storage through the documented
            # environment contract; `--db` is not a global option.
            env["SURREALDB_EMBEDDED"] = "true"
            env["SURREALDB_DATA_DIR"] = self.db_path
        cmd += [subcommand, *args]
        result = subprocess.run(
            cmd, capture_output=True, text=True, check=True, env=env
        )
        return result.stdout


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


def _now_iso() -> str:
    from datetime import datetime, timezone
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--smoke-test", action="store_true")
    args = parser.parse_args()
    if args.smoke_test:
        _smoke_test()
