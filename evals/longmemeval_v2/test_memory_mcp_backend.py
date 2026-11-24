from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory
from threading import Barrier
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

    def test_segmented_method_preserves_complete_ordered_trajectory_under_anno_limit(self):
        bodies = []
        identities = []

        def runner(command, *args):
            if command == "ingest":
                bodies.append(args[args.index("--content") + 1])
                identities.append(args[args.index("--source-id") + 1])
                return '{"result":"episode:issued"}'
            return "{}"

        trajectory = {"id": "demo", "states": [
            {"state_index": 0, "text": "я" * 550_000},
            {"state_index": 1, "text": "the last state"},
        ]}
        backend = MemoryMcpBackend({"formation_method": "ordered-segments-v1"}, runner=runner)
        backend.insert(trajectory)

        self.assertGreater(len(bodies), 1)
        self.assertTrue(all(len(body.encode("utf-8")) <= 1_048_576 for body in bodies))
        self.assertEqual(json.loads("".join(bodies)), trajectory)
        self.assertEqual(len(set(identities)), len(bodies))
        self.assertEqual(backend.stats["insert_count"], 1)

    def test_segmented_method_keeps_adjacent_states_intact_when_each_fits(self):
        bodies = []

        def runner(command, *args):
            if command == "ingest":
                bodies.append(args[args.index("--content") + 1])
                return '{"result":"episode:issued"}'
            return "{}"

        trajectory = {"states": [
            {"state_index": 0, "text": "a" * 600_000},
            {"state_index": 1, "text": "b" * 600_000},
        ]}
        MemoryMcpBackend({"formation_method": "ordered-segments-v1"}, runner=runner).insert(trajectory)

        self.assertEqual(len(bodies), 2)
        self.assertEqual(bodies[0].find('"state_index": 1'), -1)
        self.assertTrue(bodies[1].startswith('{"state_index": 1, '))
        self.assertEqual(json.loads("".join(bodies)), trajectory)

    def test_segmented_retry_reuses_source_ids_after_a_failed_extract(self):
        attempts = []
        fail = True

        def runner(command, *args):
            if command == "ingest":
                source_id = args[args.index("--source-id") + 1]
                attempts.append(source_id)
                return json.dumps({"result": source_id})
            if fail and len(attempts) == 2:
                raise RuntimeError("extract interrupted")
            return "{}"

        params = {"formation_method": "ordered-segments-v1"}
        trajectory = {"states": [{"text": "x" * 600_000}, {"text": "y" * 600_000}]}
        first = MemoryMcpBackend(params, runner=runner)
        with self.assertRaisesRegex(RuntimeError, "extract interrupted"):
            first.insert(trajectory)
        self.assertEqual(first.stats["insert_count"], 0)

        fail = False
        restarted = MemoryMcpBackend(params, runner=runner)
        restarted.insert(trajectory)
        self.assertEqual(len(attempts), 4)
        self.assertEqual(attempts[:2], attempts[2:])
        self.assertEqual(restarted.stats["insert_count"], 1)

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

    def test_isolated_cli_reads_can_overlap(self):
        both_reading = Barrier(2, timeout=3)

        def read(argv, *, env, **kwargs):
            store = Path(env["SURREALDB_DATA_DIR"])
            marker = store / "marker"
            marker.write_text(argv[-1])
            both_reading.wait()
            reply = {"result": {"items": [{"content": marker.read_text()}]}}
            return subprocess.CompletedProcess([], 0, json.dumps(reply).encode())

        with TemporaryDirectory() as directory, patch("subprocess.run", side_effect=read):
            marker = Path(directory) / "marker"
            marker.write_text("original")
            backend = MemoryMcpBackend({"db_path": directory})
            with ThreadPoolExecutor(max_workers=2) as executor:
                tea = executor.submit(backend.query, "tea")
                coffee = executor.submit(backend.query, "coffee")
                self.assertEqual(tea.result(), [{"type": "text", "value": "tea"}])
                self.assertEqual(coffee.result(), [{"type": "text", "value": "coffee"}])
            self.assertEqual(marker.read_text(), "original")
            self.assertEqual(backend.stats["query_count"], 2)

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
