import json
import os
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from evals.longmemeval_v2.memory_mcp_backend import MemoryMcpBackend


class MemoryMcpBackendTests(unittest.TestCase):
    def test_backend_exposes_upstream_memory_config(self):
        backend = MemoryMcpBackend({"db_path": "/tmp/test-memory-mcp"}, runner=lambda *_: "{}")
        self.assertEqual(backend.memory_type, "memory_mcp")
        self.assertEqual(backend.memory_params, {"db_path": "/tmp/test-memory-mcp"})

    def test_insert_uses_server_episode_id_and_query_returns_text_items(self):
        calls = []
        responses = iter([
            json.dumps({"result": {"episode_id": "episode:issued"}}),
            json.dumps({"result": {"ok": True}}),
            json.dumps({"result": {"items": [{"content": "tea"}]}}),
        ])

        def runner(command, *args):
            calls.append((command, args))
            return next(responses)

        backend = MemoryMcpBackend({"db_path": "/tmp/test-memory-mcp"}, runner=runner)
        backend.insert({"content": "I prefer tea"})
        self.assertEqual(backend.query("What do I prefer?"), [{"type": "text", "value": "tea"}])
        self.assertEqual(calls[1], ("extract", ("--episode-id", "episode:issued")))
        self.assertNotIn("--scope", calls[0][1])

    def test_trajectory_source_identity_survives_restart_and_reordering(self):
        identities = []

        def runner(command, *args):
            if command == "ingest":
                identities.append((
                    args[args.index("--source-id") + 1], args[args.index("--t-ref") + 1],
                ))
                return json.dumps({"result": "episode:issued"})
            return "{}"

        first = MemoryMcpBackend({}, runner=runner)
        first.insert({"content": "separate history"})
        first.insert({"content": "tea", "role": "user"})
        restarted = MemoryMcpBackend({}, runner=runner)
        restarted.insert({"role": "user", "content": "tea"})

        self.assertNotEqual(identities[0], identities[1])
        self.assertEqual(identities[1], identities[2])
        self.assertEqual(identities[2][1], "2000-01-01T00:00:00Z")

    def test_local_cli_uses_dedicated_store_without_inheriting_provider_or_remote_db(self):
        parent = {"SURREALDB_URL": "wss://example.invalid", "EMBEDDINGS_PROVIDER": "openai"}
        replies = [
            subprocess.CompletedProcess([], 0, b'{"result":"episode:issued"}'),
            subprocess.CompletedProcess([], 0, b"{}"),
        ]
        with patch.dict(os.environ, parent, clear=True), patch("subprocess.run", side_effect=replies) as run:
            MemoryMcpBackend({"db_path": "/tmp/test-memory-mcp"}).insert({"content": "чай"})

        argv = run.call_args_list[0].args[0]
        options = run.call_args_list[0].kwargs
        self.assertIn("--content-stdin", argv)
        self.assertNotIn("--content", argv)
        self.assertEqual(json.loads(options["input"]), {"content": "чай"})
        self.assertEqual(options["env"]["SURREALDB_DATA_DIR"], "/tmp/test-memory-mcp")
        self.assertNotIn("SURREALDB_URL", options["env"])
        self.assertNotIn("EMBEDDINGS_PROVIDER", options["env"])

    def test_each_query_starts_from_the_same_store_in_either_order(self):
        used_stores = set()

        def read(*args, env, **kwargs):
            query = args[0][-1]
            store = Path(env["SURREALDB_DATA_DIR"])
            used_stores.add(store)
            counter = store / "access-count"
            count = int(counter.read_text())
            counter.write_text(str(count + 1))
            reply = {"result": {"items": [{"content": f"{query}:{count}"}]}}
            return subprocess.CompletedProcess([], 0, json.dumps(reply).encode())

        with TemporaryDirectory() as directory, patch("subprocess.run", side_effect=read):
            counter = Path(directory) / "access-count"
            counter.write_text("0")
            backend = MemoryMcpBackend({"db_path": directory})
            forward = {query: backend.query(query) for query in ("tea", "coffee")}
            backward = {query: backend.query(query) for query in ("coffee", "tea")}

            self.assertEqual(forward, backward)
            self.assertEqual(forward["tea"], [{"type": "text", "value": "tea:0"}])
            self.assertEqual(counter.read_text(), "0")
            self.assertEqual(len(used_stores), 4)
            self.assertFalse(any(store.exists() for store in used_stores))

    def test_blank_context_is_a_failed_read(self):
        backend = MemoryMcpBackend({}, runner=lambda *_: '{"result":{"items":[{"content":"  "}]}}')

        with self.assertRaisesRegex(RuntimeError, "nonempty text"):
            backend.query("What do I prefer?")

    def test_successful_no_hit_returns_an_empty_context(self):
        backend = MemoryMcpBackend({}, runner=lambda *_: '{"result":{"items":[]}}')

        self.assertEqual(backend.query("What do I prefer?"), [])

    def test_image_queries_are_explicitly_unsupported(self):
        backend = MemoryMcpBackend({}, runner=lambda *_: "{}")
        with self.assertRaises(NotImplementedError):
            backend.query("describe image", query_image="image-bytes")

    def test_insert_accepts_public_cli_string_result(self):
        calls = []
        responses = iter([
            json.dumps({"status": "success", "result": "episode:issued"}),
            json.dumps({"status": "success", "result": {}}),
        ])

        def runner(command, *args):
            calls.append((command, args))
            return next(responses)

        MemoryMcpBackend({}, runner=runner).insert({"content": "tea"})
        self.assertEqual(calls[1], ("extract", ("--episode-id", "episode:issued")))


if __name__ == "__main__":
    unittest.main()
