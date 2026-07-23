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
import subprocess
import time
from typing import Any, Optional


class MemoryMcpBackend:
    """Memory backend for LongMemEval-V2 backed by memory_mcp."""

    def __init__(self, binary: str = "memory_mcp") -> None:
        self.binary = binary
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
        self._run_cli(
            "ingest",
            "--source-type", "longmemeval_v2",
            "--source-id", source_id,
            "--content", content,
            "--t-ref", _now_iso(),
            "--scope", "org",
        )

        # Step 2: extract facts from the ingested episode.
        episode_id = f"episode:{source_id}"  # deterministic ID
        self._run_cli(
            "extract",
            "--episode-id", episode_id,
        )

        self._insert_count += 1
        self._insert_time_secs += time.monotonic() - start

    def query(self, query: str, query_image: Optional[Any] = None) -> str:
        """Query memory for relevant context."""
        if query_image is not None:
            raise NotImplementedError(
                "Image queries are not supported; full multimodal LongMemEval-V2 "
                "requires an explicit image representation design."
            )

        start = time.monotonic()
        result = self._run_cli(
            "assemble-context",
            "--query", query,
            "--scope", "org",
        )
        self._query_count += 1
        self._query_time_secs += time.monotonic() - start
        return result

    @property
    def stats(self) -> dict:
        return {
            "insert_count": self._insert_count,
            "query_count": self._query_count,
            "insert_time_secs": round(self._insert_time_secs, 3),
            "query_time_secs": round(self._query_time_secs, 3),
        }

    def _run_cli(self, subcommand: str, *args: str) -> str:
        cmd = [self.binary, subcommand, *args]
        result = subprocess.run(cmd, capture_output=True, text=True, check=True)
        return result.stdout


def _now_iso() -> str:
    from datetime import datetime, timezone
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
