import json
import unittest

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
